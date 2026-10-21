//! Only this snapshot's complete destination facts justify a demanded read.

use super::super::*;
use crate::domain::{
    identity::Symbol,
    provenance::Origin,
    relational::{CheckResult, EvmValueQuery, RelationLimits, RelationState},
    symbolic::ExprKind,
};

fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 16_384,
        max_depth: 256,
        max_constraints: 2048,
        ..RelationLimits::default()
    }
}

fn domain(limits: RelationLimits) -> TestAlgebra {
    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
            .spec()
            .with_relations(limits),
    ))
}

fn observed(scope: u64, origin: Origin) -> TestValue {
    let mut value = TestValue::top();
    value.set_origin(origin);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant::<crate::domain::NumericValue>(7).numeric().clone())
}

fn destinations(domain: TestAlgebra, limits: RelationLimits) -> TestValue {
    constant(128)
        .join_guarded(
            &constant(192),
            domain.borrowed(),
            ExprId::fresh().unwrap(),
            limits.expressions(),
        )
        .unwrap()
}

fn relations(memory: &TestBytes, limits: RelationLimits) -> RelationState {
    memory.symbolic_relations(&limits).unwrap()
}

fn prove_raw_snapshot_read(
    facts: &RelationState,
    read: &ExprId,
    source: &ExprId,
    limits: RelationLimits,
) {
    // A local forwarding improvement may already prove the raw read. Retain
    // concrete inclusion if it only derives bounds; do not require imprecision.
    match facts.local_value(
        read,
        &limits,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    ) {
        EvmValueQuery::Exact(actual) => assert_eq!(actual, U256::from(7)),
        EvmValueQuery::Bounds(bounds) => assert!(bounds.contains(U256::from(7))),
        other => panic!("raw historical read lost its concrete seven: {other:?}"),
    }

    // Compose two local queries without importing any new input premise: first
    // prove forwarding to the original source, then retain that proved equality.
    let equal = ExprId::operation(
        opcode::EQ,
        &[read.clone(), source.clone()],
        limits.expressions(),
    )
    .unwrap();
    assert_eq!(
        facts.local_value(
            &equal,
            &limits,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        EvmValueQuery::Exact(U256::ONE)
    );
    let mut forwarded = facts.clone();
    assert_eq!(
        forwarded.assume(
            &equal,
            true,
            &limits,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        forwarded.local_value(
            read,
            &limits,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        EvmValueQuery::Exact(U256::from(7))
    );
}

#[test]
fn fixed_reads_across_ite_copy_destinations_stay_within_original_policy() {
    let limits = policy();
    let domain = domain(limits);
    let source = TestBytes::exact(&[0x34, 0x08, 0xe4, 0x70]);
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(64), &constant(128), domain.borrowed())
        .unwrap();
    // Immutable988 fails iteration9 with31717 nodes against this16384 budget.
    for iteration in 0..48 {
        let pointer = memory
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        let expected = U256::from(128 + iteration * 64);
        assert_eq!(pointer.singleton(), Some(expected));
        let alternative = domain.apply(opcode::ADD, &[pointer.clone(), constant(32)]);
        let destination = pointer
            .join_guarded(
                &alternative,
                domain.borrowed(),
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap();
        memory
            .copy_from(
                &destination,
                &source,
                &zero(),
                &constant(4),
                domain.borrowed(),
            )
            .unwrap();
        let snapshot = memory.clone();
        let read = memory
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        assert_eq!(read.singleton(), Some(expected));
        assert!(read.expression().is_none_or(|e| e.nodes() <= 4));
        assert_eq!(memory, snapshot);
        assert!(
            matches!(memory.root.kind(), ByteStateKind::Copy { destination: at, .. }
            if Some(at) == destination.expression())
        );
        assert_eq!(
            memory
                .operands
                .get(destination.expression().unwrap())
                .unwrap()
                .numeric(),
            destination.numeric()
        );
        let next = domain.apply(opcode::ADD, &[read.clone(), constant(64)]);
        memory
            .write_word(&constant(64), &next, domain.borrowed())
            .unwrap();
        memory
            .write_word(&constant(128 + iteration * 32), &read, domain.borrowed())
            .unwrap();
        assert!(memory.root.nodes() <= limits.max_nodes);
        assert!(memory.root.depth() <= limits.max_depth);
    }
}

#[test]
fn scoped_reads_keep_original_sources_old_terms_and_renamed_facts() {
    let limits = policy();
    let domain = domain(limits);
    let a = observed(1101, Origin::Calldata);
    let b = observed(1102, Origin::Returndata);
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(64), &a, domain.borrowed())
        .unwrap();
    memory
        .write_word(&constant(96), &b, domain.borrowed())
        .unwrap();
    let destination = destinations(domain, limits);
    memory
        .copy_from(
            &destination,
            &TestBytes::exact(&[0xab; 32]),
            &zero(),
            &constant(32),
            domain.borrowed(),
        )
        .unwrap();
    let original = memory.clone();
    // A previously captured raw read is never rewritten by the World proof.
    let old = ExprId::word_read(
        ByteView::new(
            memory.root.clone(),
            expr(&constant::<crate::domain::NumericValue>(64)).unwrap(),
            expr(&constant::<crate::domain::NumericValue>(32)).unwrap(),
        ),
        expr(&zero::<crate::domain::NumericValue>()).unwrap(),
        limits.expressions(),
    )
    .unwrap();
    assert!(matches!(old.kind(), ExprKind::WordRead { view, .. } if view.state == original.root));
    for current in [memory.clone(), memory.project(domain.borrowed())] {
        let read_a = current
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        let read_b = current
            .read_word_checked(&constant(96), domain.borrowed())
            .unwrap();
        assert_eq!(read_a.expression(), a.expression());
        assert_eq!(read_b.expression(), b.expression());
        assert!(!read_a.same_identity(&read_b));
        for (read, origin) in [(&read_a, Origin::Calldata), (&read_b, Origin::Returndata)] {
            assert!(
                read.provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
        }
        let facts = relations(&current, limits);
        prove_raw_snapshot_read(&facts, &old, a.expression().unwrap(), limits);
        assert_eq!(
            facts.local_value(a.expression().unwrap(), &limits, domain.borrowed()),
            EvmValueQuery::Exact(U256::from(7))
        );
        assert_eq!(current.root, original.root);
        assert_eq!(current.operands.get(a.expression().unwrap()), Some(&a));
    }
    let map = memory
        .fresh_leaves()
        .into_iter()
        .map(|e| (e, ExprId::fresh().unwrap()))
        .collect();
    let mut replayed = memory.clone();
    replayed
        .rename_fresh(&map, limits.expressions(), domain.borrowed())
        .unwrap();
    let renamed = a
        .expression()
        .unwrap()
        .rename_fresh(&map, limits.expressions())
        .unwrap();
    assert_eq!(
        replayed
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap()
            .expression(),
        Some(&renamed)
    );
    let slice = replayed
        .slice(&constant(64), &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(
        slice.read_word(&zero(), domain.borrowed()).singleton(),
        Some(U256::from(7))
    );
    let mut child = TestBytes::memory();
    child
        .copy_from(&zero(), &slice, &zero(), &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(
        child.read_word(&zero(), domain.borrowed()).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        relations(&child, limits).local_value(
            &expr(&child.read_word(&zero(), domain.borrowed())).unwrap(),
            &limits,
            domain.borrowed()
        ),
        EvmValueQuery::Exact(U256::from(7))
    );
    memory
        .write_word(&constant(64), &constant(11), domain.borrowed())
        .unwrap();
    prove_raw_snapshot_read(
        &relations(&original, limits),
        &old,
        a.expression().unwrap(),
        limits,
    );
    assert!(matches!(old.kind(), ExprKind::WordRead { view, .. } if view.state == original.root));
    assert_eq!(
        memory
            .read_word(&constant(64), domain.borrowed())
            .singleton(),
        Some(U256::from(11))
    );
}

#[test]
fn equal_roots_broadened_or_missing_facts_never_reuse_a_scoped_proof() {
    let limits = policy();
    let domain = domain(limits);
    let a = observed(1103, Origin::Calldata);
    let destination = ExprId::fresh().unwrap();
    let copy = |value: TestValue| {
        let mut memory = TestBytes::memory();
        memory
            .write_word(&constant(64), &a, domain.borrowed())
            .unwrap();
        memory
            .copy_from(
                &value.with_expression(destination.clone()),
                &TestBytes::exact(&[0xab; 32]),
                &zero(),
                &constant(32),
                domain.borrowed(),
            )
            .unwrap();
        memory
    };
    let certain = copy(TestValue::unsigned_range(U256::from(128), U256::from(192)).unwrap());
    let broader = copy(TestValue::unsigned_range(U256::ZERO, U256::from(192)).unwrap());
    let unknown = copy(TestValue::top());
    assert_eq!(certain.root, broader.root);
    assert_eq!(certain.root, unknown.root);
    assert_eq!(
        certain
            .read_word(&constant(64), domain.borrowed())
            .expression(),
        a.expression()
    );
    let mut missing = certain.clone();
    missing.operands.remove(&destination);
    let joined = certain
        .join_guarded(
            &broader,
            domain.borrowed(),
            ExprId::fresh().unwrap(),
            limits.expressions(),
        )
        .unwrap();
    for current in [&broader, &unknown, &missing, &joined] {
        let read = current
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        assert!(
            matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == current.root)
        );
        for (at, expected) in [(64, U256::from_be_bytes([0xab; 32])), (128, U256::from(7))] {
            let mut facts = relations(current, limits);
            facts
                .observe(
                    &destination,
                    constant::<crate::domain::NumericValue>(at).numeric(),
                    &limits,
                )
                .unwrap();
            assert_eq!(
                facts.local_value(read.expression().unwrap(), &limits, domain.borrowed()),
                EvmValueQuery::Exact(expected)
            );
        }
    }
}

#[test]
fn fixed_read_forwards_the_stored_guarded_term_without_equating_other_pointers() {
    let limits = policy();
    let domain = domain(limits);
    let make_pointer = || {
        constant(0x520)
            .join_guarded(
                &constant(0x560),
                domain.borrowed(),
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap()
    };
    let pointer = make_pointer();
    let independent = make_pointer();
    assert_eq!(pointer.numeric(), independent.numeric());
    assert_ne!(pointer.expression(), independent.expression());
    let mut memory = TestBytes::memory();
    memory
        .write_word(&constant(64), &pointer, domain.borrowed())
        .unwrap();
    memory
        .copy_from(
            &destinations(domain, limits),
            &TestBytes::exact(&[0xab; 32]),
            &zero(),
            &constant(32),
            domain.borrowed(),
        )
        .unwrap();
    // The actual CALL history has a padding word after the Copy, at ADD(4,P).
    let end = domain.apply(opcode::ADD, &[constant(4), pointer.clone()]);
    memory.write_word(&end, &zero(), domain.borrowed()).unwrap();
    let snapshot = memory.clone();
    let read = memory
        .read_word_checked(&constant(64), domain.borrowed())
        .unwrap();
    assert_eq!(read.expression(), pointer.expression());
    assert_eq!(read.numeric(), pointer.numeric());
    let difference = ExprId::operation(
        opcode::XOR,
        &[expr(&read).unwrap(), expr(&independent).unwrap()],
        limits.expressions(),
    )
    .unwrap();
    let EvmValueQuery::Bounds(bounds) =
        relations(&memory, limits).local_value(&difference, &limits, domain.borrowed())
    else {
        panic!("independent pointer choices retain both XOR candidates");
    };
    for candidate in [U256::ZERO, U256::from(0x40)] {
        assert!(
            bounds.contains(candidate),
            "lost independent XOR candidate {candidate}"
        );
    }
    assert_eq!(
        relations(&memory, limits).can_equal(
            &difference,
            U256::from(0x40),
            &limits,
            domain.borrowed()
        ),
        CheckResult::MayBeFeasible
    );
    assert_eq!(memory, snapshot);
}

#[test]
fn branch_local_copy_facts_preserve_the_whole_snapshot_selector() {
    let limits = policy();
    let domain = domain(limits);
    let destination = destinations(domain, limits);
    let mut yes = TestBytes::memory();
    yes.write_word(&constant(64), &constant(7), domain.borrowed())
        .unwrap();
    yes.copy_from(
        &destination,
        &TestBytes::exact(&[0xab]),
        &zero(),
        &constant(1),
        domain.borrowed(),
    )
    .unwrap();
    let mut no = TestBytes::memory();
    no.write_word(&constant(64), &constant(11), domain.borrowed())
        .unwrap();
    // The destination fact exists only in the Copy alternative.
    assert!(!no.operands.contains_key(destination.expression().unwrap()));
    let guard = ExprId::fresh().unwrap();
    let joined = yes
        .join_guarded(&no, domain.borrowed(), guard.clone(), limits.expressions())
        .unwrap();
    let snapshot = joined.clone();
    let read = joined
        .read_word_checked(&constant(64), domain.borrowed())
        .unwrap();
    assert_eq!(
        read.constants(),
        Some(&BTreeSet::from([U256::from(7), U256::from(11)]))
    );
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::Ite { condition, .. } if condition == &guard)
    );
    for (branch, expected) in [(true, 7), (false, 11)] {
        let mut facts = relations(&joined, limits);
        facts.assume(&guard, branch, &limits, domain.borrowed());
        assert_eq!(
            facts.local_value(read.expression().unwrap(), &limits, domain.borrowed()),
            EvmValueQuery::Exact(U256::from(expected))
        );
    }
    assert_eq!(joined, snapshot);
}

#[test]
fn symbolic_word_and_byte_write_bounds_are_snapshot_local() {
    let limits = policy();
    let domain = domain(limits);
    let a = observed(1105, Origin::Calldata);
    for byte in [false, true] {
        let destination = ExprId::fresh().unwrap();
        let mut base = TestBytes::memory();
        base.write_word(&constant(64), &a, domain.borrowed())
            .unwrap();
        let write = |value: TestValue| {
            let mut out = base.clone();
            let at = value.with_expression(destination.clone());
            if byte {
                out.write_byte(&at, &constant(11), domain.borrowed())
                    .unwrap();
            } else {
                out.write_word(&at, &constant(11), domain.borrowed())
                    .unwrap();
            }
            out
        };
        let certain = write(TestValue::unsigned_range(U256::from(128), U256::from(192)).unwrap());
        let broader = write(TestValue::unsigned_range(U256::ZERO, U256::from(192)).unwrap());
        let unknown = write(TestValue::top());
        let mut missing = certain.clone();
        missing.operands.remove(&destination);
        let joined = certain
            .join_guarded(
                &broader,
                domain.borrowed(),
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap();
        assert_eq!(certain.root, broader.root);
        assert_eq!(
            certain
                .read_word(&constant(64), domain.borrowed())
                .expression(),
            a.expression()
        );
        assert!(
            matches!(certain.read_byte(&constant(64), domain.borrowed()).expression().unwrap().kind(), ExprKind::ByteRead { state, .. } if state == &base.root)
        );
        for current in [&broader, &unknown, &missing, &joined] {
            let read = current
                .read_word_checked(&constant(64), domain.borrowed())
                .unwrap();
            assert!(
                matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == current.root)
            );
            assert!(
                matches!(current.read_byte(&constant(64), domain.borrowed()).expression().unwrap().kind(), ExprKind::ByteRead { state, .. } if state == &current.root)
            );
            let overwritten = if byte {
                (U256::from(11) << 248) | U256::from(7)
            } else {
                U256::from(11)
            };
            for (at, expected) in [(64, overwritten), (128, U256::from(7))] {
                let mut facts = relations(current, limits);
                facts
                    .observe(
                        &destination,
                        constant::<crate::domain::NumericValue>(at).numeric(),
                        &limits,
                    )
                    .unwrap();
                assert_eq!(
                    facts.local_value(read.expression().unwrap(), &limits, domain.borrowed()),
                    EvmValueQuery::Exact(expected)
                );
            }
        }
        // A fact supplied only by this branch never selects the other state.
        let mut no = TestBytes::memory();
        no.write_word(&constant(64), &constant(13), domain.borrowed())
            .unwrap();
        let guard = ExprId::fresh().unwrap();
        let branched = certain
            .join_guarded(&no, domain.borrowed(), guard.clone(), limits.expressions())
            .unwrap();
        let read = branched
            .read_word_checked(&constant(64), domain.borrowed())
            .unwrap();
        assert_eq!(
            read.constants(),
            Some(&BTreeSet::from([U256::from(7), U256::from(13)]))
        );
        for (branch, expected) in [(true, 7), (false, 13)] {
            let mut facts = relations(&branched, limits);
            facts.assume(&guard, branch, &limits, domain.borrowed());
            assert_eq!(
                facts.local_value(read.expression().unwrap(), &limits, domain.borrowed()),
                EvmValueQuery::Exact(U256::from(expected))
            );
        }
    }
}

#[test]
fn scoped_lower_bounds_preserve_max_padding_symbolic_length_and_atomic_errors() {
    let limits = policy();
    let domain = domain(limits);
    let destination = TestValue::unsigned_range(U256::from(128), U256::MAX)
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let length = TestValue::top().with_expression(ExprId::fresh().unwrap());
    let mut memory = TestBytes::memory();
    let input = observed(1104, Origin::Calldata);
    memory
        .write_word(&constant(64), &input, domain.borrowed())
        .unwrap();
    memory
        .copy_from(
            &destination,
            &TestBytes::exact(&[0xab; 32]),
            &zero(),
            &length,
            domain.borrowed(),
        )
        .unwrap();
    let snapshot = memory.clone();
    assert_eq!(
        memory
            .read_word(&constant(64), domain.borrowed())
            .expression(),
        input.expression()
    );
    assert_eq!(
        memory
            .read_byte(&constant(64), domain.borrowed())
            .singleton(),
        Some(U256::ZERO)
    );
    let at_max = TestValue::constant(U256::MAX);
    let read = memory
        .read_word_checked(&at_max, domain.borrowed())
        .unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == memory.root)
    );
    // Existing full-root checks run before a small demanded read can bypass it.
    let tight = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        domain.spec().with_relations(RelationLimits {
            max_nodes: 8,
            ..limits
        }),
    ));
    assert!(
        memory
            .read_word_checked(&constant(64), tight.borrowed())
            .is_err()
    );
    assert!(
        memory
            .write_word(&constant(64), &constant(11), tight.borrowed())
            .is_err()
    );
    assert_eq!(memory, snapshot);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            memory.read_word_checked(&constant(64), domain.borrowed()),
            Err(RangeError::Expression(ExprError::Cancelled)
                | RangeError::State(StateError::Cancelled))
        ));
        assert!(
            memory
                .copy_from(
                    &destination,
                    &snapshot,
                    &zero(),
                    &constant(1),
                    domain.borrowed()
                )
                .is_err()
        );
    });
    assert_eq!(memory, snapshot);
    memory
        .copy_from(&at_max, &snapshot, &at_max, &zero(), domain.borrowed())
        .unwrap();
    assert_eq!(memory, snapshot);
}
