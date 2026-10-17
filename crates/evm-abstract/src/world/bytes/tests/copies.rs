//! Demanded reads ignore copies only when their destination cannot overlap.

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

fn domain(policy: RelationLimits) -> Domain {
    Domain::from_spec(Domain::default().spec().with_relations(policy))
}

fn prove(relations: &RelationState, value: &AbstractValue, expected: U256, limits: RelationLimits) {
    assert_eq!(
        relations.local_value(&expr(value).unwrap(), &limits),
        ValueQuery::Exact(expected)
    );
    assert_eq!(
        relations.can_equal(&expr(value).unwrap(), expected ^ U256::ONE, &limits),
        CheckResult::ProvenImpossible
    );
}

fn return_length(limits: RelationLimits) -> (ExprId, ExprId) {
    let guard = ExprId::fresh().unwrap();
    let length = ExprId::ite(
        guard.clone(),
        ExprId::constant(U256::from(32)),
        ExprId::constant(U256::ZERO),
        limits.expressions(),
    )
    .unwrap();
    (guard, length)
}

#[test]
fn guarded_copy_and_overlapping_mcopy_keep_full_values_history_and_atomic_controls() {
    let limits = policy();
    let domain = domain(limits);
    let scoped = |scope, origin, number| {
        let mut value = AbstractValue::top();
        value.set_origin(origin);
        value
            .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
            .with_numeric(constant(number).numeric().clone())
    };
    let array = |value: &AbstractValue| {
        let mut array = ByteArray::memory();
        array.write_word(&zero(), value, 256, domain).unwrap();
        array
    };
    let left = array(&scoped(911, Origin::Calldata, 7));
    let right = array(&scoped(912, Origin::Returndata, 9));
    let guard = ExprId::fresh().unwrap();
    let mut memory = left
        .join_guarded(&right, domain, guard.clone(), limits.expressions())
        .unwrap();
    assert!(matches!(memory.root.kind(), ByteStateKind::Ite { .. }));
    assert_eq!(memory.root.copy_leaves(), 2);
    let saved_target = memory.clone();
    let historical = memory.read_word_checked(&zero(), domain).unwrap();
    // The 31-byte MCOPY excludes the last byte; keep copied bytes 28..=30
    // nonzero so the overlap checks exercise data as well as leading zeros.
    let source_word = 0x1122_3344;
    let mut source = array(&scoped(913, Origin::Calldata, source_word));
    let saved_source = source.clone();
    let copied = source.read_word_checked(&zero(), domain).unwrap();
    assert_eq!(copied.singleton(), Some(U256::from(source_word)));
    let copied_bytes = copied.singleton().unwrap().to_be_bytes::<32>();
    let dst = ExprId::constant(U256::from(64));
    let len = ExprId::constant(U256::from(32));
    let view = ByteView::new(
        source.root.clone(),
        ExprId::constant(U256::ZERO),
        len.clone(),
    );
    let yes = left
        .root
        .copy(
            dst.clone(),
            view.clone(),
            len.clone(),
            limits.expressions().into(),
        )
        .unwrap();
    let no = right
        .root
        .copy(dst, view, len, limits.expressions().into())
        .unwrap();
    let expected = yes
        .join_guarded(&no, guard, limits.expressions().into())
        .unwrap();
    memory
        .copy_from(&constant(64), &source, &zero(), &constant(32), 256, domain)
        .unwrap();
    assert_eq!(memory.root, expected);
    assert!(
        matches!(memory.root.kind(), ByteStateKind::Copy { previous, source, .. }
        if previous == &saved_target.root && source.state == saved_source.root)
    );
    assert_eq!(memory.root.copy_leaves(), 1);
    assert_eq!(
        memory.read_word_checked(&zero(), domain).unwrap(),
        historical
    );
    assert_eq!(
        memory.read_word_checked(&constant(64), domain).unwrap(),
        copied
    );
    assert_eq!(source, saved_source);
    let captured = memory.clone();
    let saved_captured = captured.clone();
    let mcopy_dst = ExprId::constant(U256::from(65));
    let mcopy_src = ExprId::constant(U256::from(64));
    let mcopy_len = ExprId::constant(U256::from(31));
    let expected_mcopy = captured
        .root
        .copy(
            mcopy_dst,
            ByteView::new(captured.root.clone(), mcopy_src, mcopy_len.clone()),
            mcopy_len,
            limits.expressions().into(),
        )
        .unwrap();
    memory
        .copy_from(
            &constant(65),
            &captured,
            &constant(64),
            &constant(31),
            256,
            domain,
        )
        .unwrap();
    assert_eq!(memory.root, expected_mcopy);
    assert_eq!(captured, saved_captured);
    assert!(
        matches!(memory.root.kind(), ByteStateKind::Copy { previous, source, .. }
        if previous == &captured.root && source.state == captured.root)
    );
    for at in [0, 1, 28, 29, 30] {
        let source_index = ExprId::constant(U256::from(64 + at));
        let target_index = ExprId::constant(U256::from(65 + at));
        let source_value = captured
            .read_byte_checked(&constant(64 + at), domain)
            .unwrap();
        let actual = memory
            .read_byte_checked(&constant(65 + at), domain)
            .unwrap();
        // In-range byte observers retain their snapshot and address, unlike
        // whole-word windows that can forward the captured source expression.
        assert!(matches!(
            source_value.expression().map(ExprId::kind),
            Some(ExprKind::ByteRead { state, index })
            if state == &captured.root && index == &source_index
        ));
        assert!(matches!(
            actual.expression().map(ExprId::kind),
            Some(ExprKind::ByteRead { state, index })
            if state == &expected_mcopy && index == &target_index
        ));
        let expected_target_expr =
            ExprId::byte_read(expected_mcopy.clone(), target_index, limits.expressions()).unwrap();
        assert_eq!(source_value.singleton(), Some(U256::from(copied_bytes[at])));
        // Rebase only the observation expression; numeric facts, provenance,
        // identity and symbolic-limit markers must all remain exactly equal.
        assert_eq!(
            actual,
            source_value.clone().with_expression(expected_target_expr)
        );
    }
    let overlapped = memory.read_word_checked(&constant(64), domain).unwrap();
    source
        .write_word(&zero(), &constant(99), 256, domain)
        .unwrap();
    memory
        .write_word(&zero(), &constant(77), 256, domain)
        .unwrap();
    assert_eq!(
        saved_target.read_word_checked(&zero(), domain).unwrap(),
        historical
    );
    assert_eq!(
        saved_source.read_word_checked(&zero(), domain).unwrap(),
        copied
    );
    assert_eq!(
        captured.read_word_checked(&constant(64), domain).unwrap(),
        copied
    );
    assert_eq!(
        memory.read_word_checked(&constant(64), domain).unwrap(),
        overlapped
    );
    // Wrapper metadata can reject before COPY construction. Check both
    // contracts separately under the same node and depth policies.
    for (denied, state_limit, wrapper_limit) in [
        (
            RelationLimits {
                max_nodes: saved_target.root.nodes(),
                ..limits
            },
            StateError::Limit {
                nodes: yes.nodes(),
                depth: yes.depth(),
            },
            RangeError::Expression(ExprError::Limit {
                // Three facts per branch, two entries per fact, plus a guard.
                nodes: 13,
                depth: 2,
            }),
        ),
        (
            RelationLimits {
                max_depth: saved_target.root.depth(),
                ..limits
            },
            StateError::Limit {
                nodes: expected.nodes(),
                depth: expected.depth(),
            },
            RangeError::State(StateError::Limit {
                nodes: expected.nodes(),
                depth: expected.depth(),
            }),
        ),
    ] {
        let len = ExprId::constant(U256::from(32));
        assert_eq!(
            saved_target.root.copy(
                ExprId::constant(U256::from(64)),
                ByteView::new(
                    saved_source.root.clone(),
                    ExprId::constant(U256::ZERO),
                    len.clone(),
                ),
                len,
                denied.expressions().into(),
            ),
            Err(state_limit)
        );
        let mut refused = saved_target.clone();
        assert_eq!(
            refused.copy_from(
                &constant(64),
                &saved_source,
                &zero(),
                &constant(32),
                256,
                self::domain(denied)
            ),
            Err(wrapper_limit)
        );
        assert_eq!(refused, saved_target);
    }
    let mut refused = saved_target.clone();
    let fee = refused.copy_work(&saved_source, &constant(64), &zero(), &constant(32), domain);
    let mut budget = crate::resource::WorkBudget::new(fee - 1);
    let admitted = budget.charge(fee).then(|| {
        refused.copy_from(
            &constant(64),
            &saved_source,
            &zero(),
            &constant(32),
            256,
            domain,
        )
    });
    assert!(admitted.is_none());
    assert_eq!(budget.used(), 0);
    assert!(budget.exhausted());
    assert_eq!(refused, saved_target);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            refused.copy_from(
                &constant(64),
                &saved_source,
                &zero(),
                &constant(32),
                256,
                domain
            ),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert_eq!(refused, saved_target);
    });
}

#[test]
fn free_pointer_reads_across_calldata_and_return_copies_stay_bounded() {
    let limits = policy();
    let domain = domain(limits);
    let (_, length) = return_length(limits);
    let returned = constant(32).with_expression(length.clone());
    let calldata = ByteArray::exact(&[0x34, 0x08, 0xe4, 0x70]);
    let returndata = ByteArray::exact(&[1; 32]);
    let mut memory = ByteArray::memory();
    memory
        .write_word(&constant(64), &constant(128), 4096, domain)
        .unwrap();
    let mut old_reads = Vec::new();
    // The unchanged combined baseline exhausts16384 nodes on this loop.
    for iteration in 0..48 {
        let pointer = memory.read_word_checked(&constant(64), domain).unwrap();
        let expected = U256::from(128 + iteration * 64);
        assert_eq!(pointer.singleton(), Some(expected));
        memory
            .copy_from(&pointer, &calldata, &zero(), &constant(4), 4096, domain)
            .unwrap();
        let pointer = memory.read_word_checked(&constant(64), domain).unwrap();
        assert_eq!(pointer.singleton(), Some(expected));
        old_reads.push((pointer.clone(), expected));
        let next = domain.apply(opcode::ADD, &[pointer.clone(), constant(64)]);
        memory
            .write_word(&constant(64), &next, 4096, domain)
            .unwrap();
        let target = domain.apply(opcode::ADD, &[pointer.clone(), constant(32)]);
        memory
            .copy_from(&target, &returndata, &zero(), &returned, 4096, domain)
            .unwrap();
        assert!(
            matches!(memory.state_root().kind(), ByteStateKind::Copy { length: retained, .. } if retained == &length)
        );
        assert!(memory.operands.contains_key(&length));
        memory
            .write_word(&constant(128 + iteration * 32), &pointer, 4096, domain)
            .unwrap();
        assert!(memory.state_root().nodes() <= 4 + (iteration + 1) * 24);
        assert!(memory.state_root().depth() <= limits.max_depth);
    }
    for (old, expected) in old_reads {
        prove(&RelationState::default(), &old, expected, limits);
    }
}

#[test]
fn disjoint_copy_reads_preserve_sources_facts_snapshots_and_hashes() {
    let limits = policy();
    let domain = domain(limits);
    let input = |scope, origin| {
        let mut value = AbstractValue::top();
        value.set_origin(origin);
        value
            .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
            .with_numeric(constant(7).numeric().clone())
    };
    let a = input(901, Origin::Calldata);
    let b = input(902, Origin::Returndata);
    let mut memory = ByteArray::memory();
    memory.write_word(&zero(), &a, 256, domain).unwrap();
    memory.write_word(&constant(32), &b, 256, domain).unwrap();
    let snapshot = memory.clone();
    let old = memory.read_word_checked(&zero(), domain).unwrap();
    let hash = memory
        .slice(&zero(), &constant(64), 256, domain)
        .unwrap()
        .keccak(domain, 64)
        .unwrap();
    let (_, length) = return_length(limits);
    let returned = constant(32).with_expression(length.clone());
    memory
        .copy_from(&constant(128), &snapshot, &zero(), &returned, 256, domain)
        .unwrap();
    let root = memory.state_root().clone();
    assert!(
        matches!(root.kind(), ByteStateKind::Copy { source, length: retained, .. } if source.state == *snapshot.state_root() && retained == &length)
    );
    for current in [
        &memory,
        &memory.widening_form(domain, limits.expressions()).unwrap(),
    ] {
        let mut relations = RelationState::default();
        for (expression, value) in current.symbolic_facts() {
            relations
                .observe(expression, value.numeric(), &limits)
                .unwrap();
        }
        for (offset, original, origin) in [(0, &a, Origin::Calldata), (32, &b, Origin::Returndata)]
        {
            assert_eq!(
                current.operands.get(original.expression().unwrap()),
                Some(original)
            );
            let read = current
                .read_word_checked(&constant(offset), domain)
                .unwrap();
            assert_eq!(read.expression(), original.expression());
            assert!(
                read.provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
            prove(&relations, &read, U256::from(7), limits);
        }
        assert!(
            !current
                .read_word(&zero(), domain)
                .same_identity(&current.read_word(&constant(32), domain))
        );
        assert_eq!(current.state_root(), &root);
    }
    let new_hash = memory
        .slice(&zero(), &constant(64), 256, domain)
        .unwrap()
        .keccak(domain, 64)
        .unwrap();
    assert_eq!(hash.singleton(), new_hash.singleton());
    assert!(hash.singleton().is_some());
    memory
        .write_word(&zero(), &constant(11), 256, domain)
        .unwrap();
    assert!(old.same_identity(&a));
    let mut historical_relations = RelationState::default();
    for (expression, value) in snapshot.symbolic_facts() {
        historical_relations
            .observe(expression, value.numeric(), &limits)
            .unwrap();
    }
    prove(&historical_relations, &old, U256::from(7), limits);
    assert_eq!(
        snapshot.read_word(&zero(), domain).singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        memory.read_word(&zero(), domain).singleton(),
        Some(U256::from(11))
    );
}

#[test]
fn overlapping_and_symbolic_copy_ranges_keep_local_alternatives() {
    let limits = policy();
    let domain = domain(limits);
    let source = ByteArray::exact(&[0xab; 32]);
    let mut original = ByteArray::memory();
    original
        .write_word(&zero(), &constant(7), 256, domain)
        .unwrap();
    let target = AbstractValue::unsigned_range(U256::ZERO, U256::from(64))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let mut symbolic = original.clone();
    symbolic
        .copy_from(&target, &source, &zero(), &constant(32), 256, domain)
        .unwrap();
    let read = symbolic.read_word_checked(&zero(), domain).unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if &view.state == symbolic.state_root())
    );
    for (at, expected) in [(0, U256::from_be_bytes([0xab; 32])), (64, U256::from(7))] {
        let mut relations = RelationState::default();
        relations
            .observe(
                target.expression().unwrap(),
                constant(at).numeric(),
                &limits,
            )
            .unwrap();
        prove(&relations, &read, expected, limits);
    }
    let (guard, length) = return_length(limits);
    let uncertain = domain
        .join(&zero(), &constant(32))
        .with_expression(length.clone());
    let mut symbolic = original.clone();
    symbolic
        .copy_from(&zero(), &source, &zero(), &uncertain, 256, domain)
        .unwrap();
    let read = symbolic.read_word_checked(&zero(), domain).unwrap();
    assert!(read.singleton().is_none());
    assert!(
        matches!(symbolic.state_root().kind(), ByteStateKind::Copy { length: retained, .. } if retained == &length)
    );
    for (success, expected) in [
        (false, U256::from(7)),
        (true, U256::from_be_bytes([0xab; 32])),
    ] {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&guard, success, &limits),
            CheckResult::MayBeFeasible
        );
        prove(&relations, &read, expected, limits);
    }
    // This demand precedes the destination for every possible copy length.
    let mut disjoint = original.clone();
    disjoint
        .copy_from(&constant(128), &source, &zero(), &uncertain, 256, domain)
        .unwrap();
    assert_eq!(
        disjoint.read_word(&zero(), domain).singleton(),
        Some(U256::from(7))
    );
    assert!(
        matches!(disjoint.state_root().kind(), ByteStateKind::Copy { length: retained, .. } if retained == &length)
    );
    let mut overlapping = original;
    overlapping
        .copy_from(&constant(31), &source, &zero(), &constant(1), 256, domain)
        .unwrap();
    let read = overlapping.read_word_checked(&zero(), domain).unwrap();
    assert!(
        matches!(read.expression().unwrap().kind(), ExprKind::WordRead { view, .. } if &view.state == overlapping.state_root())
    );
    prove(&RelationState::default(), &read, U256::from(0xab), limits);
}

#[test]
fn copy_range_clips_maximum_and_large_sources_stay_lazy() {
    let limits = policy();
    let expression = |n| ExprId::constant(n);
    let source = ByteStateId::exact(Arc::from([0xab, 0xcd]));
    let root = ByteStateId::zero()
        .copy(
            expression(U256::MAX),
            ByteView::new(
                source.clone(),
                expression(U256::ZERO),
                expression(U256::from(2)),
            ),
            expression(U256::from(2)),
            limits.expressions().into(),
        )
        .unwrap();
    assert_eq!(
        ExprId::byte_read(root.clone(), expression(U256::ZERO), limits.expressions())
            .unwrap()
            .as_constant(),
        Some(U256::ZERO)
    );
    let read = ExprId::word_read(
        ByteView::new(root, expression(U256::MAX), expression(U256::from(32))),
        expression(U256::ZERO),
        limits.expressions(),
    )
    .unwrap();
    prove(
        &RelationState::default(),
        &AbstractValue::top().with_expression(read),
        U256::from(0xab) << 248,
        limits,
    );
    let root = ByteStateId::zero()
        .copy(
            expression(U256::from(1024)),
            ByteView::new(
                source.clone(),
                expression(U256::ZERO),
                expression(U256::MAX),
            ),
            expression(U256::MAX),
            limits.expressions().into(),
        )
        .unwrap();
    assert_eq!(root.nodes(), 7);
    assert!(
        matches!(root.kind(), ByteStateKind::Copy { source: view, .. } if view.state == source)
    );
    for (index, expected) in [
        (U256::ZERO, U256::ZERO),
        (U256::from(1024), U256::from(0xab)),
        (U256::MAX, U256::ZERO),
    ] {
        let read =
            ExprId::byte_read(root.clone(), expression(index), limits.expressions()).unwrap();
        prove(
            &RelationState::default(),
            &AbstractValue::top().with_expression(read),
            expected,
            limits,
        );
    }
}

#[test]
fn disjoint_copy_demand_keeps_limits_cancellation_and_atomic_failures() {
    let limits = policy();
    let domain = domain(limits);
    let source = ByteArray::exact(&[0xab; 4]);
    let mut memory = ByteArray::memory();
    memory
        .write_word(&zero(), &constant(7), 256, domain)
        .unwrap();
    memory
        .copy_from(&constant(128), &source, &zero(), &constant(4), 256, domain)
        .unwrap();
    let original = memory.clone();
    let low = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: memory.state_root().nodes() + 3,
        ..limits
    }));
    assert!(matches!(
        memory.read_word_checked(&zero(), low),
        Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    let low = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_depth: 1,
        ..limits
    }));
    assert!(
        memory
            .copy_from(&constant(160), &source, &zero(), &constant(4), 256, low)
            .is_err()
    );
    assert_eq!(memory, original);
    assert!(matches!(
        memory.copy_from(
            &AbstractValue::constant(U256::MAX),
            &source,
            &zero(),
            &constant(4),
            256,
            domain
        ),
        Err(RangeError::OffsetTooLarge(_))
    ));
    assert_eq!(memory, original);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            memory.read_word_checked(&zero(), domain),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
        assert!(
            memory
                .copy_from(&constant(160), &source, &zero(), &constant(4), 256, domain)
                .is_err()
        );
        assert_eq!(memory, original);
        memory
            .copy_from(
                &AbstractValue::constant(U256::MAX),
                &source,
                &AbstractValue::constant(U256::MAX),
                &zero(),
                256,
                domain,
            )
            .unwrap();
        assert_eq!(memory, original);
    });
}
