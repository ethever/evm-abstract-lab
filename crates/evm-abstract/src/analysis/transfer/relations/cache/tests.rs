//! Exact reuse remains scoped, bounded and independent of numeric candidates.

use super::{MAX_ENTRIES, QueryCache};
use crate::{
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{TransferContext, WorkBudget},
    },
    domain::{
        AbstractValue, Domain, NumericValue,
        relational::{
            CheckResult, QueryReason, RelationLimits, RelationState, SmtProvider, ValueQuery,
        },
        symbolic::{ExprId, SlotStateId, SourceId, StateBinding},
    },
};
use alloy_primitives::U256;

fn read(source: SourceId) -> ExprId {
    let state = SlotStateId::observed(source, Vec::new(), Default::default()).unwrap();
    ExprId::slot_read(state, ExprId::constant(U256::ZERO), Default::default()).unwrap()
}

fn observe(expression: &ExprId, value: u64, limits: &RelationLimits) -> RelationState {
    let mut state = RelationState::default();
    state
        .observe(
            expression,
            &NumericValue::constant(U256::from(value)),
            limits,
        )
        .unwrap();
    state
}

#[test]
fn complete_snapshot_and_policy_changes_invalidate_answers() {
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let state = RelationState::default();
    let changed_constraints = observe(&expression, 7, &limits);
    let changed_path = changed_constraints
        .join_guarded(
            &observe(&expression, 9, &limits),
            ExprId::fresh().unwrap(),
            &limits,
        )
        .unwrap();
    assert_eq!(changed_path.constraints().count(), 0);
    assert_ne!(changed_path.path(), state.path());
    let mut bottom = state.clone();
    assert_eq!(
        bottom.assume(&ExprId::constant(U256::ZERO), true, &limits),
        CheckResult::Unsat
    );
    for changed in [&changed_constraints, &changed_path, &bottom] {
        let mut cache = QueryCache::default();
        cache.remember(&state, &expression, &limits, &ValueQuery::Multiple);
        assert_eq!(
            cache.get(&state.clone(), &expression.clone(), &limits),
            Some(ValueQuery::Multiple)
        );
        assert_eq!(cache.get(changed, &expression, &limits), None);
        assert!(cache.values.is_empty());
    }
    for changed in [
        RelationLimits {
            enabled: false,
            ..limits
        },
        RelationLimits {
            max_constraints: limits.max_constraints + 1,
            ..limits
        },
        RelationLimits {
            max_nodes: limits.max_nodes + 1,
            ..limits
        },
        RelationLimits {
            max_depth: limits.max_depth + 1,
            ..limits
        },
        RelationLimits {
            provider: SmtProvider::Bitwuzla,
            ..limits
        },
        RelationLimits {
            rlimit: limits.rlimit + 1,
            ..limits
        },
    ] {
        let mut cache = QueryCache::default();
        cache.remember(&state, &expression, &limits, &ValueQuery::Multiple);
        assert_eq!(cache.get(&state, &expression, &changed), None);
    }
}

#[test]
fn structurally_equal_expressions_reuse_only_the_same_source_identity() {
    let limits = RelationLimits::default();
    let relations = RelationState::default();
    let token = ExprId::fresh().unwrap();
    let source = SourceId::scoped(token.clone(), U256::ZERO);
    let expression = read(source.clone());
    let mut cache = QueryCache::default();
    cache.remember(&relations, &expression, &limits, &ValueQuery::Multiple);
    assert_eq!(
        cache.get(&relations, &read(source), &limits),
        Some(ValueQuery::Multiple)
    );
    for source in [
        SourceId::fresh().unwrap(),
        SourceId::scoped(token, U256::ONE),
    ] {
        assert_eq!(cache.get(&relations, &read(source), &limits), None);
    }
}

#[test]
fn unknown_answers_are_never_retained() {
    let limits = RelationLimits::default();
    let state = RelationState::default();
    let expression = ExprId::fresh().unwrap();
    let mut cache = QueryCache::default();
    for reason in [
        QueryReason::ResourceLimit,
        QueryReason::Cancelled,
        QueryReason::ExpressionLimit,
    ] {
        cache.remember(&state, &expression, &limits, &ValueQuery::Unknown(reason));
        assert_eq!(cache.get(&state, &expression, &limits), None);
        assert!(cache.snapshot.is_none());
        assert_eq!(cache.retained_work, 0);
    }
    for answer in [
        ValueQuery::Unique(U256::from(7)),
        ValueQuery::Multiple,
        ValueQuery::Infeasible,
    ] {
        let mut cache = QueryCache::default();
        cache.remember(&state, &expression, &limits, &answer);
        assert_eq!(cache.get(&state, &expression, &limits), Some(answer));
    }
}

#[test]
fn entry_and_retained_work_bounds_evict_only_optional_proofs() {
    let state = RelationState::default();
    for (limits, count) in [
        (RelationLimits::default(), MAX_ENTRIES + 1),
        (
            RelationLimits {
                max_nodes: 6,
                ..RelationLimits::default()
            },
            3,
        ),
    ] {
        let mut cache = QueryCache::default();
        let expressions: Vec<_> = (0..count).map(|_| ExprId::fresh().unwrap()).collect();
        for expression in &expressions {
            cache.remember(&state, expression, &limits, &ValueQuery::Multiple);
            assert!(cache.values.len() <= MAX_ENTRIES);
            assert!(cache.retained_work <= limits.max_nodes);
        }
        assert_eq!(cache.get(&state, &expressions[0], &limits), None);
        assert_eq!(
            cache.get(&state, expressions.last().unwrap(), &limits),
            Some(ValueQuery::Multiple)
        );
        // A missed proof is obtained from the original query again, then fits
        // exactly the same bounded cache without changing the query policy.
        let answer = state.unique_value(&expressions[0], &limits);
        assert_eq!(answer, ValueQuery::Multiple);
        cache.remember(&state, &expressions[0], &limits, &answer);
        assert_eq!(cache.get(&state, &expressions[0], &limits), Some(answer));
    }
    let mut cache = QueryCache::default();
    let expression = ExprId::fresh().unwrap();
    let too_small = RelationLimits {
        max_nodes: 2,
        ..RelationLimits::default()
    };
    cache.remember(&state, &expression, &too_small, &ValueQuery::Multiple);
    assert!(cache.snapshot.is_none());
    assert_eq!(cache.get(&state, &expression, &too_small), None);
}

#[test]
fn repeated_refinement_reuses_native_unique_and_multiple_proofs_for_every_provider() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let mut config = ExecutionConfig::default();
        config.analysis.relations.provider = provider;
        let unique_state = SlotStateId::observed(
            SourceId::fresh().unwrap(),
            vec![StateBinding {
                index: U256::ZERO,
                value: ExprId::constant(U256::from(7)),
                numeric: NumericValue::constant(U256::from(7)),
            }],
            Default::default(),
        )
        .unwrap();
        let unique = ExprId::slot_read(
            unique_state,
            ExprId::constant(U256::ZERO),
            Default::default(),
        )
        .unwrap();
        let multiple = read(SourceId::fresh().unwrap());
        for (expression, expected) in [(unique, Some(U256::from(7))), (multiple, None)] {
            let allowance = config.analysis.relations.rlimit as usize;
            // The first optional mathematical-cache maintenance can spend at
            // most half an allowance. This still cannot pay four native
            // allowances, so the second call needs the original transfer cache.
            let mut budget = WorkBudget::new(3 * allowance + allowance / 2 - 1);
            let mut context = TransferContext {
                config: &config,
                domain: Domain::default(),
                budget: &mut budget,
                root_address: None,
                input_scope: None,
                query_cache: Default::default(),
                procedure: false,
                discover_code: false,
            };
            let mut first = AbstractValue::top().with_expression(expression.clone());
            assert_eq!(
                super::super::refine(&mut first, &RelationState::default(), &mut context),
                Ok(true),
                "{provider:?}"
            );
            assert_eq!(first.singleton(), expected);
            let after_first = context.budget.used();
            assert!(after_first >= 2 * allowance);
            assert_eq!(
                context.query_cache.get(
                    &RelationState::default(),
                    &expression,
                    &config.analysis.relations
                ),
                Some(expected.map_or(ValueQuery::Multiple, ValueQuery::Unique)),
                "the original transfer cache retained the real solver proof"
            );
            let mathematical_hits = context.budget.smt_cache_stats().unique_hits;
            let mut second = AbstractValue::top().with_expression(expression);
            assert_eq!(
                super::super::refine(&mut second, &RelationState::default(), &mut context),
                Ok(true),
                "{provider:?}"
            );
            assert_eq!(second, first);
            assert!(context.budget.used() - after_first < allowance);
            assert_eq!(
                context.budget.smt_cache_stats().unique_hits,
                mathematical_hits,
                "the transfer cache answered before the mathematical/native query path"
            );
            assert!(!context.budget.exhausted());
        }
    }
}

#[test]
fn cached_multiple_still_filters_changed_finite_candidates_for_every_provider() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let mut config = ExecutionConfig::default();
        config.analysis.relations.provider = provider;
        let policy = config.analysis.relations;
        let expression = ExprId::fresh().unwrap();
        let relations = observe(&expression, 2, &policy)
            .join_guarded(
                &observe(&expression, 3, &policy),
                ExprId::fresh().unwrap(),
                &policy,
            )
            .unwrap();
        let allowance = policy.rlimit as usize;
        let domain = Domain::default();
        // Unique maintenance <= R/2; two candidate maintenance steps <= R/2.
        // The total still cannot pay the six mandatory native allowances that
        // would be needed without the original transfer-level Multiple proof.
        let mut budget = WorkBudget::new(6 * allowance - 1);
        let mut context = TransferContext {
            config: &config,
            domain,
            budget: &mut budget,
            root_address: None,
            input_scope: None,
            query_cache: Default::default(),
            procedure: false,
            discover_code: false,
        };
        let mut first = AbstractValue::top().with_expression(expression.clone());
        assert_eq!(
            super::super::refine(&mut first, &relations, &mut context),
            Ok(true),
            "{provider:?}"
        );
        assert_eq!(first.singleton(), None);
        assert_eq!(
            context.query_cache.get(&relations, &expression, &policy),
            Some(ValueQuery::Multiple)
        );
        let mut changed = domain
            .join(
                &AbstractValue::constant(U256::ZERO),
                &AbstractValue::constant(U256::ONE),
            )
            .with_expression(expression);
        assert_eq!(
            super::super::refine(&mut changed, &relations, &mut context),
            Ok(false),
            "{provider:?}: both current candidates are impossible"
        );
        assert!(!context.budget.exhausted());
        assert!(context.budget.used() >= 4 * allowance);
    }
}

#[test]
fn a_cached_proof_cannot_bypass_cancelled_or_exhausted_work() {
    for cancelled in [false, true] {
        let control = crate::analysis::control::Control::new(Default::default());
        control.scope(|| {
            let config = ExecutionConfig::default();
            let relations = RelationState::default();
            let expression = read(SourceId::fresh().unwrap());
            let mut cache = QueryCache::default();
            cache.remember(
                &relations,
                &expression,
                &config.analysis.relations,
                &ValueQuery::Multiple,
            );
            let mut budget = WorkBudget::new(if cancelled { config.max_work } else { 0 });
            if cancelled {
                control.cancellation().cancel();
            }
            let mut context = TransferContext {
                config: &config,
                domain: Domain::default(),
                budget: &mut budget,
                root_address: None,
                input_scope: None,
                query_cache: cache,
                procedure: false,
                discover_code: false,
            };
            let mut value = AbstractValue::top().with_expression(expression);
            let before = value.clone();
            assert_eq!(
                super::super::refine(&mut value, &relations, &mut context),
                Err(FrontierReason::Work)
            );
            assert_eq!(value, before);
            assert!(context.budget.exhausted());
            assert_eq!(context.budget.used(), 0);
        });
    }
}
