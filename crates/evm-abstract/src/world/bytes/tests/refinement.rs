//! Selected CALL-style output paths refine metadata, never historical roots.

use std::collections::BTreeSet;

use super::super::*;
use crate::domain::relational::{
    CheckResult, QueryReason, RelationLimits, RelationState, ScalarQuery, ValueQuery,
};

fn v(n: usize) -> AbstractValue {
    constant(n)
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
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
    .map_err(|error| match error {
        ExprError::Cancelled => QueryReason::Cancelled,
        ExprError::Limit { .. } => QueryReason::ExpressionLimit,
        error => panic!("ISZERO construction failed: {error:?}"),
    })?;
    match relations.local_value(&zero, &p) {
        ValueQuery::Exact(v) if v == U256::ZERO => Ok(Some(true)),
        ValueQuery::Exact(v) if v == U256::ONE => Ok(Some(false)),
        ValueQuery::Bounds(bounds) => {
            // ISZERO is intrinsically Boolean. Bounds preserve both truth
            // candidates; they do not claim two satisfying selector witnesses.
            assert!(bounds.contains(U256::ZERO));
            assert!(bounds.contains(U256::ONE));
            Ok(None)
        }
        ValueQuery::Undetermined(reason) => Err(reason),
        ValueQuery::ProvenImpossible => panic!("guard query requires a reachable relation state"),
        ValueQuery::Exact(value) => panic!("ISZERO produced a non-Boolean word: {value}"),
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

fn call_output_with_scalar_guards() -> (ByteArray, ExprId, ExprId, ExprId) {
    let (mut joined, byte_guard, _) = call_output();
    joined.refine_operands(&mut |_, value| value.forget_expression());
    let operand_guard = ExprId::fresh().unwrap();
    let nested_operand_guard = ExprId::fresh().unwrap();
    let operand = v(16)
        .join_guarded(
            &v(32),
            Domain::default(),
            nested_operand_guard.clone(),
            policy().expressions(),
        )
        .unwrap()
        .join_guarded(
            &v(64),
            Domain::default(),
            operand_guard.clone(),
            policy().expressions(),
        )
        .unwrap();
    // Retained scalar metadata has its own guards, outside the byte root.
    joined
        .operands
        .insert(operand.expression().unwrap().clone(), operand);
    (joined, byte_guard, operand_guard, nested_operand_guard)
}

#[test]
fn guard_collection_finds_byte_root_and_scalar_guards_without_changing_history() {
    let (joined, byte_guard, operand_guard, nested_operand_guard) =
        call_output_with_scalar_guards();
    let domain = Domain::default();
    let original = joined.clone();
    let root = joined.state_root().clone();
    let sources = joined.fresh_leaves();
    let historical = joined.read_word(&v(0x400), domain);
    let historical_expression = historical.expression().unwrap().clone();
    assert!(historical.contains(U256::ZERO));
    assert!(historical.contains(U256::from(512)));
    assert_ne!(byte_guard, operand_guard);
    assert_ne!(byte_guard, nested_operand_guard);
    assert_ne!(operand_guard, nested_operand_guard);
    let expected = BTreeSet::from([byte_guard.clone(), operand_guard, nested_operand_guard]);
    assert_eq!(
        joined.guard_conditions(policy().expressions()).unwrap(),
        expected
    );
    assert_eq!(joined, original);
    assert_eq!(joined.state_root(), &root);
    assert_eq!(joined.fresh_leaves(), sources);
    assert_eq!(joined.read_word(&v(0x400), domain), historical);
    assert_eq!(historical.expression(), Some(&historical_expression));

    // Dropping scalar expressions leaves the CALL guard in the immutable root.
    let mut root_only = joined.clone();
    root_only.update_values(&mut |value| value.forget_expression());
    for value in root_only
        .bytes
        .values()
        .chain(root_only.operands.values())
        .chain([&root_only.length, &root_only.default])
    {
        if let Some(expression) = value.expression() {
            assert_eq!(
                Some(expression.clone()),
                value.singleton().map(ExprId::constant),
                "root-only fixture must have no scalar guard expressions",
            );
        }
    }
    let root_only_original = root_only.clone();
    let root_only_sources = root_only.fresh_leaves();
    assert_eq!(
        root_only.guard_conditions(policy().expressions()).unwrap(),
        BTreeSet::from([byte_guard]),
    );
    assert_eq!(root_only, root_only_original);
    assert_eq!(root_only.state_root(), &root);
    assert_eq!(root_only.fresh_leaves(), root_only_sources);
    assert_eq!(root_only.read_word(&v(0x400), domain), historical);
    assert_eq!(joined, original);
}

#[test]
fn collected_serial_proofs_match_native_refinement_on_the_same_frozen_call_output() {
    let (frozen, byte_guard, operand_guard, nested_operand_guard) =
        call_output_with_scalar_guards();
    let domain = Domain::default();
    let historical = frozen.read_word(&v(0x400), domain);
    let expected_conditions = BTreeSet::from([
        byte_guard.clone(),
        operand_guard.clone(),
        nested_operand_guard.clone(),
    ]);
    let expected_order: Vec<_> = expected_conditions.iter().cloned().collect();
    for (selector_value, expected_truth, expected_word) in [
        (Some(0usize), Some(false), Some(U256::ZERO)),
        (Some(2usize), Some(true), Some(U256::from(512))),
        (None, None, None),
    ] {
        let mut relations = RelationState::default();
        if let Some(value) = selector_value {
            for condition in [&byte_guard, &operand_guard, &nested_operand_guard] {
                let equal = ExprId::operation(
                    opcode::EQ,
                    &[condition.clone(), ExprId::constant(U256::from(value))],
                    policy().expressions(),
                )
                .unwrap();
                assert_eq!(
                    relations.assume(&equal, true, &policy()),
                    CheckResult::MayBeFeasible
                );
            }
            assert_eq!(
                relations.local_value(&byte_guard, &policy()),
                ValueQuery::Exact(U256::from(value)),
            );
        }
        let mut prepared = frozen.clone();
        let conditions = prepared.guard_conditions(policy().expressions()).unwrap();
        assert_eq!(conditions, expected_conditions);
        assert_eq!(prepared, frozen);
        let mut proofs = BTreeMap::new();
        let mut proof_calls = Vec::new();
        for condition in conditions {
            proof_calls.push(condition.clone());
            proofs.insert(condition.clone(), prove(&relations, &condition).unwrap());
        }
        assert_eq!(proof_calls, expected_order);
        assert_eq!(
            proofs,
            BTreeMap::from([
                (byte_guard.clone(), expected_truth),
                (operand_guard.clone(), expected_truth),
                (nested_operand_guard.clone(), expected_truth),
            ]),
        );
        assert_eq!(prepared, frozen);
        let mut lookup_calls = Vec::new();
        let prepared_result = prepared
            .refine_guards(domain, policy().expressions(), &mut |condition| {
                lookup_calls.push(condition.clone());
                Ok::<_, QueryReason>(*proofs.get(condition).expect("collected guard proof"))
            })
            .unwrap();
        let mut direct = frozen.clone();
        let mut direct_calls = Vec::new();
        let direct_result = direct
            .refine_guards(domain, policy().expressions(), &mut |condition| {
                direct_calls.push(condition.clone());
                prove(&relations, condition)
            })
            .unwrap();
        assert_eq!(lookup_calls, expected_order);
        assert_eq!(direct_calls, expected_order);
        assert_eq!(prepared_result, direct_result);
        assert_eq!(prepared, direct);
        assert_eq!(prepared.state_root(), frozen.state_root());
        assert_eq!(prepared.fresh_leaves(), frozen.fresh_leaves());
        for index in [0, 0x400, 0x41f, 0x420, 0x4bf] {
            assert_eq!(
                prepared.byte_at(index, domain),
                direct.byte_at(index, domain)
            );
            assert_eq!(
                prepared.read_word(&v(index), domain),
                direct.read_word(&v(index), domain)
            );
        }
        let output = prepared.slice(&v(0x400), &v(192), 2048, domain).unwrap();
        assert_eq!(output.read_word(&v(0), domain).singleton(), expected_word);
        if let Some(expected) = expected_word {
            assert_eq!(
                output.exact_bytes().unwrap()[..32],
                expected.to_be_bytes::<32>()
            );
        } else {
            assert_eq!(prepared, frozen);
            assert_eq!(prepared.read_word(&v(0x400), domain), historical);
        }
        assert_eq!(frozen.read_word(&v(0x400), domain), historical);
    }

    // A later serial proof error must not publish the earlier wrapper fact.
    let mut failed = frozen.clone();
    let mut error_calls = Vec::new();
    assert_eq!(
        failed.refine_guards(domain, policy().expressions(), &mut |condition| {
            error_calls.push(condition.clone());
            if error_calls.len() == 2 {
                Err(QueryReason::ExpressionLimit)
            } else {
                Ok(Some(true))
            }
        }),
        Err(GuardRefinementError::Proof(QueryReason::ExpressionLimit)),
    );
    assert_eq!(error_calls, expected_order[..2]);
    assert_eq!(failed, frozen);
    assert_eq!(failed.read_word(&v(0x400), domain), historical);
}

#[test]
fn call_result_guard_derives_selector_truth_and_refines_the_full_output() {
    let (joined, selector, branch) = call_output();
    let domain = Domain::default();
    let before = joined.read_word(&v(0x400), domain);
    assert!(before.contains(U256::ZERO));
    assert!(before.contains(U256::from(512)));
    for (failure, expected) in [(false, 512), (true, 0)] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&branch, failure, &policy()),
            CheckResult::MayBeFeasible
        );
        // ISZERO(ITE(selector, 1, 0)) determines truth, not the
        // particular nonzero word used as a selector.
        if failure {
            assert_eq!(
                relations.local_value(&selector, &policy()),
                ValueQuery::Exact(U256::ZERO)
            );
        } else {
            let ValueQuery::Bounds(bounds) = relations.local_value(&selector, &policy()) else {
                panic!("the nonzero selector still includes different words");
            };
            assert!(bounds.contains(U256::ONE));
            assert!(bounds.contains(U256::from(2)));
            assert!(!bounds.contains(U256::ZERO));
        }
        assert_eq!(prove(&relations, &selector), Ok(Some(!failure)));
        assert_eq!(prove(&relations, &branch), Ok(Some(failure)));
        let mut derived = joined.clone();
        assert!(
            derived
                .refine_guards(domain, policy().expressions(), &mut |e| prove(
                    &relations, e
                ))
                .unwrap()
        );
        assert_eq!(derived.state_root(), joined.state_root());
        assert_eq!(derived.fresh_leaves(), joined.fresh_leaves());
        let output = derived.slice(&v(0x400), &v(192), 2048, domain).unwrap();
        assert_eq!(
            output.read_word(&v(0), domain).singleton(),
            Some(U256::from(expected))
        );
        assert_eq!(
            output.exact_bytes().unwrap()[..32],
            U256::from(expected).to_be_bytes::<32>()
        );

        // The explicit selector premise independently gives the same full
        // projection; it cannot make the already derived proof stronger.
        assert_eq!(
            relations.assume(&selector, !failure, &policy()),
            CheckResult::MayBeFeasible
        );
        let mut explicit = joined.clone();
        assert!(
            explicit
                .refine_guards(domain, policy().expressions(), &mut |e| prove(
                    &relations, e
                ))
                .unwrap()
        );
        assert_eq!(explicit, derived);
        assert!(before.singleton().is_none());
        assert_eq!(joined.read_word(&v(0x400), domain), before);
    }
}

#[test]
fn global_refinement_callbacks_do_not_reject_inactive_conditional_sources() {
    let domain = Domain::default();
    let limits = policy();
    let x = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let bounded = AbstractValue::unsigned_range(U256::ONE, U256::from(9))
        .unwrap()
        .with_expression(x.clone());
    let mut yes = ByteArray::memory();
    yes.write_word(&v(0), &bounded, 128, domain).unwrap();
    let mut no = ByteArray::memory();
    no.write_word(
        &v(0),
        &AbstractValue::top().with_expression(x.clone()),
        128,
        domain,
    )
    .unwrap();
    let joined = yes
        .join_guarded(&no, domain, guard.clone(), limits.expressions())
        .unwrap();
    let snapshot = joined.clone();
    let mut relations = RelationState::default();
    assert_eq!(
        relations.assume(&guard, false, &limits),
        CheckResult::MayBeFeasible
    );
    relations.observe(&x, v(20).numeric(), &limits).unwrap();
    assert_eq!(relations.check(&limits), CheckResult::MayBeFeasible);
    // This is the real caller's rejection condition. It is correct for a
    // conjunction, but this bound holds only on the inactive true branch.
    assert_eq!(
        relations.refine_numeric(&x, bounded.numeric(), domain, &limits),
        ScalarQuery::Infeasible
    );
    for keyed in [false, true] {
        let mut current = joined.clone();
        let mut failed = false;
        let mut visited_x = false;
        let mut refine = |expression: &ExprId, value: &mut AbstractValue| {
            if failed || value.singleton().is_some() || !relations.relevant(expression) {
                return;
            }
            visited_x |= expression == &x;
            match relations.refine_numeric(expression, value.numeric(), domain, &limits) {
                ScalarQuery::Refined { numeric, .. } => {
                    *value = value.clone().with_numeric(*numeric);
                }
                ScalarQuery::Infeasible => failed = true,
                ScalarQuery::Unchanged => {}
                other => panic!("unexpected global refinement result: {other:?}"),
            }
        };
        if keyed {
            current.refine_operands(&mut refine);
        } else {
            current.update_values(&mut |value| {
                if let Some(expression) = value.expression().cloned()
                    && !matches!(
                        expression.kind(),
                        crate::domain::symbolic::ExprKind::ByteRead { .. }
                            | crate::domain::symbolic::ExprKind::WordRead { .. }
                            | crate::domain::symbolic::ExprKind::Keccak { .. }
                    )
                {
                    refine(&expression, value);
                }
            });
        }
        assert!(visited_x, "the unconditional source must still be refined");
        assert!(!failed, "an inactive branch must not reject the wrapper");
        assert_eq!(
            current.operands.get(&x).unwrap().singleton(),
            Some(U256::from(20))
        );
        assert_eq!(current.conditional, joined.conditional);
        let mut retained_bound = false;
        current.conditional.visit(&mut |expression, value| {
            if expression == &x
                && let Some(value) = value
            {
                assert_eq!(value, &bounded);
                retained_bound = true;
            }
        });
        assert!(retained_bound, "the inactive source bound must survive");
        assert!(
            current
                .refine_guards(domain, limits.expressions(), &mut |condition| {
                    prove(&relations, condition)
                })
                .unwrap()
        );
        assert_eq!(
            current.operands.get(&guard).unwrap().singleton(),
            Some(U256::ZERO)
        );
        let read = current.read_word_checked(&v(0), domain).unwrap();
        assert_eq!(read.singleton(), Some(U256::from(20)));
        assert_eq!(read.expression(), Some(&x));
        assert_eq!(current.state_root(), joined.state_root());
        assert_eq!(current.conditional, joined.conditional);
    }
    assert_eq!(joined, snapshot);
}

#[test]
fn retained_operand_keys_refine_short_return_copy_and_preserve_its_suffix() {
    let domain = Domain::default();
    let returndata = ByteArray::unknown()
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    let length = returndata.len().expression().unwrap().clone();
    let mut memory = ByteArray::memory();
    let mut prior = vec![0xaa; 32];
    prior.extend([0xbb; 32]);
    memory
        .copy_from(
            &v(32),
            &ByteArray::exact(&prior),
            &v(0),
            &v(64),
            128,
            domain,
        )
        .unwrap();
    memory
        .copy_return_data(&v(32), &returndata, &v(16), 128, domain)
        .unwrap();
    let root = memory.state_root().clone();
    assert!(memory.byte_at(33, domain).singleton().is_none());
    // Optional value expressions may be dropped by a numerical projection;
    // retained operand keys still identify exactly the same source terms.
    memory.refine_operands(&mut |_, value| value.forget_expression());
    assert!(memory.operands.get(&length).unwrap().expression().is_none());
    let equal = ExprId::operation(
        opcode::EQ,
        &[length, ExprId::constant(U256::ONE)],
        policy().expressions(),
    )
    .unwrap();
    let mut relations = RelationState::default();
    assert_eq!(
        relations.assume(&equal, true, &policy()),
        CheckResult::MayBeFeasible
    );
    memory.refine_operands(&mut |expression, value| match relations.refine_numeric(
        expression,
        value.numeric(),
        domain,
        &policy(),
    ) {
        ScalarQuery::Refined { numeric, .. } => {
            *value = value.clone().with_numeric(*numeric);
        }
        ScalarQuery::Unchanged => {}
        result => panic!("reachable operand refinement expected: {result:?}"),
    });
    assert!(
        memory
            .refine_guards(domain, policy().expressions(), &mut |condition| prove(
                &relations, condition
            ))
            .unwrap()
    );
    assert_eq!(memory.state_root(), &root);
    for index in 33..64 {
        assert_eq!(
            memory.byte_at(index, domain).singleton(),
            Some(U256::from(0xaa))
        );
    }
    for index in 64..96 {
        assert_eq!(
            memory.byte_at(index, domain).singleton(),
            Some(U256::from(0xbb))
        );
    }
    assert_eq!(memory.len().singleton(), Some(U256::from(96)));
}

#[test]
fn unknown_error_cancel_and_tiny_budget_leave_guarded_history_unchanged() {
    let (mut joined, selector, _) = call_output();
    let original = joined.clone();
    let domain = Domain::default();
    assert_eq!(
        joined.guard_conditions(policy().expressions()).unwrap(),
        BTreeSet::from([selector.clone()]),
    );
    assert_eq!(joined, original);
    let mut calls = 0;
    assert!(
        joined
            .refine_guards(domain, policy().expressions(), &mut |_| {
                calls += 1;
                Ok::<_, QueryReason>(None)
            })
            .unwrap()
    );
    assert_eq!(calls, 1);
    assert_eq!(joined, original);
    calls = 0;
    assert_eq!(
        joined.refine_guards(domain, policy().expressions(), &mut |_| {
            calls += 1;
            Err(QueryReason::ExpressionLimit)
        }),
        Err(GuardRefinementError::Proof(QueryReason::ExpressionLimit)),
    );
    assert_eq!(calls, 1);
    assert_eq!(joined, original);
    let tiny = ExprLimits {
        max_nodes: 1,
        max_depth: 1,
    };
    assert!(joined.guard_conditions(tiny).is_err());
    assert_eq!(joined, original);
    calls = 0;
    assert!(
        joined
            .refine_guards(domain, tiny, &mut |_| {
                calls += 1;
                Ok::<_, QueryReason>(Some(true))
            })
            .is_err()
    );
    assert_eq!(calls, 0);
    assert_eq!(joined, original);
    let cancel = analysis_control::Cancellation::new();
    cancel.cancel();
    calls = 0;
    analysis_control::with_cancellation(&cancel, || {
        assert_eq!(
            prove(&RelationState::default(), &selector),
            Err(QueryReason::Cancelled)
        );
        assert!(joined.guard_conditions(policy().expressions()).is_err());
        assert_eq!(joined, original);
        assert!(
            joined
                .refine_guards(domain, policy().expressions(), &mut |_| {
                    calls += 1;
                    Ok::<_, QueryReason>(Some(true))
                })
                .is_err()
        );
    });
    assert_eq!(calls, 0);
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
    assert_eq!(
        relations.assume(&guard, true, &policy()),
        CheckResult::MayBeFeasible
    );
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
