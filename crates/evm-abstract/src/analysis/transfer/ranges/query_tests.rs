//! Paid queries, reusable answers and cancellation at logical range boundaries.

use super::{
    Condition, RangeQueries, constrained, guard, literal_memory_range,
    literal_memory_range_with_queries,
};
use crate::{
    Fork, U256,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{
            self,
            query::tests::{Policy, RECEIPT, Word, context},
        },
    },
    domain::{
        EvmValue, SingletonProof, ValueAlgebra, WordElement, relational::QueryReason,
        symbolic::ExprId,
    },
    resource::WorkBudget,
    world::{EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;

#[test]
fn literal_range_refuses_before_query_and_keeps_every_successful_prefix() {
    let _receipt = RECEIPT.scope();
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let offset = EvmValue::constant(U256::ZERO);
    let size = EvmValue::constant(U256::from(7));
    for (fee, maximum) in [(100_003, 17 + 128 + 100_002), (usize::MAX, usize::MAX)] {
        RECEIPT.update(|receipt| receipt.fee = fee);
        let mut budget = WorkBudget::new(maximum);
        assert!(budget.charge(17));
        assert_eq!(
            literal_memory_range(
                &offset,
                &size,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Work)
        );
        assert_eq!(budget.used(), 17 + 128);
        assert_eq!(RECEIPT.get().queries, 0);
    }
    assert_eq!(RECEIPT.get().quotes, 2);
    RECEIPT.update(|receipt| receipt.fee = 100_003);
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        literal_memory_range(
            &offset,
            &size,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(Some(true))
    );
    assert_eq!(budget.used(), 128 + 100_003 + 128 + 64 + 128);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));
}

#[test]
fn empty_literal_range_never_quotes_or_discovers_offset() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let offset = EvmValue::constant(U256::from(7));
    let size = EvmValue::constant(U256::ZERO);
    let mut budget = WorkBudget::new(128 + 64);
    assert_eq!(
        literal_memory_range(
            &offset,
            &size,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(Some(true))
    );
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
        (0, 0, 192)
    );
}

#[test]
fn range_query_checks_current_and_original_ledger_before_and_after_discovery() {
    let _receipt = RECEIPT.scope();
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    for cancel_owner in [false, true] {
        for stage in 0..3 {
            let owner = analysis_control::Cancellation::new();
            let active = analysis_control::Cancellation::new();
            let mut budget =
                analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
            assert!(budget.charge(17));
            let offset = EvmValue::constant(U256::ZERO);
            let size = EvmValue::from_numeric(Word {
                cancel_query_owner: (cancel_owner && stage == 2).then(|| owner.clone()),
                cancel_quote_owner: (cancel_owner && stage == 1).then(|| owner.clone()),
                ..Word::constant(U256::from(7))
            });
            RECEIPT.update(|receipt| {
                receipt.quotes = 0;
                receipt.queries = 0;
                receipt.cancel_on_quote = !cancel_owner && stage == 1;
                receipt.cancel_on_query = !cancel_owner && stage == 2;
            });
            // Quote cancellation uses the active scope; both ledger identities
            // are separately exercised before and during actual discovery.
            if stage == 0 {
                if cancel_owner {
                    owner.cancel();
                } else {
                    active.cancel();
                }
            }
            analysis_control::with_cancellation(&active, || {
                assert_eq!(
                    literal_memory_range(
                        &offset,
                        &size,
                        &mut context(&config, &mut budget, algebra.borrowed())
                    ),
                    Err(FrontierReason::Relations(QueryReason::Cancelled))
                );
            });
            assert_eq!(
                budget.used(),
                17 + if stage == 0 {
                    0
                } else if stage == 1 {
                    128
                } else {
                    128 + 61
                }
            );
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (usize::from(stage != 0), usize::from(stage == 2))
            );
        }
    }
}

#[test]
fn paid_answers_survive_literal_fallback_and_guard_reuses_operation_result_proof() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = 100_003;
    });
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let offset = EvmValue::<Word>::top().with_expression(ExprId::fresh().unwrap());
    let size = EvmValue::<Word>::top();
    let mut budget = WorkBudget::new(10_000_000);
    let mut queries = RangeQueries::default();
    assert_eq!(
        literal_memory_range_with_queries(
            &offset,
            &size,
            &mut queries,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(None)
    );
    assert_eq!(queries.size, SingletonProof::Answer(None));
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    let built = guard::build_after_queries(
        &offset,
        &size,
        &mut queries,
        &mut context(&config, &mut budget, algebra.borrowed()),
    )
    .unwrap();
    // Offset already has a valid expression; size's paid negative answer is
    // reusable after attaching its fresh term. No detached condition fee exists.
    assert_eq!(RECEIPT.get().quotes, 1);
    assert_eq!(built.singleton, SingletonProof::Answer(None));
    assert!(built.condition.expression().is_some());
    let monitored = built.condition.clone().with_numeric(Word {
        query_tag: Some(99),
        ..built.condition.numeric().clone()
    });
    // Solver operations on new numeric elements have their own complete fees.
    // Watch only the unchanged condition consumed by these two branches.
    RECEIPT.update(|receipt| {
        receipt.query_tag = Some(99);
        receipt.quotes = 0;
        receipt.queries = 0;
        receipt.fee = usize::MAX;
    });
    let condition = Condition::prepare(&monitored, built.singleton, &mut budget).unwrap();
    assert!(condition.truth.zero && condition.truth.nonzero);
    assert_eq!(RECEIPT.get().queries, 0);

    let world = EvmWorld::<Word>::new(Fork::Osaka, "condition consumer");
    let entry = EvmEntry::concrete(
        Address::ZERO,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let queries_before = RECEIPT.get().queries;
    for nonzero in [true, false] {
        let branch = constrained(
            &payload,
            condition.value,
            condition.truth,
            nonzero,
            Some(&built.retention),
            &mut context(&config, &mut budget, algebra.borrowed()),
        )
        .unwrap_or_else(|_| panic!("paid Boolean branch must retain its assumption"));
        assert!(branch.payload.is_some());
    }
    assert_eq!(RECEIPT.get().queries, queries_before);
    assert_eq!(RECEIPT.get().quotes, 0);
}

#[test]
fn already_known_size_is_reused_with_no_new_fee_and_exact_bounds_coverage() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    for (offset, allowed) in [
        (U256::ZERO, true),
        (super::logical::MAX_END, false),
        (U256::MAX, false),
    ] {
        let mut budget = WorkBudget::new(1_000_000);
        let mut queries = RangeQueries {
            size: SingletonProof::Answer(Some(U256::from(7))),
            offset: SingletonProof::NotQueried,
        };
        assert_eq!(
            literal_memory_range_with_queries(
                &EvmValue::constant(offset),
                &EvmValue::constant(U256::from(7)),
                &mut queries,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Ok(Some(allowed))
        );
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
}

#[test]
fn guard_refuses_unknown_literal_work_and_discards_cancelled_operation_results() {
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let offset = EvmValue::<Word>::top().with_expression(ExprId::fresh().unwrap());
    let size = EvmValue::<Word>::top().with_expression(ExprId::fresh().unwrap());
    let _receipt = RECEIPT.scope();
    let _literals = transfer::query::tests::LITERALS.scope();
    RECEIPT.update(|receipt| receipt.literal_fee = usize::MAX);
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(matches!(
        guard::build(
            &offset,
            &size,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    ));
    assert_eq!(transfer::query::tests::LITERALS.get(), 0);
    assert!(budget.used() > 0 && budget.used() < usize::MAX);
    RECEIPT.update(|receipt| {
        receipt.literal_fee = 64;
        receipt.cancel_on_apply = true;
    });
    let owner = analysis_control::Cancellation::new();
    let active = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
    analysis_control::with_cancellation(&active, || {
        assert!(matches!(
            guard::build(
                &offset,
                &size,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        ));
    });
    assert!(!owner.is_cancelled() && active.is_cancelled());
    assert!(budget.used() > 128);
    assert_eq!(
        (RECEIPT.get().quotes, transfer::query::tests::LITERALS.get()),
        (0, 2)
    );
}

#[test]
fn boolean_membership_preserves_opaque_exclusions_without_singleton_discovery() {
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let world = EvmWorld::<Word>::new(Fork::Osaka, "Boolean membership");
    let entry = EvmEntry::concrete(
        Address::ZERO,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let value = EvmValue::from_numeric(Word {
        upper: U256::ONE,
        excluded: Some(U256::ZERO),
        ..Word::top()
    });
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let mut refused = WorkBudget::new(127);
    assert!(matches!(
        Condition::prepare(&value, SingletonProof::NotQueried, &mut refused),
        Err(FrontierReason::Work)
    ));
    assert_eq!(refused.used(), 0);
    let mut budget = WorkBudget::new(128);
    let condition = Condition::prepare(&value, SingletonProof::Answer(None), &mut budget).unwrap();
    assert!(!condition.truth.zero && condition.truth.nonzero);
    for nonzero in [false, true] {
        let branch = constrained(
            &payload,
            condition.value,
            condition.truth,
            nonzero,
            None,
            &mut context(&config, &mut budget, algebra.borrowed()),
        )
        .unwrap_or_else(|_| panic!("Boolean proof settles the branch"));
        assert_eq!(branch.payload.is_some(), nonzero);
    }
    assert_eq!(
        (budget.used(), RECEIPT.get().quotes, RECEIPT.get().queries),
        (128, 0, 0)
    );
}

#[test]
fn actual_range_answers_fund_retained_metadata_and_obey_both_cancellation_owners() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|r| {
        r.watched = Some(U256::from(32));
        r.fee = 100_003;
    });
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let offset = EvmValue::<Word>::constant(U256::ZERO).with_expression(ExprId::fresh().unwrap());
    let size = EvmValue::<Word>::constant(U256::from(32)).with_expression(ExprId::fresh().unwrap());
    let world = EvmWorld::new(Fork::Osaka, "actual range certificate ownership");
    let entry = EvmEntry::concrete(
        Address::ZERO,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut result = transfer::empty_execution(payload);
    let mut admission_budget = WorkBudget::new(config.max_work);
    let queries = super::memory_with_queries(
        &mut result,
        &offset,
        &size,
        &mut context(&config, &mut admission_budget, algebra.borrowed()),
        0,
    )
    .unwrap();
    assert_eq!(queries.offset, SingletonProof::Answer(Some(U256::ZERO)));
    assert_eq!(queries.size, SingletonProof::Answer(Some(U256::from(32))));
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    let work = [&offset, &size]
        .iter()
        .map(|value| value.clone_work() + value.expression().unwrap().work_size() + 2)
        .sum::<usize>();
    for (maximum, cancellation) in [(work - 1, 0), (work, 0), (work, 1), (work, 2)] {
        let owner = analysis_control::Cancellation::new();
        let active = analysis_control::Cancellation::new();
        let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(maximum));
        if cancellation == 1 {
            owner.cancel();
        }
        if cancellation == 2 {
            active.cancel();
        }
        let prepared = analysis_control::with_cancellation(&active, || {
            queries.prepare(&offset, &size, &mut budget)
        });
        if cancellation != 0 {
            assert_eq!(
                prepared.unwrap_err(),
                FrontierReason::Relations(QueryReason::Cancelled)
            );
            assert_eq!(budget.used(), 0);
        } else if maximum < work {
            assert_eq!(prepared.unwrap_err(), FrontierReason::Work);
            assert_eq!(budget.used(), 0);
        } else {
            let (prepared_offset, prepared_size) = prepared.unwrap();
            assert_eq!(budget.used(), work);
            for (prepared, original, word) in [
                (&prepared_offset, &offset, U256::ZERO),
                (&prepared_size, &size, U256::from(32)),
            ] {
                assert_eq!(prepared.numeric(), original.numeric());
                assert_eq!(
                    prepared.expression().and_then(ExprId::as_constant),
                    Some(word)
                );
            }
        }
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    }
    // Paid negative answers and unqueried operands retain their exact metadata.
    let unknown = EvmValue::from_numeric(Word {
        lower: U256::ZERO,
        upper: U256::from(32),
        excluded: Some(U256::from(7)),
        ..Word::top()
    })
    .with_expression(ExprId::fresh().unwrap());
    let queries = RangeQueries {
        offset: SingletonProof::NotQueried,
        size: SingletonProof::Answer(None),
    };
    let mut budget = WorkBudget::new(offset.clone_work() + unknown.clone_work());
    let (unchanged_offset, unchanged_size) =
        queries.prepare(&offset, &unknown, &mut budget).unwrap();
    assert_eq!(unchanged_offset, offset);
    assert_eq!(unchanged_size, unknown);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
}
