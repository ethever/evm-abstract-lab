use super::*;
use crate::domain::summary::{covers_scalar, equal_scalar};

#[test]
fn summary_coverage_uses_private_top_and_equality_without_export_or_reconstruction() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let private = EvmValue::from_numeric(Word::private(64));
    let top = EvmValue::from_numeric(Word::top());
    assert_eq!(private.numeric().observation(), top.numeric().observation());
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(1_000_000);
    assert!(!covers_scalar(&private, &top, domain.borrowed(), &mut budget).unwrap());
    assert!(covers_scalar(&top, &private, domain.borrowed(), &mut budget).unwrap());
    assert!(covers_scalar(&private, &private, domain.borrowed(), &mut budget).unwrap());
    assert!(equal_scalar(&private, &private, domain.borrowed(), &mut budget).unwrap());
    let calls = RECEIPT.get();
    assert!(calls.top_queries > 0 && calls.equalities > 0);
    assert_eq!(
        calls.exports
            + calls.clones
            + calls.projections
            + calls.canonicals
            + calls.joins
            + calls.widenings
            + calls.constructions,
        0
    );
    assert!(!private.contains(U256::from(7)));
}

#[test]
fn summary_coverage_rejects_executable_metadata_even_when_persistent_eq_ignores_it() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let plain = EvmValue::from_numeric(Word::private(64));
    let runtime = plain.clone().with_identity(1, 2);
    assert_eq!(
        plain, runtime,
        "persistent identity Eq omits runtime metadata"
    );
    let mut sourced = plain.clone();
    sourced.set_origin(crate::domain::provenance::Origin::Storage);
    for value in [
        runtime,
        plain.clone().with_symbol(Symbol::Caller, Some(37)),
        plain.clone().with_expression(ExprId::fresh().unwrap()),
        plain.clone().with_symbolic_limit(),
        sourced,
    ] {
        RECEIPT.set(Receipt::default());
        assert!(
            !covers_scalar(
                &value,
                &plain,
                domain.borrowed(),
                &mut WorkBudget::new(10_000)
            )
            .unwrap()
        );
        assert!(
            !covers_scalar(
                &plain,
                &value,
                domain.borrowed(),
                &mut WorkBudget::new(10_000)
            )
            .unwrap()
        );
        assert_eq!(RECEIPT.get().top_queries + RECEIPT.get().equalities, 0);
    }
}

#[test]
fn summary_coverage_unavailable_query_is_unknown_without_sticky_exhaustion() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let private = EvmValue::from_numeric(Word::private(usize::MAX));
    let mut budget = WorkBudget::new(100_000);
    RECEIPT.set(Receipt::default());
    assert!(!covers_scalar(&private, &private, domain.borrowed(), &mut budget).unwrap());
    assert!(!budget.exhausted());
    assert_eq!(budget.used(), 16);
    assert_eq!(RECEIPT.get().top_queries + RECEIPT.get().equalities, 0);
    assert!(budget.charge(100_000 - 16));
}

#[test]
fn summary_coverage_cancellation_before_and_during_predicate_uses_both_owners() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for captured in [false, true] {
        for during in [false, true] {
            let cancellation = Cancellation::new();
            let mut budget = if captured {
                analysis_control::with_cancellation(&cancellation, || WorkBudget::new(100_000))
            } else {
                WorkBudget::new(100_000)
            };
            let value = EvmValue::from_numeric(Word {
                cancellation: Some(cancellation.clone()),
                cancel_at: if during {
                    CancelAt::Hook
                } else {
                    CancelAt::Never
                },
                ..Word::private(64)
            });
            if !during {
                cancellation.cancel();
            }
            RECEIPT.set(Receipt::default());
            let mut run = || covers_scalar(&value, &value, domain.borrowed(), &mut budget);
            let result = if captured {
                run()
            } else {
                analysis_control::with_cancellation(&cancellation, run)
            };
            assert_eq!(result, Err(CloneError::Cancelled));
            assert_eq!(RECEIPT.get().top_queries, usize::from(during));
            assert_eq!(RECEIPT.get().equalities, 0);
            assert_eq!(budget.used(), if during { 80 } else { 0 });
        }
    }
}
