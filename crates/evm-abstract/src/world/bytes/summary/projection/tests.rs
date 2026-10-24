use super::*;
use crate::{
    domain::{EvmValue, NumericValue, ProductAlgebra, ValueAlgebra},
    resource::CloneError,
};

#[test]
fn symbolic_exact_absence_pays_only_visited_prefix_and_keeps_full_numeric_fallback() {
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let array = EvmByteArray::<NumericValue>::unknown()
        .slice(
            &EvmValue::constant(U256::ZERO),
            &EvmValue::constant(U256::from(128)),
            domain.borrowed(),
        )
        .unwrap();
    let source = array.clone();
    let old = graph_work(array.root.work_size().saturating_add(2)).saturating_mul(129);
    let mut budget = WorkBudget::new(old - 1);
    assert_eq!(
        array
            .exact_projection_with_payment(Some(U256::from(128)), domain.borrowed(), &mut budget)
            .unwrap(),
        None
    );
    let fee = budget.used();
    assert!(fee < old);
    for (limit, succeeds) in [(fee, true), (fee - 1, false)] {
        let mut budget = WorkBudget::new(limit);
        let result = array.exact_projection_with_payment(
            Some(U256::from(128)),
            domain.borrowed(),
            &mut budget,
        );
        assert_eq!(result.is_ok(), succeeds);
        if !succeeds {
            assert!(matches!(
                result,
                Err(SummaryError::Budget(CloneError::Work))
            ));
        }
        assert!(budget.used() <= limit);
    }
    let expected = array.summary_projection(domain.borrowed()).unwrap();
    let mut budget = WorkBudget::new(10_000_000);
    let actual = array
        .summary_projection_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(array, source);
}

#[test]
fn fully_known_probe_keeps_the_original_complete_invoice_and_both_cancel_owners() {
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let bytes: Vec<_> = (0u8..64).collect();
    let array = EvmByteArray::<NumericValue>::exact(&bytes);
    let unit = graph_work(array.root.work_size().saturating_add(2));
    let fee = unit.saturating_mul(bytes.len() + 1);
    let mut budget = WorkBudget::new(fee);
    assert_eq!(
        array
            .exact_projection_with_payment(
                Some(U256::from(bytes.len())),
                domain.borrowed(),
                &mut budget
            )
            .unwrap(),
        Some(bytes)
    );
    assert_eq!(budget.used(), fee);
    let mut budget = WorkBudget::new(fee - 1);
    assert!(matches!(
        array.exact_projection_with_payment(Some(U256::from(64)), domain.borrowed(), &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    ));
    for captured in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(fee))
        } else {
            WorkBudget::new(fee)
        };
        token.cancel();
        let mut run = || {
            array.exact_projection_with_payment(
                Some(U256::from(64)),
                domain.borrowed(),
                &mut budget,
            )
        };
        let result = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&token, run)
        };
        assert!(matches!(
            result,
            Err(SummaryError::Budget(CloneError::Cancelled))
        ));
        assert_eq!(budget.used(), 0);
    }
}
