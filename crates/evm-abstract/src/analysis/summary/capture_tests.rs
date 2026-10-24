//! Countercontrols for removing only a discarded absolute-payload copy.

use super::*;
use crate::{
    analysis::{EvmChildFrame, EvmContinuation, machine::fixtures, transfer},
    domain::{WordElement, symbolic::ExprId},
    resource::clone_payment::tests::{RECEIPT, Word, fixture, private_value, reset},
};
use std::convert::Infallible;

fn completed_call() -> EvmCompletedCall<Word> {
    let (world, entry, _, domain) = fixture("00");
    let mut payload = transfer::initial(&world, &entry, domain.borrowed()).unwrap();
    let value = private_value(500_000).with_expression(ExprId::fresh().unwrap());
    payload.active_mut().stack = vec![value.clone(), value.clone()];
    payload.active_mut().address_operand = Some(value.clone());
    payload.active_mut().key.jump_history = vec![3, 7, 11];
    payload.store.write_balance(entry.address, value.clone());
    payload.active_mut().saved_store = payload.store.snapshot();
    for index in 0..2 {
        let mut state = payload.active().clone();
        state.key.basic_block_index = index;
        payload.call_stack.push_child(EvmChildFrame {
            state,
            continuation: EvmContinuation {
                return_block: Some(index + 10),
                output_offset: value.clone(),
                output_size: value.clone(),
                creation: None,
            },
        });
    }
    let mut output_store = payload.store.clone();
    output_store.write_balance(entry.address, private_value(700_000));
    EvmCompletedCall {
        payload,
        kind: OutcomeKind::Revert,
        data: EvmByteArray::exact(&[7, 9]),
        output_store,
        output_kind: OutcomeKind::Failure,
        output_data: EvmByteArray::exact(&[11, 13, 17]),
    }
}

#[test]
fn relative_call_retains_full_private_values_sources_snapshots_and_all_six_fields() {
    let _scope = RECEIPT.scope();
    for prefix in 0..=2 {
        let source = completed_call();
        reset();
        let absolute = source.payload.clone();
        let discarded = RECEIPT.get();
        drop(absolute);
        reset();
        let mut reference = source.clone();
        reference.payload = relative(&reference.payload, prefix);
        let before = RECEIPT.get();
        reset();
        let actual = relative_completed_call(&source, prefix);
        let after = RECEIPT.get();
        assert_eq!(after.clones + discarded.clones, before.clones);
        assert_eq!(after.bytes + discarded.bytes, before.bytes);
        assert!(discarded.bytes >= 3 * 4096);
        assert_eq!(after.queries, 0);
        assert_eq!(after.views, 0);
        assert_eq!(after.exports, 0);
        assert_eq!(actual.payload, reference.payload);
        assert_eq!(actual.kind, reference.kind);
        assert_eq!(actual.data, reference.data);
        assert_eq!(actual.output_store, reference.output_store);
        assert_eq!(actual.output_kind, reference.output_kind);
        assert_eq!(actual.output_data, reference.output_data);
        assert_eq!(
            actual.payload.fresh_leaves(),
            reference.payload.fresh_leaves()
        );
        assert_eq!(actual.payload.call_stack.depth(), 3 - prefix);
        let original = source.payload.active();
        let retained = actual.payload.active();
        assert!(retained.stack[0].same_identity(&original.stack[0]));
        assert_eq!(
            retained.stack[0].expression(),
            original.stack[0].expression()
        );
        assert!(std::ptr::eq(
            retained.saved_store.state(),
            original.saved_store.state(),
        ));
        assert_eq!(retained.stack[0].numeric().private.len(), 4096);
        assert!(!retained.stack[0].numeric().contains(crate::U256::from(7)));
        let old = source.payload.active().stack[0].numeric().private.clone();
        drop(source);
        assert_eq!(actual.payload.active().stack[0].numeric().private, old);
        eprintln!(
            "SUMMARY_CALL_COPY prefix={prefix} before={before:?} after={after:?} removed={discarded:?}"
        );
    }
}

#[test]
fn actual_capture_keeps_exact_refusal_and_closed_call_evidence() {
    let (world, entry) = fixtures::fixture(&[(0x101, "5f5f5f5f5f61020061fffff100"), (0x200, "00")]);
    let graph = crate::analysis::testing::execute(
        world,
        entry,
        fixtures::config(),
        crate::analysis::testing::NumericLimits::machine().spec(),
    )
    .unwrap();
    let state = graph
        .states()
        .iter()
        .find(|state| state.entry.call_stack.depth() == 2)
        .unwrap();
    let id = state.id;
    let edges = graph.edges().iter().cloned().collect();
    let diagnostics = graph.diagnostics().iter().cloned().collect();
    let mut completed = vec![Vec::new(); graph.states().len()];
    completed[id].push(EvmCompletedCall {
        payload: state.exit.as_ref().unwrap().clone(),
        kind: OutcomeKind::Return,
        data: EvmByteArray::exact(&[3, 5]),
        output_store: state.entry.store.clone(),
        output_kind: OutcomeKind::Return,
        output_data: EvmByteArray::exact(&[7, 11]),
    });
    let mut setup = WorkBudget::new(100_000_000);
    let runtime = Runtime::<Infallible>::new(&mut setup).unwrap();
    let mut cache = EvmCache::<crate::domain::NumericValue>::new();
    let input = cache.input(&graph, id, &mut setup).unwrap().unwrap();
    let candidate = EvmCandidate { state: id, input };
    let partitions: Vec<Option<bool>> = vec![None; graph.states().len()];
    let mut paid = WorkBudget::new(100_000_000);
    let EvmCapture::Complete(certificate) = capture(
        &graph,
        &edges,
        &runtime,
        &diagnostics,
        &completed,
        &partitions,
        &candidate,
        &mut paid,
    ) else {
        panic!("ordinary closed child must capture")
    };
    assert_eq!(certificate.nodes[0].completed_calls.len(), 1);
    let call = &certificate.nodes[0].completed_calls[0];
    let expected = relative_completed_call(&completed[id][0], 1);
    assert_eq!(call.payload, expected.payload);
    assert_eq!(call.data, expected.data);
    assert_eq!(call.output_data, expected.output_data);
    assert_eq!(call.output_store, expected.output_store);
    assert_eq!(certificate.terminals.len(), 1);
    assert_eq!(
        certificate.terminals[0].output.data,
        completed[id][0].output_data
    );
    assert!(graph.states[id].execution_evidence().unwrap().is_current());
    let fee = paid.used();
    for (limit, succeeds) in [(fee - 1, false), (fee, true)] {
        let mut budget = WorkBudget::new(limit);
        let result = capture(
            &graph,
            &edges,
            &runtime,
            &diagnostics,
            &completed,
            &partitions,
            &candidate,
            &mut budget,
        );
        assert_eq!(matches!(result, EvmCapture::Complete(_)), succeeds);
        if !succeeds {
            assert!(matches!(
                result,
                EvmCapture::Stopped(FrontierReason::SummaryWork)
            ));
        }
    }
    eprintln!("SUMMARY_CAPTURE_EXACT fee={fee}");
}
