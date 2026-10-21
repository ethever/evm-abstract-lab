//! Actual completion snapshots keep their native copies behind declared prices.

use super::*;
use crate::{
    analysis::{EvmContinuation, EvmMachinePayload, transfer::empty_execution},
    domain::{EvmValue, WordElement},
    resource::clone_payment::tests::{
        Policy, RECEIPT, Word, assert_unobserved, fixture, private_value, reset,
    },
    world::{EvmByteArray, EvmSnapshot},
};

type Payload = EvmMachinePayload<Word>;
type Bytes = EvmByteArray<Word>;
type Value = EvmValue<Word>;

fn literal(value: u64) -> Value {
    Value::constant(U256::from(value)).with_expression(ExprId::constant(U256::from(value)))
}

fn array(value: &Value, policy: &ValueAlgebra<Policy>) -> Bytes {
    let mut bytes = Bytes::memory();
    bytes
        .write_word(&literal(0), value, policy.borrowed())
        .unwrap();
    bytes
}

fn child(config: &ExecutionConfig, policy: &ValueAlgebra<Policy>) -> Payload {
    let (world, entry, _, _) = fixture("00");
    let mut payload = initial(&world, &entry, policy.borrowed()).unwrap();
    let nested = payload
        .call_stack
        .root()
        .clone()
        .into_child(EvmContinuation {
            return_block: Some(0),
            output_offset: literal(0),
            output_size: literal(0),
            creation: None,
        });
    payload.call_stack.push_child(nested);
    assert!(payload.call_stack.depth() < config.max_call_depth);
    payload
}

fn context<'a>(
    config: &'a ExecutionConfig,
    policy: &'a ValueAlgebra<Policy>,
    budget: &'a mut WorkBudget,
) -> TransferContext<'a, Policy> {
    crate::analysis::transfer::testing::context(config, budget, policy.borrowed())
}

#[test]
fn max_native_copy_price_refuses_actual_precreation_payload_and_data_captures() {
    let _scope = RECEIPT.scope();
    for location in ["parent memory", "returned data", "live store"] {
        let (_, _, config, policy) = fixture("00");
        let mut payload = child(&config, &policy);
        let expensive = private_value(usize::MAX).with_expression(ExprId::fresh().unwrap());
        let mut data = Bytes::empty();
        match location {
            "parent memory" => {
                let nested = payload.call_stack.pop_child().unwrap();
                payload.active_mut().memory = array(&expensive, &policy);
                payload.call_stack.push_child(nested);
            }
            "returned data" => data = array(&expensive, &policy),
            "live store" => payload
                .store
                .write_balance(Address::repeat_byte(0x55), expensive),
            _ => unreachable!(),
        }
        let expected = payload.clone();
        let mut result = empty_execution(payload.clone());
        let mut budget = WorkBudget::new(usize::MAX);
        reset();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            data,
            &mut context(&config, &policy, &mut budget),
            0,
        );
        assert_unobserved();
        assert_eq!(RECEIPT.get().private_quotes, 1, "{location}");
        assert!(budget.used() > 0);
        assert_eq!(result.payload, expected);
        assert!(result.completed_calls.is_empty() && result.successors.is_empty());
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    }
}

#[test]
fn rollback_prices_the_actual_saved_store_without_recursing_through_checkpoint_clones() {
    let _scope = RECEIPT.scope();
    let (_, _, config, policy) = fixture("00");
    let mut payload = child(&config, &policy);
    let mut saved = payload.store.clone();
    saved.write_balance(Address::repeat_byte(0x55), private_value(usize::MAX));
    payload
        .call_stack
        .children_mut()
        .last_mut()
        .unwrap()
        .state
        .saved_store = EvmSnapshot::from_state(saved);
    let expected = payload.clone();
    let mut result = empty_execution(payload.clone());
    let mut budget = WorkBudget::new(usize::MAX);
    reset();
    finish(
        &mut result,
        payload,
        OutcomeKind::Revert,
        Bytes::empty(),
        &mut context(&config, &policy, &mut budget),
        0,
    );
    let receipt = RECEIPT.get();
    assert_eq!(
        (
            receipt.bytes,
            receipt.private_quotes,
            receipt.views,
            receipt.queries,
            receipt.exports
        ),
        (0, 1, 0, 0, 0)
    );
    assert!(
        receipt.clones > 0,
        "the cheap pre-creation capture was already paid"
    );
    assert_eq!(result.payload, expected);
    assert!(result.completed_calls.is_empty() && result.successors.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn actual_finish_price_hook_cancellation_keeps_both_owners_and_all_native_snapshots_private() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let (_, _, config, policy) = fixture("00");
        let mut payload = child(&config, &policy);
        let mut word = Word::expensive(100_003);
        word.cancel_price = Some(if cancel_captured {
            captured.clone()
        } else {
            current.clone()
        });
        payload
            .store
            .write_balance(Address::repeat_byte(0x55), Value::from_numeric(word));
        let expected = payload.clone();
        let mut result = empty_execution(payload.clone());
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        reset();
        RECEIPT.update(|receipt| receipt.armed = true);
        analysis_control::with_cancellation(&current, || {
            finish(
                &mut result,
                payload,
                OutcomeKind::Return,
                Bytes::empty(),
                &mut context(&config, &policy, &mut budget),
                0,
            );
        });
        assert_unobserved();
        assert_eq!(captured.is_cancelled(), cancel_captured);
        assert_eq!(current.is_cancelled(), !cancel_captured);
        assert_eq!(result.payload, expected);
        assert!(result.completed_calls.is_empty() && result.successors.is_empty());
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
        );
    }
}

#[test]
fn finite_declared_snapshot_price_preserves_raw_completed_payload_output_and_atomic_refusal() {
    let _scope = RECEIPT.scope();
    let (_, _, config, policy) = fixture("00");
    let mut payload = child(&config, &policy);
    let expensive = private_value(100_003).with_expression(ExprId::fresh().unwrap());
    payload
        .store
        .write_balance(Address::repeat_byte(0x55), expensive);
    let original = payload.clone();
    let data = Bytes::exact(&[9]);
    let mut pilot_result = empty_execution(payload.clone());
    let mut pilot_budget = WorkBudget::new(usize::MAX);
    reset();
    finish(
        &mut pilot_result,
        payload.clone(),
        OutcomeKind::Return,
        data.clone(),
        &mut context(&config, &policy, &mut pilot_budget),
        0,
    );
    assert!(pilot_result.frontiers.is_empty());
    let total = pilot_budget.used();
    let continuation = &payload.call_stack.active_child().unwrap().continuation;
    let final_copy = payload
        .call_stack
        .parent()
        .unwrap()
        .memory
        .copy_return_data_work(
            &data,
            &continuation.output_offset,
            &continuation.output_size,
            policy.borrowed(),
        )
        + continuation.output_offset.clone_work()
        + continuation.output_size.clone_work();
    for maximum in [0, total - 1, total] {
        let mut result = empty_execution(payload.clone());
        let working = payload.clone();
        let returned = data.clone();
        let mut budget = WorkBudget::new(maximum);
        reset();
        finish(
            &mut result,
            working,
            OutcomeKind::Return,
            returned,
            &mut context(&config, &policy, &mut budget),
            0,
        );
        if maximum == total {
            assert_eq!(budget.used(), total);
            assert!(result.frontiers.is_empty());
            assert_eq!(
                (result.completed_calls.len(), result.successors.len()),
                (1, 1)
            );
            let completed = &result.completed_calls[0];
            assert_eq!(completed.payload, original);
            assert_eq!(completed.kind, OutcomeKind::Return);
            assert_eq!(completed.data, data);
            assert_eq!(completed.output_store, original.store);
            assert_eq!(completed.output_data, data);
            // Persistent map snapshots share their entries; std snapshots own
            // two predicate copies. Both must preserve the complete store.
            assert_eq!(
                RECEIPT.get().bytes,
                if cfg!(feature = "imbl") { 0 } else { 2 * 4096 }
            );
            assert!(budget.used() >= 2 * 100_003);
            // Inspect the retained predicate after the measured clone receipt.
            let balance = completed
                .output_store
                .read_balance(Address::repeat_byte(0x55));
            assert!(!balance.numeric().contains(U256::from(7)));
            assert!(balance.numeric().contains(U256::from(8)));
        } else {
            assert_eq!(
                budget.used(),
                if maximum == 0 { 0 } else { total - final_copy }
            );
            assert!(result.completed_calls.is_empty() && result.successors.is_empty());
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        }
        assert_eq!(result.payload, original);
    }
}
