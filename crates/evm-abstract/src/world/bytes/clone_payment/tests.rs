use super::*;
use crate::{
    U256,
    domain::{
        EvmValue,
        symbolic::{ExprId, ExprLimits},
    },
    resource::clone_payment::tests::{
        RECEIPT, Receipt, Word, assert_unobserved, private_value, reset,
    },
};
use std::collections::BTreeMap;

fn array(work: usize) -> EvmByteArray<Word> {
    let mut array = EvmByteArray::memory();
    array.operands.insert(
        ExprId::fresh().unwrap(),
        private_value(work).with_symbol(crate::world::Symbol::CallValue, Some(138)),
    );
    array
}

#[test]
fn already_cancelled_current_or_captured_owner_pays_no_prefix_or_price_hook() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        let array = array(1_000_000);
        if cancel_captured {
            captured.cancel();
        } else {
            current.cancel();
        }
        reset();
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                array.pay_clone_with(&mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn exact_owned_array_fee_and_one_less_never_clone_while_quoting() {
    let _scope = RECEIPT.scope();
    let array = array(1_000_000);
    let old = array.clone();
    let value = array.operands.values().next().unwrap();
    let mut measure = WorkBudget::new(usize::MAX);
    reset();
    array.pay_clone_with(&mut measure).unwrap();
    assert_unobserved();
    let fee = measure.used();
    assert!(fee > 1_000_000 && array.work_size() < 10_000);
    for limit in [array.clone_prefix_work() - 1, fee - 1] {
        reset();
        let mut budget = WorkBudget::new(limit);
        assert_eq!(array.pay_clone_with(&mut budget), Err(CloneError::Work));
        assert_unobserved();
        assert_eq!(
            budget.used(),
            if limit < array.clone_prefix_work() {
                0
            } else {
                array.clone_prefix_work()
            }
        );
        assert_eq!(
            RECEIPT.get().private_quotes,
            usize::from(limit >= array.clone_prefix_work())
        );
        assert_eq!(array, old);
    }
    reset();
    let mut budget = WorkBudget::new(fee);
    array.pay_clone_with(&mut budget).unwrap();
    let copy = array.clone();
    assert_eq!(budget.used(), fee);
    assert_eq!(RECEIPT.get().bytes, 4096);
    assert_eq!(copy, old);
    assert!(copy.operands.values().next().unwrap().same_identity(value));
    assert_eq!(copy.state_root().id(), array.state_root().id());
    let numeric = copy.operands.values().next().unwrap().numeric();
    assert!(!numeric.contains(U256::from(7)));
    assert!(numeric.contains(U256::from(8)));
    assert!(numeric.observation().contains(U256::from(7)));
}

#[test]
fn max_and_aggregate_overflow_refuse_after_paid_prefix() {
    let _scope = RECEIPT.scope();
    for price in [usize::MAX, usize::MAX - 128] {
        let array = array(price);
        reset();
        let mut budget = WorkBudget::new(usize::MAX);
        assert_eq!(array.pay_clone_with(&mut budget), Err(CloneError::Work));
        assert_unobserved();
        assert_eq!(budget.used(), array.clone_prefix_work());
    }
}

#[test]
fn current_and_captured_owner_can_cancel_from_cached_clone_quote() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        let mut array = array(1_000_000);
        let mut word = Word::expensive(1_000_000);
        word.cancel_price = Some(if cancel_captured {
            captured.clone()
        } else {
            current.clone()
        });
        array
            .operands
            .insert(ExprId::fresh().unwrap(), EvmValue::from_numeric(word));
        reset();
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                array.pay_clone_with(&mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_unobserved();
        assert_eq!(budget.used(), array.clone_prefix_work());
    }
}

#[test]
fn conditional_children_share_private_values_without_a_native_quote_or_copy() {
    let _scope = RECEIPT.scope();
    let mut array = EvmByteArray::<Word>::memory();
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let left = BTreeMap::from([(key, private_value(usize::MAX))]);
    let empty = BTreeMap::new();
    let conditional = super::super::conditional::ConditionalFacts::default();
    array.conditional = super::super::conditional::ConditionalFacts::join(
        (&left, &conditional),
        (&empty, &conditional),
        &empty,
        guard,
        ExprLimits::default(),
    )
    .unwrap();
    reset();
    let mut budget = WorkBudget::new(1_000_000);
    array.pay_clone_with(&mut budget).unwrap();
    assert_unobserved();
    assert_eq!(RECEIPT.get().private_quotes, 0);
    let copy = array.clone();
    assert_eq!(RECEIPT.get().bytes, 0);
    assert_eq!(copy, array);
    assert!(budget.used() < 1_000_000);
    assert_eq!(copy.len().singleton(), Some(U256::ZERO));
}
