use super::super::{Branch, ConditionalFacts, Context, Facts, Scope};
use super::{Actions, shared_branches};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeMap, convert::Infallible, sync::Arc};
use {
    crate::domain::identity::Symbol, crate::domain::provenance::Origin,
    crate::domain::symbolic::ByteView, crate::domain::symbolic::ExprError,
    crate::domain::symbolic::ExprId, crate::domain::symbolic::ExprKind,
    crate::domain::symbolic::ExprLimits, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestBytes, crate::world::test_support::TestValue,
};

fn word(value: u64) -> TestValue {
    TestValue::constant(U256::from(value))
}

fn constant(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn limits() -> ExprLimits {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .spec()
        .relations()
        .expressions()
}

fn empty() -> Scope {
    Scope {
        facts: Facts::new(),
        conditional: ConditionalFacts::default(),
    }
}

fn fact(key: &ExprId, value: TestValue) -> Scope {
    Scope {
        facts: Facts::from([(key.clone(), value)]),
        ..empty()
    }
}

fn ite(condition: &ExprId, yes: &Scope, no: &Scope) -> Scope {
    Scope {
        facts: Facts::new(),
        conditional: ConditionalFacts::from_branches(vec![Arc::new(Branch {
            condition: condition.clone(),
            then_facts: yes.clone(),
            else_facts: no.clone(),
        })]),
    }
}

fn joined(original: &Scope, policy: ExprLimits) -> Result<ConditionalFacts, ExprError> {
    let branch = &original.conditional.branches[0];
    ConditionalFacts::join(
        (&branch.then_facts.facts, &branch.then_facts.conditional),
        (&branch.else_facts.facts, &branch.else_facts.conditional),
        &Facts::new(),
        branch.condition.clone(),
        policy,
    )
}

fn observed(number: u64, input: u64, origin: Origin) -> TestValue {
    let mut value = TestValue::top()
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(input))
        .with_numeric(word(number).numeric().clone());
    value.set_origin(origin);
    value
}

fn evaluate(expression: &ExprId, guards: &BTreeMap<ExprId, U256>) -> U256 {
    if let Some(value) = guards.get(expression) {
        return *value;
    }
    match expression.kind() {
        ExprKind::Constant(value) => *value,
        ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } => evaluate(
            if evaluate(condition, guards) == U256::ZERO {
                else_value
            } else {
                then_value
            },
            guards,
        ),
        _ => panic!("unassigned independent test selector"),
    }
}

// A Scope means its whole fact prefix AND every conditional branch. Compare
// complete selected values as well as this predicate; numeric-only equality
// would miss input, source, expression and nested-scope regressions.
fn holds(scope: &Scope, guards: &BTreeMap<ExprId, U256>, rejected: &TestValue) -> bool {
    scope.facts.values().all(|value| value != rejected)
        && scope.conditional.branches.iter().all(|branch| {
            holds(
                if evaluate(&branch.condition, guards) == U256::ZERO {
                    &branch.else_facts
                } else {
                    &branch.then_facts
                },
                guards,
                rejected,
            )
        })
}

fn selected(scope: &Scope, guards: &BTreeMap<ExprId, U256>) -> Vec<(ExprId, TestValue)> {
    let mut out = scope.facts.clone().into_iter().collect::<Vec<_>>();
    for branch in &scope.conditional.branches {
        out.extend(selected(
            if evaluate(&branch.condition, guards) == U256::ZERO {
                &branch.else_facts
            } else {
                &branch.then_facts
            },
            guards,
        ));
    }
    out
}

#[test]
fn all_five_rules_preserve_complete_scoped_predicates_for_independent_word_guards() {
    let [s, g, h, q] = std::array::from_fn::<_, 4, _>(|_| ExprId::fresh().unwrap());
    let a_value = observed(7, 701, Origin::Calldata);
    let b_value = observed(2, 702, Origin::Returndata);
    let c_value = observed(11, 703, Origin::Storage);
    let d_value = observed(13, 704, Origin::TransientStorage);
    let mut a = fact(&ExprId::fresh().unwrap(), a_value.clone());
    a.conditional = ite(
        &q,
        &fact(&ExprId::fresh().unwrap(), c_value.clone()),
        &fact(&ExprId::fresh().unwrap(), d_value.clone()),
    )
    .conditional;
    let b = fact(&ExprId::fresh().unwrap(), b_value.clone());
    let nested = ite(&g, &a, &b);
    let shapes = [
        (ite(&s, &nested, &a), g.clone(), constant(1)),
        (ite(&s, &nested, &b), g.clone(), constant(0)),
        (ite(&s, &a, &nested), constant(1), g.clone()),
        (ite(&s, &b, &nested), constant(0), g.clone()),
        (ite(&s, &nested, &ite(&h, &a, &b)), g.clone(), h.clone()),
    ];
    let alphabet = [U256::ZERO, U256::ONE, U256::from(2), U256::MAX];
    for (original, yes_guard, no_guard) in shapes {
        let snapshot = original.clone();
        let conditional = joined(&original, limits()).unwrap();
        let branch = &conditional.branches[0];
        assert_eq!(
            branch.condition,
            ExprId::ite(s.clone(), yes_guard, no_guard, limits()).unwrap()
        );
        assert_eq!(branch.then_facts, a);
        assert_eq!(branch.else_facts, b);
        let normalized = Scope {
            conditional,
            ..empty()
        };
        for sv in alphabet {
            for gv in alphabet {
                for hv in alphabet {
                    for qv in alphabet {
                        let guards = BTreeMap::from([
                            (s.clone(), sv),
                            (g.clone(), gv),
                            (h.clone(), hv),
                            (q.clone(), qv),
                        ]);
                        assert_eq!(selected(&original, &guards), selected(&normalized, &guards));
                        for rejected in [&a_value, &b_value, &c_value, &d_value] {
                            assert_eq!(
                                holds(&original, &guards, rejected),
                                holds(&normalized, &guards, rejected)
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(original, snapshot);
    }
}

#[test]
fn same_content_root_cannot_alias_different_numeric_input_provenance_or_nested_scopes() {
    let source = TestBytes::unknown();
    let read = |array: &TestBytes| {
        ExprId::word_read(
            ByteView::new(array.state_root().clone(), constant(0), constant(32)),
            constant(0),
            limits(),
        )
        .unwrap()
    };
    let expression = read(&source);
    let key = ExprId::fresh().unwrap();
    let value = observed(7, 801, Origin::Calldata).with_expression(expression.clone());
    let a = fact(&key, value.clone());
    let b = fact(&key, word(2));
    let mut provenance = value.clone();
    provenance.set_origin(Origin::Memory);
    let mut nested = a.clone();
    nested.conditional = ite(
        &ExprId::fresh().unwrap(),
        &fact(&ExprId::fresh().unwrap(), word(17)),
        &empty(),
    )
    .conditional;
    let another_source = TestBytes::unknown();
    assert_ne!(source.state_root(), another_source.state_root());
    let variants = [
        fact(&key, value.clone().with_numeric(word(9).numeric().clone())),
        fact(
            &key,
            observed(7, 802, Origin::Calldata).with_expression(expression.clone()),
        ),
        fact(&key, provenance),
        fact(&key, value.with_expression(read(&another_source))),
        nested,
    ];
    for different in variants {
        assert_ne!(a, different);
        let original = ite(
            &ExprId::fresh().unwrap(),
            &ite(&ExprId::fresh().unwrap(), &a, &b),
            &different,
        );
        assert_eq!(joined(&original, limits()).unwrap(), original.conditional);
    }
    assert_eq!(read(&source.clone()), expression);
}

#[test]
fn prefixes_conjunctions_and_reversed_pairs_stay_opaque() {
    let [s, g, h] = std::array::from_fn::<_, 3, _>(|_| ExprId::fresh().unwrap());
    let a = fact(&ExprId::fresh().unwrap(), word(7));
    let b = fact(&ExprId::fresh().unwrap(), word(2));
    let nested = ite(&g, &a, &b);
    let mut prefix = nested.clone();
    prefix.facts = fact(&ExprId::fresh().unwrap(), word(19)).facts;
    let mut conjunction = nested.clone();
    conjunction
        .conditional
        .branches
        .extend(ite(&h, &b, &a).conditional.branches);
    conjunction.conditional = ConditionalFacts::from_branches(conjunction.conditional.branches);
    for opaque in [prefix, conjunction] {
        let original = ite(&s, &opaque, &a);
        assert_eq!(joined(&original, limits()).unwrap(), original.conditional);
    }
    let original = ite(&s, &nested, &ite(&h, &b, &a));
    assert_eq!(joined(&original, limits()).unwrap(), original.conditional);
}

#[test]
fn unknown_selector_and_inactive_contradiction_never_promote_global_facts() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let [s, t] = std::array::from_fn::<_, 2, _>(|_| ExprId::fresh().unwrap());
    let key = ExprId::fresh().unwrap();
    let inactive = ExprId::fresh().unwrap();
    let a = fact(&key, word(7));
    let mut b = fact(&key, word(9));
    let mut contradiction = fact(&key, word(11));
    contradiction.facts.insert(inactive.clone(), word(13));
    b.conditional = ite(&constant(1), &contradiction, &empty()).conditional;
    let original = ite(&t, &ite(&s, &a, &b), &a);
    let out = joined(&original, limits()).unwrap();
    let guard = &out.branches[0].condition;
    let global = Facts::new();
    let context = Context::new(&global, Some(&out), domain.borrowed());
    assert_eq!(context.value(&key, domain.borrowed()), None);
    assert_eq!(
        context.branch(guard, true).value(&key, domain.borrowed()),
        Some(word(7))
    );
    assert!(global.is_empty());
    let mapped = out
        .map(domain.borrowed(), &mut |term, value| {
            assert_ne!(
                term, &inactive,
                "inactive scopes cannot invoke the callback"
            );
            Ok((
                if term == guard {
                    constant(1)
                } else {
                    term.clone()
                },
                value.cloned(),
            ))
        })
        .unwrap();
    // The selected fact prefix keeps its true stamp rather than becoming a
    // separate unconditional fact; the original contradictory scope is intact.
    assert_eq!(mapped.branches[0].condition, constant(1));
    assert_eq!(mapped.branches[0].then_facts, a);
    assert_eq!(out.branches[0].else_facts, b);
    assert!(global.is_empty());
}

#[test]
fn optional_guard_depth_and_node_growth_keep_the_original_valid_metadata() {
    let a = fact(&ExprId::fresh().unwrap(), word(7));
    let b = fact(&ExprId::fresh().unwrap(), word(2));
    let compound = || {
        (0..3).fold(ExprId::fresh().unwrap(), |term, _| {
            ExprId::operation(opcode::ADD, &[term, constant(1)], limits()).unwrap()
        })
    };
    let depth_case = ite(&compound(), &ite(&ExprId::fresh().unwrap(), &a, &b), &a);
    let depth_policy = ExprLimits {
        max_depth: depth_case.conditional.depth,
        ..limits()
    };
    let node_case = ite(&compound(), &ite(&compound(), &a, &b), &a);
    let node_policy = ExprLimits {
        max_nodes: node_case
            .conditional
            .entries
            .max(node_case.conditional.largest_expression),
        ..limits()
    };
    for (original, policy) in [(depth_case, depth_policy), (node_case, node_policy)] {
        let snapshot = original.clone();
        original.conditional.check(policy).unwrap();
        assert_eq!(joined(&original, policy).unwrap(), original.conditional);
        assert_eq!(original, snapshot);
    }
}

#[test]
fn independent_native_metadata_work_admission_retains_the_original_snapshot() {
    // Shared expression handles give a large expanded work bound without a
    // large allocation. The caller policy is deliberately distinct from the
    // unchanged export policy; this does not change any analysis budget.
    fn expanded(leaves: usize, policy: ExprLimits) -> ExprId {
        if leaves == 1 {
            return ExprId::fresh().unwrap();
        }
        let left = expanded(leaves / 2, policy);
        let right = if leaves.is_multiple_of(2) {
            left.clone()
        } else {
            expanded(leaves - leaves / 2, policy)
        };
        ExprId::operation(opcode::ADD, &[left, right], policy).unwrap()
    }
    let native = crate::domain::symbolic::GraphLimits::default();
    let policy = ExprLimits {
        max_nodes: native.max_nodes + 2,
        max_depth: native.max_depth,
    };
    let value = TestValue::top();
    let expression_nodes = native.max_nodes - value.work_size() - 3;
    assert_eq!(expression_nodes % 2, 1);
    let expression = expanded(expression_nodes.div_ceil(2), policy);
    assert_eq!(expression.nodes(), expression_nodes);
    let b = fact(&ExprId::fresh().unwrap(), value.with_expression(expression));
    let original = ite(
        &ExprId::fresh().unwrap(),
        &ite(&ExprId::fresh().unwrap(), &empty(), &b),
        &empty(),
    );
    assert_eq!(original.conditional.work, native.max_nodes);
    original
        .conditional
        .check(ExprLimits {
            max_nodes: native.max_nodes,
            max_depth: native.max_depth,
        })
        .unwrap();
    assert_eq!(joined(&original, policy).unwrap(), original.conditional);
}

#[test]
fn original_limits_real_cancellation_and_action_refusal_stay_atomic() {
    let a = fact(&ExprId::fresh().unwrap(), word(7));
    let b = fact(&ExprId::fresh().unwrap(), word(2));
    let original = ite(
        &ExprId::fresh().unwrap(),
        &ite(&ExprId::fresh().unwrap(), &a, &b),
        &a,
    );
    let snapshot = original.clone();
    let tiny = ExprLimits {
        max_nodes: original.conditional.entries - 1,
        ..limits()
    };
    let error = original.conditional.check(tiny).unwrap_err();
    assert_eq!(joined(&original, tiny), Err(error));
    let mut paid = Actions {
        remaining: 1000,
        declined: false,
    };
    assert!(paid.equal(&a, &a).unwrap());
    let spent = 1000 - paid.remaining;
    assert!(spent > a.facts.len());
    let mut refused = Actions {
        remaining: spent - 1,
        declined: false,
    };
    assert!(!refused.equal(&a, &a).unwrap());
    assert!(refused.cloned(&a).unwrap().is_none());
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(joined(&original, limits()), Err(ExprError::Cancelled));
        assert_eq!(
            shared_branches(original.conditional.clone(), limits()),
            Err(ExprError::Cancelled)
        );
        assert_eq!(refused.pay(0), Err(ExprError::Cancelled));
    });
    assert_eq!(original, snapshot);
    assert_eq!(a, snapshot.conditional.branches[0].else_facts);
}

#[test]
fn repeated_rules_borrow_a_large_tail_once_and_decline_overlong_optional_work() {
    let a = fact(&ExprId::fresh().unwrap(), word(7));
    let mut tail = empty();
    for _ in 0..20 {
        tail.facts.insert(ExprId::fresh().unwrap(), word(2));
    }
    let mut original = tail.clone();
    for _ in 0..13 {
        original = ite(&ExprId::fresh().unwrap(), &a, &original);
    }
    let out = joined(&original, limits()).unwrap();
    assert_eq!(out.branches[0].then_facts, a);
    assert_eq!(out.branches[0].else_facts, tail);
    assert!(out.entries < original.conditional.entries);
    let mut long = fact(&ExprId::fresh().unwrap(), word(2));
    for _ in 0..40 {
        long = ite(&ExprId::fresh().unwrap(), &a, &long);
    }
    long.conditional.check(limits()).unwrap();
    let snapshot = long.clone();
    assert_eq!(joined(&long, limits()).unwrap(), long.conditional);
    assert_eq!(long, snapshot);
}

fn candidates(array: &TestBytes, domain: TestAlgebra) -> Vec<U256> {
    array
        .read_word_checked(&word(0), domain.borrowed())
        .unwrap()
        .constants()
        .expect("retain the complete finite scoped word candidates")
        .iter()
        .copied()
        .collect()
}

#[test]
fn public_byte_array_repeated_root_factor_keeps_exact_scoped_precision() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let [q, s, t] = std::array::from_fn::<_, 3, _>(|_| ExprId::fresh().unwrap());
    let value = domain
        .join(&word(7), &word(9))
        .with_expression(ExprId::ite(q.clone(), constant(7), constant(9), limits()).unwrap());
    let mut source = TestBytes::memory();
    source
        .write_word(&word(0), &value, domain.borrowed())
        .unwrap();
    let original = source.clone();
    let refine = |truth| {
        let mut out = source.clone();
        assert!(
            out.refine_guards::<Infallible, _>(domain.borrowed(), limits(), &mut |condition| {
                Ok((condition == &q).then_some(truth))
            })
            .unwrap()
        );
        assert_eq!(out.state_root(), source.state_root());
        out
    };
    let yes = refine(true);
    let no = refine(false);
    let mut b = TestBytes::memory();
    b.write_word(&word(0), &word(2), domain.borrowed()).unwrap();
    let scoped_snapshots = (yes.clone(), no.clone(), b.clone());
    for (scoped, expected) in [(&yes, 7u64), (&no, 9u64)] {
        assert_eq!(candidates(scoped, domain), vec![U256::from(expected)]);
        let left = scoped
            .join_guarded(&b, domain.borrowed(), s.clone(), limits())
            .unwrap();
        assert_eq!(
            candidates(&left, domain),
            vec![U256::from(2), U256::from(expected)]
        );
        let joined = left
            .join_guarded(scoped, domain.borrowed(), t.clone(), limits())
            .unwrap();
        assert_eq!(
            candidates(&joined, domain),
            vec![U256::from(2), U256::from(expected)],
            "root factoring must retain the exact supported scoped finite set"
        );
        assert!(joined.symbolic_facts().all(|(term, _)| term != &q));
    }
    assert_eq!(yes.state_root(), no.state_root());
    assert_ne!(yes, no);
    let left = yes
        .join_guarded(&b, domain.borrowed(), s, limits())
        .unwrap();
    let changed_scope = left
        .join_guarded(&no, domain.borrowed(), t, limits())
        .unwrap();
    assert_eq!(
        candidates(&changed_scope, domain),
        vec![U256::from(2), U256::from(7), U256::from(9)],
        "different scopes of the same root must retain both q alternatives"
    );
    assert_eq!(source, original);
    assert_eq!(
        (&yes, &no, &b),
        (
            &scoped_snapshots.0,
            &scoped_snapshots.1,
            &scoped_snapshots.2
        )
    );
    assert_eq!(
        candidates(&source, domain),
        vec![U256::from(7), U256::from(9)]
    );
}
