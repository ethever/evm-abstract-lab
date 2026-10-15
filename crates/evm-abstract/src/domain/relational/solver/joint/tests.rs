//! Domain preparation and ledger boundaries; these cases require no native check.

use super::unique;
use crate::{
    domain::{
        relational::{BudgetedValueQuery, QueryReason, RelationLimits, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use embedded_smt::{Cancellation, QueryBudget, with_cancellation};

fn targets() -> [ExprId; 2] {
    [
        ExprId::constant(U256::from(1)),
        ExprId::constant(U256::from(2)),
    ]
}

#[test]
fn unpaid_reservation_wins_over_preparation_policy_errors() {
    let limits = RelationLimits {
        enabled: false,
        max_nodes: 0,
        ..RelationLimits::default()
    };
    let mut ledger = WorkBudget::new(0);
    assert_eq!(
        unique(
            &RelationState::default(),
            &targets(),
            &limits,
            &mut ledger.query_budget(11),
        ),
        Ok(vec![BudgetedValueQuery::Work; 2]),
    );
    assert_eq!(ledger.used(), 0);
    assert!(ledger.exhausted());
}

#[test]
fn combined_expression_limit_keeps_each_paid_preparation_charge() {
    let limits = RelationLimits {
        max_nodes: 2,
        ..RelationLimits::default()
    };
    let state = RelationState::default();
    let targets = targets();
    let mut probes = WorkBudget::new(100);
    for target in &targets {
        assert!(probes.charge(2));
        // Each old Fi fits two nodes: the true path and one word literal. A
        // maintenance refusal is a lookup miss, never a native uniqueness query.
        assert_eq!(
            state.probe_unique_value_cached(
                target,
                &limits,
                &mut probes
                    .smt_cache_query(limits.cache_scope())
                    .with_allowance(0),
            ),
            Ok(None),
        );
    }
    let mut ledger = WorkBudget::new(22);
    {
        let mut queries = ledger.query_budget(11);
        for _ in 0..2 {
            assert_eq!(
                state.unique_many_budgeted(&targets, &limits, &mut queries),
                Err(QueryReason::ExpressionLimit),
            );
        }
    }
    assert_eq!(ledger.used(), 22);
    assert!(!ledger.exhausted());
}

#[test]
fn duplicate_admission_cannot_reuse_an_unpaid_preparation_permit() {
    let target = ExprId::constant(U256::from(1));
    let mut ledger = WorkBudget::new(11);
    assert_eq!(
        unique(
            &RelationState::default(),
            &[target.clone(), target],
            &RelationLimits::default(),
            &mut ledger.query_budget(11),
        ),
        Err(QueryReason::Configuration),
    );
    assert_eq!(ledger.used(), 11);
    assert!(!ledger.exhausted());
}

#[test]
fn captured_cancel_is_unpaid_work_and_current_cancel_is_paid_query_failure() {
    let state = RelationState::default();
    let limits = RelationLimits::default();
    let targets = targets();
    let cancellation = Cancellation::new();
    cancellation.cancel();
    let (mut used, mut exhausted) = (0, false);
    {
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, cancellation.clone(), 11);
        assert_eq!(
            state.unique_many_budgeted(&targets, &limits, &mut budget),
            Ok(vec![BudgetedValueQuery::Work; 2]),
        );
    }
    assert_eq!(used, 0);
    assert!(exhausted);
    let (mut used, mut exhausted) = (0, false);
    {
        let mut budget = QueryBudget::new(100, &mut used, &mut exhausted, Cancellation::new(), 11);
        assert_eq!(
            with_cancellation(&cancellation, || {
                state.unique_many_budgeted(&targets, &limits, &mut budget)
            }),
            Err(QueryReason::Cancelled),
        );
    }
    assert_eq!(used, 11);
    assert!(!exhausted);
}
