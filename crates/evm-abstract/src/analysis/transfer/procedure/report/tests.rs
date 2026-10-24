use super::*;
use crate::{
    U256,
    domain::{EvmValue, symbolic::ExprId},
    resource::clone_payment::tests::{
        RECEIPT, Receipt, SummaryPolicy, Word, reset, summary_fixture,
    },
};

fn source() -> Source {
    Source {
        namespace: 7,
        procedure: 2,
        entry_revision: 3,
        kind: OutcomeKind::Return,
        generation: 4,
    }
}

fn opaque() -> (
    crate::domain::ValueAlgebra<SummaryPolicy>,
    EvmByteArraySummary<Word>,
    EvmByteArray<Word>,
) {
    let (_, _, _, algebra) = summary_fixture("00");
    let mut setup = WorkBudget::new(100_000_000);
    let mut array = EvmByteArray::memory();
    let word =
        EvmValue::from_numeric(Word::expensive(4096)).with_expression(ExprId::fresh().unwrap());
    array
        .write_word(&EvmValue::constant(U256::ZERO), &word, algebra.borrowed())
        .unwrap();
    let bound = array
        .summary_projection_with_payment(algebra.borrowed(), &mut setup)
        .unwrap();
    let data = bound
        .instantiate_with_payment(algebra.borrowed(), &mut setup)
        .unwrap();
    // This is the same paid owned copy that root delivery already retains.
    data.pay_clone_with(&mut setup).unwrap();
    let cached = data.clone();
    (algebra, bound, cached)
}

fn sources(
    bound: EvmByteArraySummary<Word>,
    data: EvmByteArray<Word>,
) -> Peekable<IntoIter<Data<Word>>> {
    let entry = Data::new(source(), bound, data, &mut WorkBudget::new(RETAIN_WORK)).unwrap();
    vec![entry].into_iter().peekable()
}

#[test]
fn owned_report_data_removes_native_replay_without_losing_private_bindings() {
    let _scope = RECEIPT.scope();
    let (algebra, bound, data) = opaque();
    let root = data.state_root().id();
    let mut baseline = WorkBudget::new(100_000_000);
    reset();
    let independently_replayed = bound
        .instantiate_with_payment(algebra.borrowed(), &mut baseline)
        .unwrap();
    let replay = RECEIPT.get();
    assert!(replay.clones > 0 && replay.queries > 0, "{replay:?}");
    assert_ne!(independently_replayed.state_root().id(), root);

    let mut sources = sources(bound, data);
    reset();
    let mut budget = WorkBudget::new(TAKE_WORK);
    let moved = take_data(&mut sources, source(), &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert_eq!(budget.used(), TAKE_WORK);
    assert!(baseline.used() > budget.used());
    assert_eq!(moved.state_root().id(), root);
    assert!(sources.next().is_none());
    for array in [&moved, &independently_replayed] {
        let word = array.read_word(&EvmValue::constant(U256::ZERO), algebra.borrowed());
        assert!(word.numeric().requires_retained_binding());
        assert!(word.contains(U256::from(8)));
        assert!(!word.contains(U256::from(7)));
    }
    eprintln!(
        "report replay control: fresh_work={} clones={} queries={} observations={}; moved_work={} clones=0 queries=0 observations=0",
        baseline.used(),
        replay.clones,
        replay.queries,
        replay.exports,
        budget.used()
    );
}

#[test]
fn changed_producer_revisions_and_namespaces_never_consume_another_report() {
    let _scope = RECEIPT.scope();
    let (_, bound, data) = opaque();
    let root = data.state_root().id();
    let mut sources = sources(bound, data);
    reset();
    let original = source();
    let different = [
        Source {
            namespace: 8,
            ..original
        },
        Source {
            procedure: 3,
            ..original
        },
        Source {
            entry_revision: 4,
            ..original
        },
        Source {
            kind: OutcomeKind::Revert,
            ..original
        },
        Source {
            generation: 5,
            ..original
        },
    ];
    for key in different {
        let mut budget = WorkBudget::new(TAKE_WORK);
        assert!(take_data(&mut sources, key, &mut budget).unwrap().is_none());
        assert_eq!(sources.peek().unwrap().data.state_root().id(), root);
    }
    let mut budget = WorkBudget::new(TAKE_WORK * 2);
    assert!(
        take_data(&mut sources, original, &mut budget)
            .unwrap()
            .is_some()
    );
    assert!(
        take_data(&mut sources, original, &mut budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(RECEIPT.get(), Receipt::default());
}

#[test]
fn report_move_refuses_before_consumption_and_keeps_both_cancellation_owners() {
    let _scope = RECEIPT.scope();
    let (_, bound, data) = opaque();
    reset();
    let mut retention = WorkBudget::new(RETAIN_WORK - 1);
    assert!(matches!(
        Data::new(source(), bound, data, &mut retention),
        Err(CloneError::Work)
    ));
    assert_eq!(retention.used(), 0);
    assert_eq!(RECEIPT.get(), Receipt::default());
    let (_, bound, data) = opaque();
    let mut sources = sources(bound, data);
    reset();
    let mut budget = WorkBudget::new(TAKE_WORK - 1);
    assert_eq!(
        take_data(&mut sources, source(), &mut budget),
        Err(CloneError::Work)
    );
    assert_eq!(budget.used(), 0);
    assert!(sources.peek().is_some());
    assert_eq!(
        take_data(&mut sources, source(), &mut budget),
        Err(CloneError::Work)
    );
    assert_eq!(RECEIPT.get(), Receipt::default());

    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(TAKE_WORK));
        if cancel_captured {
            captured.cancel();
        } else {
            ambient.cancel();
        }
        analysis_control::with_cancellation(&ambient, || {
            assert_eq!(
                take_data(&mut sources, source(), &mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_eq!(budget.used(), 0);
        assert!(sources.peek().is_some());
        assert_eq!(RECEIPT.get(), Receipt::default());
    }
}

#[test]
fn report_replay_tracks_actual_input_growth_and_separate_root_namespaces() {
    use crate::analysis::{
        ExecutionConfig,
        engine::Counters,
        machine::fixtures::{address, fixture, word},
        transfer::{
            StorageReadPolicy,
            procedure::testing::{Table, initial},
        },
    };
    let (world, entry) = fixture(&[(0x101, "60005460005260206000f3")]);
    let values = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    for use_summaries in [true, false] {
        let mut table = Table::default();
        let config = ExecutionConfig {
            use_summaries,
            root_terminal_coverage: false,
            max_work: 100_000_000,
            ..Default::default()
        };
        let mut counters = Counters::default();
        let mut old = None;
        let mut roots = Vec::new();
        for (source, value) in [5, 9].into_iter().enumerate() {
            let mut input = initial(&world, &entry, values.borrowed()).unwrap();
            input
                .store
                .write(address(0x101), &word(0), &word(value), values.borrowed());
            let (execution, record) = table.execute(
                source,
                &world,
                &entry,
                input,
                &config,
                values.borrowed(),
                &mut WorkBudget::new(config.max_work),
                &mut counters,
                StorageReadPolicy::Abstract,
            );
            assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
            assert!(record.closed);
            assert_eq!(record.procedures.len(), table.procedures.len());
            let root = record.root_procedure.unwrap();
            roots.push(root);
            let output = record.procedures[root]
                .outputs
                .iter()
                .find(|output| output.kind == OutcomeKind::Return)
                .unwrap();
            let returned = execution
                .outcomes
                .iter()
                .find(|output| output.kind == OutcomeKind::Return)
                .unwrap();
            assert_eq!(
                output.data.state_root().id(),
                returned.data.state_root().id(),
                "the read-only report must consume the already paid root materialization"
            );
            let read = output.data.read_word(&word(0), values.borrowed());
            assert!(read.contains(U256::from(value)));
            if let Some(old) = &old {
                let old: &EvmByteArray<crate::domain::NumericValue> = old;
                assert_eq!(
                    old.read_word(&word(0), values.borrowed()).singleton(),
                    Some(U256::from(5))
                );
                assert_ne!(old.state_root().id(), output.data.state_root().id());
            } else {
                old = Some(output.data.clone());
            }
        }
        assert_eq!(roots[0] == roots[1], use_summaries);
    }
}
