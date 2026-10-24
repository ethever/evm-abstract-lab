use super::*;
use crate::resource::clone_payment::tests::{
    RECEIPT, Receipt, Word, fixture as opaque_fixture, private_value, reset,
};

fn opaque_payload() -> EvmMachinePayload<Word> {
    let (world, entry, _, algebra) = opaque_fixture("00");
    let mut input = crate::analysis::transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let value = private_value(500_000);
    input.active_mut().stack = vec![value.clone(), value];
    input
        .store
        .write_balance(entry.address, private_value(500_000));
    input.active_mut().saved_store = input.store.snapshot();
    let child = input.active().clone();
    input.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(0),
            output_offset: private_value(500_000),
            output_size: private_value(500_000),
            creation: None,
        },
    });
    input
}

fn paid_copy<N: WordElement>(input: &EvmMachinePayload<N>) -> EvmMachinePayload<N> {
    input
        .pay_clone_with(&mut WorkBudget::new(usize::MAX))
        .unwrap();
    input.clone()
}

#[test]
fn owned_first_delivery_preserves_the_complete_payload_and_later_paid_copies() {
    let _scope = RECEIPT.scope();
    let input = opaque_payload();
    let retained = paid_copy(&input);
    let mut baseline = WorkBudget::new(usize::MAX);
    reset();
    retained.pay_clone_with(&mut baseline).unwrap();
    let original_copy = retained.clone();
    let copied = RECEIPT.get();
    assert!(copied.clones > 0 && copied.bytes >= 4096);

    let mut incoming = Some(input);
    let mut budget = WorkBudget::new(usize::MAX);
    reset();
    let mut first = take_root_payload(&mut incoming, &retained, &mut budget).unwrap();
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert_eq!(budget.used(), 0);
    assert!(incoming.is_none());
    assert_eq!(first, original_copy);
    assert_eq!(
        first.active().memory.state_root().id(),
        retained.active().memory.state_root().id()
    );
    assert_eq!(
        first.active().calldata.state_root().id(),
        retained.active().calldata.state_root().id()
    );
    assert!(first.active().stack[0].same_identity(&retained.active().stack[0]));
    assert!(first.active().stack[0].same_identity(&first.active().stack[1]));
    assert!(std::ptr::eq(
        first.active().saved_store.state(),
        retained.active().saved_store.state()
    ));
    assert!(!first.active().stack[0].contains(U256::from(7)));
    assert!(first.active().stack[0].contains(U256::from(8)));

    // Delivery mutation cannot alter the separately retained failure anchor.
    first.active_mut().stack.clear();
    first
        .store
        .write_balance(address(0x101), EvmValue::constant(U256::ZERO));
    reset();
    let second = take_root_payload(&mut incoming, &retained, &mut budget).unwrap();
    assert_eq!(budget.used(), baseline.used());
    assert_eq!(RECEIPT.get(), copied);
    assert_eq!(second, original_copy);
    assert_eq!(retained, original_copy);
    eprintln!(
        "root delivery control: original_copy_work={} native_clones={} private_bytes={}; first_move_work=0 native_clones=0; later_copy_work={}",
        baseline.used(),
        copied.clones,
        copied.bytes,
        budget.used()
    );
}

#[test]
fn root_delivery_refusal_keeps_the_failure_anchor_and_checks_both_cancellation_owners() {
    let _scope = RECEIPT.scope();
    let input = opaque_payload();
    let retained = paid_copy(&input);
    let original = paid_copy(&input);
    let mut measure = WorkBudget::new(usize::MAX);
    retained.pay_clone_with(&mut measure).unwrap();
    let mut incoming = Some(input);
    let mut failed = WorkBudget::new(0);
    assert!(!failed.charge(1));
    reset();
    assert_eq!(
        take_root_payload(&mut incoming, &retained, &mut failed),
        Err(MachineError::Work)
    );
    assert!(incoming.is_some());
    assert_eq!(RECEIPT.get(), Receipt::default());
    let _first =
        take_root_payload(&mut incoming, &retained, &mut WorkBudget::new(usize::MAX)).unwrap();
    reset();
    assert_eq!(
        take_root_payload(
            &mut incoming,
            &retained,
            &mut WorkBudget::new(measure.used() - 1)
        ),
        Err(MachineError::Work)
    );
    assert_eq!(RECEIPT.get().clones, 0);
    assert_eq!(retained, original);

    for captured_target in [true, false] {
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        let mut incoming = Some(paid_copy(&retained));
        if captured_target {
            captured.cancel();
        } else {
            ambient.cancel();
        }
        reset();
        let error = analysis_control::with_cancellation(&ambient, || {
            take_root_payload(&mut incoming, &retained, &mut budget)
        });
        assert_eq!(
            error,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert!(incoming.is_some());
        assert_eq!(budget.used(), 0);
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(retained, original);
    }
}

#[test]
fn later_root_delivery_checks_cancellation_during_native_quotation_before_clone() {
    let _scope = RECEIPT.scope();
    for captured_target in [true, false] {
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut retained = opaque_payload();
        let mut word = Word::expensive(500_000);
        word.cancel_price = Some(if captured_target {
            captured.clone()
        } else {
            ambient.clone()
        });
        retained.active_mut().stack[0] = EvmValue::from_numeric(word);
        let original = paid_copy(&retained);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        let error = analysis_control::with_cancellation(&ambient, || {
            take_root_payload(&mut None, &retained, &mut budget)
        });
        assert_eq!(
            error,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert!(RECEIPT.get().private_quotes > 0);
        assert_eq!(RECEIPT.get().clones, 0);
        assert_eq!(retained, original);
        reset();
    }
}

#[test]
fn actual_root_deliveries_leave_the_original_failure_payload_unchanged() {
    let (world, entry) = fixture(&[]);
    let algebra = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, algebra.borrowed())
            .unwrap();
    let original = paid_copy(&input);
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let (execution, record) = Table::default().execute(
        0,
        &world,
        &entry,
        input,
        &config,
        algebra.borrowed(),
        &mut budget,
        &mut Counters::default(),
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed);
    assert!(execution.frontiers.is_empty());
    assert_eq!(execution.payload, original);
    assert_eq!(
        execution
            .outcomes
            .iter()
            .map(|output| output.kind)
            .collect::<Vec<_>>(),
        vec![
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure
        ]
    );
}
