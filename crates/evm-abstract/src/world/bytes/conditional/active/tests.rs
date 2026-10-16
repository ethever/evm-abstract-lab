//! Inactive implications and interrupted projections cannot change a wrapper.

use crate::{
    analysis::FrontierReason,
    domain::{
        AbstractValue, Domain,
        relational::{QueryReason, RelationLimits, RelationState, ValueQuery},
        symbolic::{ByteStateKind, ExprError, ExprId, ExprLimits},
    },
    resource::WorkBudget,
    world::{ByteArray, GuardRefinementError, RangeError},
};
use alloy_primitives::U256;

fn v(n: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), FrontierReason> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(FrontierReason::Relations(QueryReason::Cancelled));
    }
    if !budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    Ok(())
}

fn copied(memory: &ByteArray, minimum_return: usize) -> (ByteArray, ExprId, ExprId) {
    let domain = Domain::default();
    let mut returndata = ByteArray::unknown();
    let returned_length = returndata.len().expression().unwrap().clone();
    returndata.length = returndata.length.clone().with_numeric(
        AbstractValue::unsigned_range(U256::from(minimum_return), U256::MAX)
            .unwrap()
            .numeric()
            .clone(),
    );
    let mut out = memory.clone();
    out.copy_return_data(&v(32), &returndata, &v(16), 128, domain)
        .unwrap();
    let ByteStateKind::Copy { length, .. } = out.state_root().kind() else {
        panic!("expected a retained symbolic minimum");
    };
    let minimum = length.clone();
    (out, minimum, returned_length)
}

#[test]
fn unknown_outer_scope_hides_even_a_known_inner_scope_with_conflicting_copy_fact() {
    let domain = Domain::default();
    let limits = RelationLimits::default();
    let before = ByteArray::memory();
    let (changed, minimum, returned_length) = copied(&before, 2);
    assert!(!changed.operands[&minimum].contains(U256::ONE));
    let inner = ExprId::fresh().unwrap();
    let outer = ExprId::fresh().unwrap();
    let mut joined = changed
        .join_guarded(&before, domain, inner.clone(), limits.expressions())
        .unwrap()
        .join_guarded(&before, domain, outer, limits.expressions())
        .unwrap();
    assert!(
        joined
            .refine_guards(domain, limits.expressions(), &mut |condition| {
                Ok::<_, FrontierReason>((condition == &inner).then_some(true))
            })
            .unwrap()
    );
    let original = joined.clone();
    let mut relations = RelationState::default();
    relations
        .observe(&returned_length, v(1).numeric(), &limits)
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        relations.local_value_budgeted(&minimum, &limits, &mut budget),
        ValueQuery::Exact(U256::ONE),
        "the hidden bound would contradict the same frozen global F",
    );
    let mut callbacks = 0;
    assert!(
        joined
            .refine_active_copy_lengths(
                limits.expressions(),
                &mut budget,
                reserve,
                &mut |_, _, _| {
                    callbacks += 1;
                    Ok(false)
                },
            )
            .unwrap()
    );
    assert_eq!(callbacks, 0);
    assert_eq!(joined, original);
}

#[derive(Clone, Copy)]
enum Stop {
    Error,
    Cancel,
    Work,
    Infeasible,
}

#[test]
fn copy_length_refinement_is_atomic_after_a_staged_update_and_on_preflight_failure() {
    let (once, _, _) = copied(&ByteArray::memory(), 0);
    let (original, _, _) = copied(&once, 0);
    let limits = ExprLimits::default();
    for stop in [Stop::Error, Stop::Cancel, Stop::Work, Stop::Infeasible] {
        let cancellation = analysis_control::Cancellation::new();
        let mut current = original.clone();
        let mut callbacks = 0;
        let result = analysis_control::with_cancellation(&cancellation, || {
            let mut budget = WorkBudget::new(1_000_000);
            current.refine_active_copy_lengths(
                limits,
                &mut budget,
                reserve,
                &mut |_, value, budget| {
                    callbacks += 1;
                    *value = value.clone().with_numeric(v(1).numeric().clone());
                    if callbacks == 1 {
                        return Ok(true);
                    }
                    match stop {
                        Stop::Error => Err(FrontierReason::Relations(QueryReason::ScalarFactLimit)),
                        Stop::Cancel => {
                            cancellation.cancel();
                            Ok(false)
                        }
                        Stop::Work => {
                            reserve(budget, usize::MAX)?;
                            unreachable!("the finite ledger must reject usize::MAX");
                        }
                        Stop::Infeasible => Ok(false),
                    }
                },
            )
        });
        match stop {
            Stop::Error => assert_eq!(
                result,
                Err(GuardRefinementError::Proof(FrontierReason::Relations(
                    QueryReason::ScalarFactLimit
                ),))
            ),
            Stop::Cancel => assert_eq!(
                result,
                Err(GuardRefinementError::Bytes(RangeError::Expression(
                    ExprError::Cancelled
                ),))
            ),
            Stop::Work => assert_eq!(
                result,
                Err(GuardRefinementError::Proof(FrontierReason::Work))
            ),
            Stop::Infeasible => assert_eq!(result, Ok(false)),
        }
        assert_eq!(callbacks, 2);
        assert_eq!(current, original);
    }
    let mut current = original.clone();
    let mut callbacks = 0;
    let mut update = |_: &ExprId, _: &mut AbstractValue, _: &mut WorkBudget| {
        callbacks += 1;
        Ok::<_, FrontierReason>(true)
    };
    assert_eq!(
        current.refine_active_copy_lengths(limits, &mut WorkBudget::new(0), reserve, &mut update),
        Err(GuardRefinementError::Proof(FrontierReason::Work)),
    );
    let tiny = ExprLimits {
        max_nodes: 1,
        max_depth: 1,
    };
    assert!(matches!(
        current.refine_active_copy_lengths(
            tiny,
            &mut WorkBudget::new(1_000_000),
            reserve,
            &mut update
        ),
        Err(GuardRefinementError::Bytes(RangeError::Expression(
            ExprError::Limit { .. }
        ))),
    ));
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            current.refine_active_copy_lengths(tiny, &mut WorkBudget::new(0), reserve, &mut update),
            Err(GuardRefinementError::Bytes(RangeError::Expression(
                ExprError::Cancelled
            ))),
        );
    });
    assert_eq!(callbacks, 0);
    assert_eq!(current, original);
}
