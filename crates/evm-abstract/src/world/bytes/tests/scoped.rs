//! Only this snapshot's complete destination facts justify a demanded read.

use super::super::*;
use crate::domain::{
    identity::Symbol,
    provenance::Origin,
    relational::{CheckResult, RelationLimits, RelationState, ValueQuery},
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

fn domain(limits: RelationLimits) -> Domain {
    Domain::from_spec(Domain::default().spec().with_relations(limits))
}

fn observed(scope: u64, origin: Origin) -> AbstractValue {
    let mut value = AbstractValue::top();
    value.set_origin(origin);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_numeric(constant(7).numeric().clone())
}

fn destinations(domain: Domain, limits: RelationLimits) -> AbstractValue {
    constant(128)
        .join_guarded(
            &constant(192),
            domain,
            ExprId::fresh().unwrap(),
            limits.expressions(),
        )
        .unwrap()
}

fn relations(memory: &ByteArray, limits: RelationLimits) -> RelationState {
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
    match facts.local_value(read, &limits) {
        ValueQuery::Exact(actual) => assert_eq!(actual, U256::from(7)),
        ValueQuery::Bounds(bounds) => assert!(bounds.contains(U256::from(7))),
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
        facts.local_value(&equal, &limits),
        ValueQuery::Exact(U256::ONE)
    );
    let mut forwarded = facts.clone();
    assert_eq!(
        forwarded.assume(&equal, true, &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        forwarded.local_value(read, &limits),
        ValueQuery::Exact(U256::from(7))
    );
}

#[test]
fn fixed_reads_across_ite_copy_destinations_stay_within_original_policy() {
    let limits = policy();
    let domain = domain(limits);
    let source = ByteArray::exact(&[0x34, 0x08, 0xe4, 0x70]);
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(64), &constant(128), 4096, domain)
        .unwrap();
    // Immutable988 fails iteration9 with31717 nodes against this16384 budget.
    for iteration in 0..48 {
        let pointer = memory.read_word_checked(&constant(64), domain).unwrap();
        let expected = U256::from(128 + iteration * 64);
        assert_eq!(pointer.singleton(), Some(expected));
        let alternative = domain.apply(opcode::ADD, &[pointer.clone(), constant(32)]);
        let destination = pointer
            .join_guarded(
                &alternative,
                domain,
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap();
        memory
            .copy_from(&destination, &source, &zero(), &constant(4), 4096, domain)
            .unwrap();
        let snapshot = memory.clone();
        let read = memory.read_word_checked(&constant(64), domain).unwrap();
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
            .write_word(&constant(64), &next, 4096, domain)
            .unwrap();
        memory
            .write_word(&constant(128 + iteration * 32), &read, 4096, domain)
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
    let mut memory = ByteArray::memory();
    memory.write_word(&constant(64), &a, 4096, domain).unwrap();
    memory.write_word(&constant(96), &b, 4096, domain).unwrap();
    let destination = destinations(domain, limits);
    memory
        .copy_from(
            &destination,
            &ByteArray::exact(&[0xab; 32]),
            &zero(),
            &constant(32),
            4096,
            domain,
        )
        .unwrap();
    let original = memory.clone();
    // A previously captured raw read is never rewritten by the World proof.
    let old = ExprId::word_read(
        ByteView::new(
            memory.root.clone(),
            expr(&constant(64)).unwrap(),
            expr(&constant(32)).unwrap(),
        ),
        expr(&zero()).unwrap(),
        limits.expressions(),
    )
    .unwrap();
    assert!(matches!(old.kind(), ExprKind::WordRead { view, .. } if view.state == original.root));
    for current in [
        memory.clone(),
        memory.widening_form(domain, limits.expressions()).unwrap(),
    ] {
        let read_a = current.read_word_checked(&constant(64), domain).unwrap();
        let read_b = current.read_word_checked(&constant(96), domain).unwrap();
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
            facts.local_value(a.expression().unwrap(), &limits),
            ValueQuery::Exact(U256::from(7))
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
    replayed.rename_fresh(&map, limits.expressions()).unwrap();
    let renamed = a
        .expression()
        .unwrap()
        .rename_fresh(&map, limits.expressions())
        .unwrap();
    assert_eq!(
        replayed
            .read_word_checked(&constant(64), domain)
            .unwrap()
            .expression(),
        Some(&renamed)
    );
    let slice = replayed
        .slice(&constant(64), &constant(32), 4096, domain)
        .unwrap();
    assert_eq!(
        slice.read_word(&zero(), domain).singleton(),
        Some(U256::from(7))
    );
    let mut child = ByteArray::memory();
    child
        .copy_from(&zero(), &slice, &zero(), &constant(32), 4096, domain)
        .unwrap();
    assert_eq!(
        child.read_word(&zero(), domain).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        relations(&child, limits)
            .local_value(&expr(&child.read_word(&zero(), domain)).unwrap(), &limits),
        ValueQuery::Exact(U256::from(7))
    );
    memory
        .write_word(&constant(64), &constant(11), 4096, domain)
        .unwrap();
    prove_raw_snapshot_read(
        &relations(&original, limits),
        &old,
        a.expression().unwrap(),
        limits,
    );
    assert!(matches!(old.kind(), ExprKind::WordRead { view, .. } if view.state == original.root));
    assert_eq!(
        memory.read_word(&constant(64), domain).singleton(),
        Some(U256::from(11))
    );
}

#[test]
fn equal_roots_broadened_or_missing_facts_never_reuse_a_scoped_proof() {
    let limits = policy();
    let domain = domain(limits);
    let a = observed(1103, Origin::Calldata);
    let destination = ExprId::fresh().unwrap();
    let copy = |value: AbstractValue| {
        let mut memory = ByteArray::memory();
        memory.write_word(&constant(64), &a, 4096, domain).unwrap();
        memory
            .copy_from(
                &value.with_expression(destination.clone()),
                &ByteArray::exact(&[0xab; 32]),
                &zero(),
                &constant(32),
                4096,
                domain,
            )
            .unwrap();
        memory
    };
    let certain = copy(AbstractValue::unsigned_range(U256::from(128), U256::from(192)).unwrap());
    let broader = copy(AbstractValue::unsigned_range(U256::ZERO, U256::from(192)).unwrap());
    let unknown = copy(AbstractValue::top());
    assert_eq!(certain.root, broader.root);
    assert_eq!(certain.root, unknown.root);
    assert_eq!(
        certain.read_word(&constant(64), domain).expression(),
        a.expression()
    );
    let mut missing = certain.clone();
    missing.operands.remove(&destination);
    let joined = certain
        .join_guarded(
            &broader,
            domain,
            ExprId::fresh().unwrap(),
            limits.expressions(),
        )
        .unwrap();
    for current in [&broader, &unknown, &missing, &joined] {
        let read = current.read_word_checked(&constant(64), domain).unwrap();
        assert!(
            matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == current.root)
        );
        for (at, expected) in [(64, U256::from_be_bytes([0xab; 32])), (128, U256::from(7))] {
            let mut facts = relations(current, limits);
            facts
                .observe(&destination, constant(at).numeric(), &limits)
                .unwrap();
            assert_eq!(
                facts.local_value(read.expression().unwrap(), &limits),
                ValueQuery::Exact(expected)
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
                domain,
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap()
    };
    let pointer = make_pointer();
    let independent = make_pointer();
    assert_eq!(pointer.numeric(), independent.numeric());
    assert_ne!(pointer.expression(), independent.expression());
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(64), &pointer, 4096, domain)
        .unwrap();
    memory
        .copy_from(
            &destinations(domain, limits),
            &ByteArray::exact(&[0xab; 32]),
            &zero(),
            &constant(32),
            4096,
            domain,
        )
        .unwrap();
    // The actual CALL history has a padding word after the Copy, at ADD(4,P).
    let end = domain.apply(opcode::ADD, &[constant(4), pointer.clone()]);
    memory.write_word(&end, &zero(), 4096, domain).unwrap();
    let snapshot = memory.clone();
    let read = memory.read_word_checked(&constant(64), domain).unwrap();
    assert_eq!(read.expression(), pointer.expression());
    assert_eq!(read.numeric(), pointer.numeric());
    let difference = ExprId::operation(
        opcode::XOR,
        &[expr(&read).unwrap(), expr(&independent).unwrap()],
        limits.expressions(),
    )
    .unwrap();
    let ValueQuery::Bounds(bounds) = relations(&memory, limits).local_value(&difference, &limits)
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
        relations(&memory, limits).can_equal(&difference, U256::from(0x40), &limits),
        CheckResult::MayBeFeasible
    );
    assert_eq!(memory, snapshot);
}

#[test]
fn branch_local_copy_facts_preserve_the_whole_snapshot_selector() {
    let limits = policy();
    let domain = domain(limits);
    let destination = destinations(domain, limits);
    let mut yes = ByteArray::memory();
    yes.write_word(&constant(64), &constant(7), 4096, domain)
        .unwrap();
    yes.copy_from(
        &destination,
        &ByteArray::exact(&[0xab]),
        &zero(),
        &constant(1),
        4096,
        domain,
    )
    .unwrap();
    let mut no = ByteArray::memory();
    no.write_word(&constant(64), &constant(11), 4096, domain)
        .unwrap();
    // The destination fact exists only in the Copy alternative.
    assert!(!no.operands.contains_key(destination.expression().unwrap()));
    let guard = ExprId::fresh().unwrap();
    let joined = yes
        .join_guarded(&no, domain, guard.clone(), limits.expressions())
        .unwrap();
    let snapshot = joined.clone();
    let read = joined.read_word_checked(&constant(64), domain).unwrap();
    assert_eq!(
        read.constants(),
        Some(&BTreeSet::from([U256::from(7), U256::from(11)]))
    );
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::Ite { condition, .. } if condition == &guard)
    );
    for (branch, expected) in [(true, 7), (false, 11)] {
        let mut facts = relations(&joined, limits);
        facts.assume(&guard, branch, &limits);
        assert_eq!(
            facts.local_value(read.expression().unwrap(), &limits),
            ValueQuery::Exact(U256::from(expected))
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
        let mut base = ByteArray::memory();
        base.write_word(&constant(64), &a, 4096, domain).unwrap();
        let write = |value: AbstractValue| {
            let mut out = base.clone();
            let at = value.with_expression(destination.clone());
            if byte {
                out.write_byte(&at, &constant(11), 4096, domain).unwrap();
            } else {
                out.write_word(&at, &constant(11), 4096, domain).unwrap();
            }
            out
        };
        let certain =
            write(AbstractValue::unsigned_range(U256::from(128), U256::from(192)).unwrap());
        let broader = write(AbstractValue::unsigned_range(U256::ZERO, U256::from(192)).unwrap());
        let unknown = write(AbstractValue::top());
        let mut missing = certain.clone();
        missing.operands.remove(&destination);
        let joined = certain
            .join_guarded(
                &broader,
                domain,
                ExprId::fresh().unwrap(),
                limits.expressions(),
            )
            .unwrap();
        assert_eq!(certain.root, broader.root);
        assert_eq!(
            certain.read_word(&constant(64), domain).expression(),
            a.expression()
        );
        assert!(
            matches!(certain.read_byte(&constant(64), domain).expression().unwrap().kind(), ExprKind::ByteRead { state, .. } if state == &base.root)
        );
        for current in [&broader, &unknown, &missing, &joined] {
            let read = current.read_word_checked(&constant(64), domain).unwrap();
            assert!(
                matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == current.root)
            );
            assert!(
                matches!(current.read_byte(&constant(64), domain).expression().unwrap().kind(), ExprKind::ByteRead { state, .. } if state == &current.root)
            );
            let overwritten = if byte {
                (U256::from(11) << 248) | U256::from(7)
            } else {
                U256::from(11)
            };
            for (at, expected) in [(64, overwritten), (128, U256::from(7))] {
                let mut facts = relations(current, limits);
                facts
                    .observe(&destination, constant(at).numeric(), &limits)
                    .unwrap();
                assert_eq!(
                    facts.local_value(read.expression().unwrap(), &limits),
                    ValueQuery::Exact(expected)
                );
            }
        }
        // A fact supplied only by this branch never selects the other state.
        let mut no = ByteArray::memory();
        no.write_word(&constant(64), &constant(13), 4096, domain)
            .unwrap();
        let guard = ExprId::fresh().unwrap();
        let branched = certain
            .join_guarded(&no, domain, guard.clone(), limits.expressions())
            .unwrap();
        let read = branched.read_word_checked(&constant(64), domain).unwrap();
        assert_eq!(
            read.constants(),
            Some(&BTreeSet::from([U256::from(7), U256::from(13)]))
        );
        for (branch, expected) in [(true, 7), (false, 13)] {
            let mut facts = relations(&branched, limits);
            facts.assume(&guard, branch, &limits);
            assert_eq!(
                facts.local_value(read.expression().unwrap(), &limits),
                ValueQuery::Exact(U256::from(expected))
            );
        }
    }
}

#[test]
fn scoped_lower_bounds_preserve_max_padding_symbolic_length_and_atomic_errors() {
    let limits = policy();
    let domain = domain(limits);
    let destination = AbstractValue::unsigned_range(U256::from(128), U256::MAX)
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let length = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let mut memory = ByteArray::memory();
    let input = observed(1104, Origin::Calldata);
    memory
        .write_word(&constant(64), &input, 4096, domain)
        .unwrap();
    memory
        .copy_from(
            &destination,
            &ByteArray::exact(&[0xab; 32]),
            &zero(),
            &length,
            4096,
            domain,
        )
        .unwrap();
    let snapshot = memory.clone();
    assert_eq!(
        memory.read_word(&constant(64), domain).expression(),
        input.expression()
    );
    assert_eq!(
        memory.read_byte(&constant(64), domain).singleton(),
        Some(U256::ZERO)
    );
    let at_max = AbstractValue::constant(U256::MAX);
    let read = memory.read_word_checked(&at_max, domain).unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if view.state == memory.root)
    );
    // Existing full-root checks run before a small demanded read can bypass it.
    let tight = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 8,
        ..limits
    }));
    assert!(memory.read_word_checked(&constant(64), tight).is_err());
    assert!(
        memory
            .write_word(&constant(64), &constant(11), 4096, tight)
            .is_err()
    );
    assert_eq!(memory, snapshot);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            memory.read_word_checked(&constant(64), domain),
            Err(RangeError::Expression(ExprError::Cancelled)
                | RangeError::State(StateError::Cancelled))
        ));
        assert!(
            memory
                .copy_from(&destination, &snapshot, &zero(), &constant(1), 4096, domain)
                .is_err()
        );
    });
    assert_eq!(memory, snapshot);
    memory
        .copy_from(&at_max, &snapshot, &at_max, &zero(), 0, domain)
        .unwrap();
    assert_eq!(memory, snapshot);
}
