//! Common conjunctions stay unconditional; every residual uses the machine guard.

use super::super::{
    CheckResult, Constraint, QueryReason, RelationLimits, RelationPath, RelationPathKind,
    RelationState, ValueQuery,
};
use super::{c, limits, op};
use crate::domain::symbolic::{ByteStateId, ByteView, ExprId, SlotStateId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn state(atoms: impl IntoIterator<Item = Constraint>) -> RelationState {
    RelationState {
        constraints: atoms.into_iter().collect(),
        bottom: false,
        path: RelationPath::default(),
    }
}

fn equal(left: &ExprId, value: u64) -> Constraint {
    Constraint::Equal {
        left: left.clone(),
        right: c(value),
    }
}

#[test]
fn common_guarantees_preserve_word_memory_and_storage_selection_with_local_facts() {
    {
        let policy = RelationLimits { ..limits() };
        let bounds = policy.expressions();
        let guard = ExprId::fresh().unwrap();
        let common_word = ExprId::fresh().unwrap();
        let branch_word = ExprId::fresh().unwrap();
        let common = equal(&common_word, 5);
        let yes = state([common.clone(), equal(&branch_word, 7)]);
        let no = state([common.clone(), equal(&branch_word, 9)]);
        let joined = yes.join_guarded(&no, guard.clone(), &policy).unwrap();

        assert_eq!(joined.constraints().cloned().collect::<Vec<_>>(), [common]);
        assert_eq!(joined.len(), 3);
        assert!(matches!(
            joined.path().kind(),
            RelationPathKind::Ite { selector, .. } if *selector == guard
        ));
        assert_eq!(
            joined.local_value(&common_word, &policy),
            ValueQuery::Exact(U256::from(5)),
            "local: common guarantee"
        );
        super::bounds_include(joined.local_value(&branch_word, &policy), &[7, 9]);

        let memory = ByteStateId::zero()
            .write_word(c(0), c(7), bounds.into())
            .unwrap()
            .join_guarded(
                &ByteStateId::zero()
                    .write_word(c(0), c(9), bounds.into())
                    .unwrap(),
                guard.clone(),
                bounds.into(),
            )
            .unwrap();
        let memory_word =
            ExprId::word_read(ByteView::new(memory, c(0), c(32)), c(0), bounds).unwrap();
        let storage = SlotStateId::zero()
            .store(c(0), c(7), bounds.into())
            .unwrap()
            .join_guarded(
                &SlotStateId::zero()
                    .store(c(0), c(9), bounds.into())
                    .unwrap(),
                guard.clone(),
                bounds.into(),
            )
            .unwrap();
        let storage_word = ExprId::slot_read(storage, c(0), bounds).unwrap();
        let selected = ExprId::ite(guard.clone(), c(7), c(9), bounds).unwrap();
        let expressions = [selected, memory_word, storage_word];
        // An unknown selector retains the paired formulas and states. Numeric
        // projection uses common facts rather than proving their correlation.
        for expression in &expressions {
            assert!(expression.fresh_leaves().contains(&guard));
            super::bounds_include(joined.local_value(expression, &policy), &[7, 9]);
            super::bounds_include(
                joined.local_value(
                    &op(opcode::EQ, &[branch_word.clone(), expression.clone()]),
                    &policy,
                ),
                &[1],
            );
        }
        for (nonzero, expected) in [(true, 7), (false, 9)] {
            let mut selected = joined.clone();
            assert_eq!(
                selected.assume(&guard, nonzero, &policy),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                selected.local_value(&branch_word, &policy),
                ValueQuery::Exact(U256::from(expected)),
                "local: branch {nonzero}"
            );
            for expression in &expressions {
                assert_eq!(
                    selected.local_value(expression, &policy),
                    ValueQuery::Exact(U256::from(expected)),
                    "local: selected value/state {nonzero}"
                );
                assert_eq!(
                    selected.local_value(
                        &op(opcode::EQ, &[branch_word.clone(), expression.clone()]),
                        &policy
                    ),
                    ValueQuery::Exact(U256::ONE)
                );
            }
        }
    }
}

#[test]
fn disjoint_branch_guarantees_are_never_unconditional_with_local_facts() {
    {
        let policy = RelationLimits { ..limits() };
        let guard = ExprId::fresh().unwrap();
        let left = ExprId::fresh().unwrap();
        let right = ExprId::fresh().unwrap();
        let joined = state([equal(&left, 7)])
            .join_guarded(&state([equal(&right, 9)]), guard.clone(), &policy)
            .unwrap();
        assert_eq!(joined.constraints().count(), 0);
        for (nonzero, fixed, free, expected) in
            [(true, &left, &right, 7), (false, &right, &left, 9)]
        {
            let mut selected = joined.clone();
            assert_eq!(
                selected.assume(&guard, nonzero, &policy),
                CheckResult::MayBeFeasible
            );
            assert_eq!(
                selected.local_value(fixed, &policy),
                ValueQuery::Exact(U256::from(expected)),
                "local: selected branch guarantee"
            );
            super::bounds_include(selected.local_value(free, &policy), &[0, 17]);
        }
    }
}

#[test]
fn nested_conditional_guarantees_keep_their_original_selectors_with_local_facts() {
    {
        let policy = RelationLimits { ..limits() };
        let common_word = ExprId::fresh().unwrap();
        let branch_word = ExprId::fresh().unwrap();
        let inner_guard = ExprId::fresh().unwrap();
        let outer_guard = ExprId::fresh().unwrap();
        let common = equal(&common_word, 5);
        let inner = state([common.clone(), equal(&branch_word, 1)])
            .join_guarded(
                &state([common.clone(), equal(&branch_word, 2)]),
                inner_guard.clone(),
                &policy,
            )
            .unwrap();
        let joined = inner
            .join_guarded(
                &state([common.clone(), equal(&branch_word, 3)]),
                outer_guard.clone(),
                &policy,
            )
            .unwrap();
        assert_eq!(joined.constraints().cloned().collect::<Vec<_>>(), [common]);
        let expected = ExprId::ite(
            outer_guard.clone(),
            ExprId::ite(inner_guard.clone(), c(1), c(2), policy.expressions()).unwrap(),
            c(3),
            policy.expressions(),
        )
        .unwrap();
        let RelationPathKind::Ite {
            selector,
            then_path,
            ..
        } = joined.path().kind()
        else {
            panic!("outer selector was not retained");
        };
        assert_eq!(*selector, outer_guard);
        assert!(matches!(
            then_path.kind(),
            RelationPathKind::Ite { selector, .. } if *selector == inner_guard
        ));
        super::bounds_include(joined.local_value(&branch_word, &policy), &[1, 2, 3]);
        // Local projection does not discharge a whole-formula equality across
        // both unknown selectors; the selectors still retain every relation.
        let equality = op(opcode::EQ, &[branch_word.clone(), expected.clone()]);
        super::bounds_include(joined.local_value(&equality, &policy), &[1]);
        for candidate in 1..=3 {
            assert_eq!(
                joined.can_equal(&branch_word, U256::from(candidate), &policy),
                CheckResult::MayBeFeasible,
                "local: retained alternative {candidate}"
            );
        }
        assert_eq!(
            joined.can_equal(&branch_word, U256::from(4), &policy),
            CheckResult::MayBeFeasible,
            "unknown selectors do not supply a feasibility witness or a global exclusion"
        );
        for (outer, inner, value) in [(true, true, 1), (true, false, 2), (false, false, 3)] {
            let mut selected = joined.clone();
            assert_eq!(
                selected.assume(&outer_guard, outer, &policy),
                CheckResult::MayBeFeasible
            );
            if outer {
                assert_eq!(
                    selected.assume(&inner_guard, inner, &policy),
                    CheckResult::MayBeFeasible
                );
            }
            for expression in [&branch_word, &expected] {
                assert_eq!(
                    selected.local_value(expression, &policy),
                    ValueQuery::Exact(U256::from(value))
                );
            }
            assert_eq!(
                selected.local_value(&equality, &policy),
                ValueQuery::Exact(U256::ONE)
            );
            assert_eq!(
                selected.can_equal(&branch_word, U256::from(4), &policy),
                CheckResult::ProvenImpossible
            );
        }
    }
}

#[test]
fn common_constraints_count_once_and_still_obey_aggregate_limits() {
    let mut shared = ExprId::fresh().unwrap();
    for _ in 0..16 {
        shared = op(opcode::ADD, &[shared, c(1)]);
    }
    let branch = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let common = equal(&shared, 5);
    let yes = state([
        common.clone(),
        Constraint::Truth {
            expression: branch.clone(),
            nonzero: true,
        },
    ]);
    let no = state([
        common,
        Constraint::Truth {
            expression: branch,
            nonzero: false,
        },
    ]);
    let original_yes = yes.clone();
    let original_no = no.clone();
    let joined = yes.join_guarded(&no, guard.clone(), &limits()).unwrap();
    let unfactored = RelationPath::guarded(
        RelationPath::from_state(&yes),
        RelationPath::from_state(&no),
        guard.clone(),
        &limits(),
    )
    .unwrap();
    assert_eq!(joined.constraints().count(), 1);
    assert_eq!(joined.len(), 3);
    assert!(joined.work_size() < unfactored.nodes());
    let exact = RelationLimits {
        max_constraints: joined.len(),
        max_nodes: joined.work_size(),
        max_depth: shared.depth().max(joined.path().depth()),
        ..limits()
    };
    assert_eq!(
        yes.join_guarded(&no, guard.clone(), &exact).unwrap(),
        joined
    );
    assert_eq!(
        RelationPath::guarded(
            RelationPath::from_state(&yes),
            RelationPath::from_state(&no),
            guard.clone(),
            &exact,
        ),
        Err(QueryReason::ExpressionLimit)
    );
    for denied in [
        RelationLimits {
            max_nodes: exact.max_nodes - 1,
            ..exact
        },
        RelationLimits {
            max_depth: exact.max_depth - 1,
            ..exact
        },
    ] {
        assert_eq!(
            yes.join_guarded(&no, guard.clone(), &denied),
            Err(QueryReason::ExpressionLimit)
        );
    }
    assert_eq!(
        yes.join_guarded(
            &no,
            guard,
            &RelationLimits {
                max_constraints: exact.max_constraints - 1,
                ..exact
            },
        ),
        Err(QueryReason::ConstraintLimit)
    );
    assert_eq!(yes, original_yes);
    assert_eq!(no, original_no);
}

#[test]
fn factoring_preserves_bottom_alternatives_and_constant_selectors_with_local_facts() {
    {
        let policy = RelationLimits { ..limits() };
        let common_word = ExprId::fresh().unwrap();
        let branch_word = ExprId::fresh().unwrap();
        let guard = ExprId::fresh().unwrap();
        let mut impossible = state([equal(&common_word, 5), equal(&branch_word, 7)]);
        impossible.bottom = true;
        let possible = state([equal(&common_word, 5), equal(&branch_word, 9)]);
        for (yes, no, feasible_nonzero) in [
            (&impossible, &possible, false),
            (&possible, &impossible, true),
        ] {
            let joined = yes.join_guarded(no, guard.clone(), &policy).unwrap();
            assert!(!joined.is_bottom());
            assert_eq!(joined.check(&policy), CheckResult::MayBeFeasible);
            assert_eq!(
                joined.local_value(&branch_word, &policy),
                ValueQuery::Exact(U256::from(9))
            );
            let mut infeasible = joined.clone();
            assert_eq!(
                infeasible.assume(&guard, !feasible_nonzero, &policy),
                CheckResult::ProvenImpossible,
                "local: impossible branch stays false"
            );
        }
        let mut another_impossible = possible.clone();
        another_impossible.bottom = true;
        let both = impossible
            .join_guarded(&another_impossible, guard, &policy)
            .unwrap();
        assert!(both.is_bottom());
        assert_eq!(both.check(&policy), CheckResult::ProvenImpossible);
        assert_eq!(
            impossible.join_guarded(&possible, c(0), &policy).unwrap(),
            possible
        );
        assert_eq!(
            impossible.join_guarded(&possible, c(1), &policy).unwrap(),
            impossible
        );
    }
}
