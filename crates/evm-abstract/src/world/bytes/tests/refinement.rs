//! Selected CALL-style output paths refine metadata, never historical roots.

use super::super::*;
use crate::domain::relational::{
    CheckResult, QueryReason, RelationLimits, RelationState, ValueQuery,
};

fn v(n: usize) -> AbstractValue {
    constant(n)
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    }
}
fn prove(relations: &RelationState, condition: &ExprId) -> Result<Option<bool>, QueryReason> {
    let p = policy();
    let zero = ExprId::operation(
        opcode::ISZERO,
        std::slice::from_ref(condition),
        p.expressions(),
    )
    .unwrap();
    match relations.unique_value(&zero, &p) {
        ValueQuery::Unique(v) if v == U256::ZERO => Ok(Some(true)),
        ValueQuery::Unique(v) if v == U256::ONE => Ok(Some(false)),
        ValueQuery::Multiple => Ok(None),
        ValueQuery::Unknown(reason) => Err(reason),
        result => panic!("reachable Boolean proof expected: {result:?}"),
    }
}
fn call_output() -> (ByteArray, ExprId, ExprId) {
    let domain = Domain::default();
    let mut failed = ByteArray::memory();
    failed.expand(&v(0x400), &v(192), 2048, domain).unwrap();
    let mut returned = vec![0; 192];
    returned[..32].copy_from_slice(&U256::from(512).to_be_bytes::<32>());
    let mut success = failed.clone();
    success
        .copy_return_data(
            &v(0x400),
            &ByteArray::exact(&returned),
            &v(192),
            2048,
            domain,
        )
        .unwrap();
    let selector = ExprId::fresh().unwrap();
    let joined = success
        .join_guarded(&failed, domain, selector.clone(), policy().expressions())
        .unwrap();
    let result = v(1)
        .join_guarded(&v(0), domain, selector.clone(), policy().expressions())
        .unwrap();
    let branch = ExprId::operation(
        opcode::ISZERO,
        &[result.expression().unwrap().clone()],
        policy().expressions(),
    )
    .unwrap();
    (joined, selector, branch)
}

#[test]
fn native_success_and_failure_guard_refine_call_output_without_rewriting_history() {
    let (joined, selector, branch) = call_output();
    let domain = Domain::default();
    let before = joined.read_word(&v(0x400), domain);
    assert!(before.contains(U256::ZERO));
    assert!(before.contains(U256::from(512)));
    for (failure, expected) in [(false, 512), (true, 0)] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&branch, failure, &policy()),
            CheckResult::Sat
        );
        if !failure {
            assert_eq!(
                relations.unique_value(&selector, &policy()),
                ValueQuery::Multiple
            );
        }
        let mut selected = joined.clone();
        assert!(
            selected
                .refine_guards(domain, policy().expressions(), &mut |e| prove(
                    &relations, e
                ))
                .unwrap()
        );
        assert_eq!(selected.state_root(), joined.state_root());
        assert_eq!(selected.fresh_leaves(), joined.fresh_leaves());
        let output = selected.slice(&v(0x400), &v(192), 2048, domain).unwrap();
        assert_eq!(
            output.read_word(&v(0), domain).singleton(),
            Some(U256::from(expected))
        );
        assert_eq!(
            output.exact_bytes().unwrap()[..32],
            U256::from(expected).to_be_bytes::<32>()
        );
        assert!(before.singleton().is_none());
        assert_eq!(joined.read_word(&v(0x400), domain), before);
    }
}

#[test]
fn unknown_error_cancel_and_tiny_budget_leave_guarded_history_unchanged() {
    let (mut joined, _, _) = call_output();
    let original = joined.clone();
    let domain = Domain::default();
    assert!(
        joined
            .refine_guards(
                domain,
                policy().expressions(),
                &mut |_| Ok::<_, QueryReason>(None)
            )
            .unwrap()
    );
    assert_eq!(joined, original);
    assert_eq!(
        joined.refine_guards(domain, policy().expressions(), &mut |_| Err(
            QueryReason::ResourceLimit
        )),
        Err(GuardRefinementError::Proof(QueryReason::ResourceLimit)),
    );
    assert_eq!(joined, original);
    assert!(
        joined
            .refine_guards(
                domain,
                ExprLimits {
                    max_nodes: 1,
                    max_depth: 1
                },
                &mut |_| Ok::<_, QueryReason>(Some(true))
            )
            .is_err()
    );
    assert_eq!(joined, original);
    let cancel = embedded_smt::Cancellation::new();
    cancel.cancel();
    embedded_smt::with_cancellation(&cancel, || {
        assert!(
            joined
                .refine_guards(
                    domain,
                    policy().expressions(),
                    &mut |_| Ok::<_, QueryReason>(Some(true))
                )
                .is_err()
        );
    });
    assert_eq!(joined, original);
}

#[test]
fn guard_projection_for_large_lazy_copy_visits_metadata_only_and_renames_facts() {
    let domain = Domain::default();
    let size = 1024 * 1024;
    let source = ByteArray::unknown();
    let mut changed = ByteArray::memory();
    changed
        .copy_from(&v(0), &source, &v(0), &v(size), size, domain)
        .unwrap();
    let guard = ExprId::fresh().unwrap();
    let mut joined = changed
        .join_guarded(
            &ByteArray::memory(),
            domain,
            guard.clone(),
            policy().expressions(),
        )
        .unwrap();
    let mut calls = 0;
    let mut relations = RelationState::default();
    assert_eq!(relations.assume(&guard, true, &policy()), CheckResult::Sat);
    assert!(
        joined
            .refine_guards(domain, policy().expressions(), &mut |e| {
                calls += 1;
                prove(&relations, e)
            })
            .unwrap()
    );
    assert_eq!(calls, 1);
    assert!(joined.stored_bytes().next().is_none());
    assert_eq!(joined.len().singleton(), Some(U256::from(size)));
    assert!(joined.work_size() < 1000);
    let replacement = ExprId::fresh().unwrap();
    joined
        .rename_fresh(
            &BTreeMap::from([(guard, replacement.clone())]),
            policy().expressions(),
        )
        .unwrap();
    assert!(
        joined
            .symbolic_facts()
            .any(|(e, v)| e == &replacement && !v.may_be_zero())
    );
}
