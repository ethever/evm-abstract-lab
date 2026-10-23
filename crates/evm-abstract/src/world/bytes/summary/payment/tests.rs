use super::*;
use crate::{
    U256,
    domain::EvmValue,
    resource::clone_payment::tests::{
        RECEIPT, Receipt, Word, assert_unobserved, private_value, reset,
    },
};
use std::{collections::BTreeMap, sync::Arc};

fn summary(price: usize) -> EvmByteArraySummary<Word> {
    EvmByteArraySummary {
        length: EvmValue::constant(U256::from(32)),
        memory: false,
        content: Content::Cells(BTreeMap::from([(0, private_value(price))])),
        words: BTreeMap::from([(0, private_value(price))]),
    }
}

#[test]
fn size_and_clone_quotation_never_discover_or_copy_numeric_values() {
    let _scope = RECEIPT.scope();
    let summary = summary(1_000_000);
    reset();
    let size = summary.work_size();
    assert_unobserved();
    let mut budget = WorkBudget::new(usize::MAX);
    let fee = summary.clone_work_with(&mut budget).unwrap();
    assert_unobserved();
    assert_eq!(budget.used(), summary.clone_prefix_work());
    assert!(fee > 2_000_000 && size > 0);
}

#[test]
fn finite_exact_clone_fee_and_one_less_preserve_native_predicates_and_shape() {
    let _scope = RECEIPT.scope();
    let summary = summary(1_000_000);
    let mut measure = WorkBudget::new(usize::MAX);
    summary.pay_clone_with(&mut measure).unwrap();
    let fee = measure.used();
    for limit in [summary.clone_prefix_work() - 1, fee - 1] {
        reset();
        let mut budget = WorkBudget::new(limit);
        assert_eq!(
            summary.clone_with_payment(&mut budget),
            Err(CloneError::Work)
        );
        assert_unobserved();
        assert_eq!(
            budget.used(),
            if limit < summary.clone_prefix_work() {
                0
            } else {
                summary.clone_prefix_work()
            }
        );
    }
    reset();
    let mut budget = WorkBudget::new(fee);
    let copy = summary.clone_with_payment(&mut budget).unwrap();
    assert_eq!(copy, summary);
    assert_eq!(budget.used(), fee);
    assert_eq!(RECEIPT.get().clones, 3);
    assert_eq!(RECEIPT.get().bytes, 8192);
    let Content::Cells(cells) = &copy.content else {
        panic!("cell shape")
    };
    let Content::Cells(original) = &summary.content else {
        panic!("cell shape")
    };
    assert!(cells[&0].same_identity(&original[&0]));
    let numeric = cells[&0].numeric();
    assert!(!numeric.contains(U256::from(7)));
    assert!(numeric.contains(U256::from(8)));
    assert!(numeric.observation().contains(U256::from(7)));
}

#[test]
fn max_and_aggregate_overflow_refuse_after_the_census_without_cloning() {
    let _scope = RECEIPT.scope();
    for price in [usize::MAX, usize::MAX / 2] {
        let summary = summary(price);
        reset();
        let mut budget = WorkBudget::new(usize::MAX);
        assert_eq!(
            summary.clone_with_payment(&mut budget),
            Err(CloneError::Work)
        );
        assert_unobserved();
        assert_eq!(budget.used(), summary.clone_prefix_work());
    }
}

#[test]
fn exact_buffers_are_shared_and_length_is_still_paid() {
    let _scope = RECEIPT.scope();
    let bytes: Arc<[u8]> = Arc::from([1, 0, 2]);
    let summary = EvmByteArraySummary::<Word> {
        length: EvmValue::constant(U256::from(3)),
        memory: false,
        content: Content::Exact(Arc::clone(&bytes)),
        words: BTreeMap::new(),
    };
    reset();
    let mut budget = WorkBudget::new(1_000);
    let copy = summary.clone_with_payment(&mut budget).unwrap();
    let Content::Exact(copied) = &copy.content else {
        panic!("exact shape")
    };
    assert!(Arc::ptr_eq(copied, &bytes));
    assert_eq!(RECEIPT.get().clones, 1);
    assert_eq!(RECEIPT.get().queries, 0);
    assert_eq!(copy, summary);
}

#[test]
fn either_cancellation_owner_refuses_before_the_first_price_or_copy_hook() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        let summary = summary(1_000_000);
        if cancel_captured {
            captured.cancel();
        } else {
            current.cancel();
        }
        reset();
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                summary.clone_with_payment(&mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn cancellation_from_a_clone_quote_keeps_only_the_census_payment() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        let mut summary = summary(1_000_000);
        let mut native = Word::expensive(1_000_000);
        native.cancel_price = Some(if cancel_captured {
            captured.clone()
        } else {
            current.clone()
        });
        summary.length = EvmValue::from_numeric(native);
        reset();
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                summary.clone_with_payment(&mut budget),
                Err(CloneError::Cancelled)
            );
        });
        assert_unobserved();
        assert_eq!(budget.used(), summary.clone_prefix_work());
    }
}
