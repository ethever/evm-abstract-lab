use super::{NUMERIC_READS, eligible};
use alloy_primitives::U256;
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
};
use {
    crate::domain::DomainSpec, crate::domain::NumericValue, crate::domain::facts::UnaryPredicate,
    crate::domain::facts::WordBounds, crate::domain::identity::Symbol,
    crate::domain::provenance::Origin, crate::domain::relational::RelationLimits,
    crate::domain::symbolic::ByteStateKind, crate::domain::symbolic::ExprError,
    crate::domain::symbolic::ExprId, crate::domain::symbolic::ExprKind,
    crate::domain::symbolic::ExprLimits, crate::domain::symbolic::StateError,
    crate::world::RangeError, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestBytes, crate::world::test_support::TestValue,
};

fn constant(n: usize) -> TestValue {
    TestValue::constant(U256::from(n))
}

fn policies() -> [TestAlgebra; 3] {
    [(1, 1, 1, false), (8, 4, 256, true), (64, 1, 32, false)].map(
        |(capacity, rounds, facts, enabled)| {
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
                DomainSpec::new(
                    NonZeroUsize::new(capacity).unwrap(),
                    NonZeroUsize::new(rounds).unwrap(),
                    NonZeroUsize::new(facts).unwrap(),
                )
                .with_relations(RelationLimits {
                    enabled,
                    ..Default::default()
                }),
            ))
        },
    )
}

fn source(numeric: NumericValue, scope: u64) -> TestValue {
    let mut value = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .from_facts(&[UnaryPredicate::IsCodeAddress])
        .unwrap()
        .with_numeric(numeric);
    value.set_origin(Origin::Calldata);
    value
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
        .with_identity(scope, 7)
        .with_expression(ExprId::input(scope, Symbol::CalldataWord(U256::ZERO)))
}

// The original algorithm: unchanged staging followed by the original refresh.
// No reimplementation of BYTE, metadata, expressions or range rules is an oracle.
fn original(
    array: &mut TestBytes,
    offset: &TestValue,
    value: &TestValue,
    width: usize,
    domain: TestAlgebra,
) -> Result<(), RangeError> {
    let mut out = array.staged_write(offset, value, width, domain.borrowed())?;
    out.refresh(domain.borrowed());
    *array = out;
    Ok(())
}

fn assert_complete(actual: &TestBytes, expected: &TestBytes, domain: TestAlgebra) {
    assert_eq!(actual, expected);
    assert_eq!(actual.view(), expected.view());
    assert_eq!(actual.state_root(), expected.state_root());
    assert_eq!(
        actual.symbolic_facts().collect::<Vec<_>>(),
        expected.symbolic_facts().collect::<Vec<_>>()
    );
    for index in 0..=32 {
        assert_eq!(
            actual.read_byte_checked(&constant(index), domain.borrowed()),
            expected.read_byte_checked(&constant(index), domain.borrowed()),
            "byte {index}"
        );
    }
    assert_eq!(
        actual.read_word_checked(&constant(0), domain.borrowed()),
        expected.read_word_checked(&constant(0), domain.borrowed())
    );
}

#[test]
fn a1_matches_entire_original_arrays_and_reads_under_the_same_policy() {
    let _reads = NUMERIC_READS.scope();
    // These fixtures deliberately retain Top finite/bits even for a bounded
    // range or a nontrivial odd congruence, matching probe16's admitted shape.
    let constrained = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(
                WordBounds::new(U256::ONE, U256::MAX - U256::ONE).unwrap(),
            ),
            UnaryPredicate::congruent(U256::from(3), U256::ONE).unwrap(),
        ])
        .unwrap();
    assert!(!constrained.congruence().is_top());
    assert_ne!(
        constrained.interval().unsigned_bounds(),
        (U256::ZERO, U256::MAX)
    );
    for domain in policies() {
        for numeric in [
            NumericValue::top(),
            NumericValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap(),
            constrained.numeric().clone(),
        ] {
            let value = source(numeric, 73);
            assert!(value.provenance().is_code_address());
            let before = TestBytes::memory();
            assert!(eligible(
                &before,
                &constant(0),
                &value,
                32,
                domain.borrowed()
            ));
            let mut actual = before.clone();
            let mut expected = before.clone();
            NUMERIC_READS.set(0);
            actual
                .write_word(&constant(0), &value, domain.borrowed())
                .unwrap();
            assert_eq!(NUMERIC_READS.get(), 1);
            NUMERIC_READS.set(0);
            original(&mut expected, &constant(0), &value, 32, domain).unwrap();
            assert_eq!(NUMERIC_READS.get(), 32);
            assert_complete(&actual, &expected, domain);
            let ByteStateKind::WriteWord {
                previous,
                offset,
                value: written,
            } = actual.root.kind()
            else {
                panic!("the complete word root must remain authoritative")
            };
            assert_eq!(previous, before.state_root());
            assert_eq!(offset.as_constant(), Some(U256::ZERO));
            assert_eq!(Some(written), value.expression());
            assert_eq!(actual.operands.len(), 1);
            let retained = actual.operands.get(written).unwrap();
            assert_eq!(retained, &value);
            assert!(!retained.same_identity(&TestValue::top().with_identity(73, 7)));
            let mut expressions = BTreeSet::new();
            for (index, byte) in &actual.bytes {
                assert!(!byte.symbolic_limit_reached());
                assert!(!byte.same_identity(byte));
                assert!(byte.identity().input().is_none());
                assert!(!byte.provenance().is_code_address());
                assert!(
                    byte.provenance()
                        .origins()
                        .sources()
                        .unwrap()
                        .contains(&Origin::Arithmetic)
                );
                let expression = byte.expression().unwrap();
                assert!(
                    matches!(expression.kind(), ExprKind::ByteRead { state, index: at }
                    if state == actual.state_root() && at.as_constant() == Some(U256::from(*index)))
                );
                expressions.insert(expression.clone());
            }
            assert_eq!(expressions.len(), 32);
            assert_eq!(before, TestBytes::memory());
        }
    }
}

#[test]
fn quote_and_execution_admit_the_same_shape_before_and_after_touch() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value = source(NumericValue::top(), 81);
    let mut memory = TestBytes::memory();
    let untouched = memory.clone();
    assert!(
        memory
            .uniform_word_write_work(&constant(0), &value, domain.borrowed())
            .is_some()
    );
    memory
        .expand(&constant(0), &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(memory.allocations.len(), 1);
    assert!(
        memory
            .uniform_word_write_work(&constant(0), &value, domain.borrowed())
            .is_some()
    );
    let mut expected = memory.clone();
    original(&mut expected, &constant(0), &value, 32, domain).unwrap();
    memory
        .write_word(&constant(0), &value, domain.borrowed())
        .unwrap();
    assert_complete(&memory, &expected, domain);

    let saved = memory.clone();
    let expected_saved = expected.clone();
    memory
        .write_byte(&constant(64), &constant(42), domain.borrowed())
        .unwrap();
    original(&mut expected, &constant(64), &constant(42), 1, domain).unwrap();
    assert_complete(&memory, &expected, domain);
    assert_complete(&saved, &expected_saved, domain);
    assert!(
        matches!(memory.root.kind(), ByteStateKind::WriteByte { previous, .. }
        if previous == saved.state_root())
    );
    assert_eq!(untouched, TestBytes::memory());
}

#[test]
fn numeric_only_length_expression_nodes_match_cached_quote_bound() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value = source(NumericValue::top(), 82);
    let lengths = [
        TestValue::unsigned_range(U256::ZERO, U256::from(64)).unwrap(),
        TestValue::top().with_numeric(domain.join(&constant(0), &constant(16)).numeric().clone()),
    ];
    assert!(lengths[0].finite_constants().is_top());
    assert_eq!(lengths[1].constants().unwrap().len(), 2);
    for length in lengths {
        assert!(length.expression().is_none());
        assert!(!length.numeric().is_top());
        assert_eq!(
            super::super::expr(&length).unwrap().nodes(),
            length.numeric().work_size().saturating_add(5)
        );
        let mut memory = TestBytes::memory();
        memory.length = length;
        assert!(
            memory
                .uniform_word_write_work(&constant(0), &value, domain.borrowed())
                .is_some()
        );
    }
}

#[test]
fn nonliteral_msize_and_allocation_history_keep_quote_touch_and_write_in_agreement() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value = source(NumericValue::top(), 82);
    for length in [
        domain.join(&constant(0), &constant(16)),
        domain.join(&constant(0), &constant(64)),
        TestValue::top().with_expression(ExprId::fresh().unwrap()),
    ] {
        let mut before = TestBytes::memory();
        before.length = length;
        assert!(
            before
                .uniform_word_write_work(&constant(0), &value, domain.borrowed())
                .is_some()
        );
        let mut actual = before.clone();
        actual
            .expand(&constant(0), &constant(32), domain.borrowed())
            .unwrap();
        assert!(
            actual
                .uniform_word_write_work(&constant(0), &value, domain.borrowed())
                .is_some()
        );
        let mut expected = actual.clone();
        original(&mut expected, &constant(0), &value, 32, domain).unwrap();
        actual
            .write_word(&constant(0), &value, domain.borrowed())
            .unwrap();
        assert_complete(&actual, &expected, domain);
        // The standalone writer also pays/executes the original expansion.
        // Share the intended MSIZE source before independent staging creates
        // observed-slot identities. MSIZE does not participate in eligibility.
        before.length = super::super::ranges::normalized(&before.length).unwrap();
        assert!(
            before
                .uniform_word_write_work(&constant(0), &value, domain.borrowed())
                .is_some()
        );
        let mut direct = before.clone();
        original(&mut before, &constant(0), &value, 32, domain).unwrap();
        direct
            .write_word(&constant(0), &value, domain.borrowed())
            .unwrap();
        assert_complete(&direct, &before, domain);
    }
    let mut history = TestBytes::memory();
    history
        .expand(&constant(64), &constant(32), domain.borrowed())
        .unwrap();
    let symbolic = TestValue::unsigned_range(U256::ZERO, U256::from(32))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    history
        .expand(&symbolic, &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(history.allocations.len(), 2);
    assert!(
        history
            .uniform_word_write_work(&constant(0), &value, domain.borrowed())
            .is_some()
    );
    let mut expected = history.clone();
    original(&mut expected, &constant(0), &value, 32, domain).unwrap();
    history
        .write_word(&constant(0), &value, domain.borrowed())
        .unwrap();
    assert_complete(&history, &expected, domain);
    assert_eq!(history.allocations.len(), 3);

    let saturated = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(1).unwrap(),
        ),
    ));
    assert_eq!(
        TestBytes::memory().uniform_word_write_work(&constant(0), &value, saturated.borrowed()),
        Some(usize::MAX)
    );
}

fn fallback(
    array: TestBytes,
    offset: TestValue,
    value: TestValue,
    width: usize,
    domain: TestAlgebra,
) {
    assert!(!eligible(&array, &offset, &value, width, domain.borrowed()));
    if width == 32 {
        assert_eq!(
            array.uniform_word_write_work(&offset, &value, domain.borrowed()),
            None
        );
    }
    let mut actual = array.clone();
    let mut expected = array;
    assert_eq!(
        actual.write(&offset, &value, width, domain.borrowed()),
        original(&mut expected, &offset, &value, width, domain)
    );
    assert_complete(&actual, &expected, domain);
}

#[test]
fn nonuniform_sources_and_nonfresh_or_guarded_shapes_keep_the_original_path() {
    let _reads = NUMERIC_READS.scope();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value = source(NumericValue::top(), 91);
    let finite = domain
        .join(&constant(1), &constant(2))
        .with_expression(value.expression().unwrap().clone());
    let ite = value.clone().with_expression(
        ExprId::ite(
            ExprId::fresh().unwrap(),
            ExprId::fresh().unwrap(),
            ExprId::fresh().unwrap(),
            ExprLimits::default(),
        )
        .unwrap(),
    );
    for rejected in [
        source(NumericValue::unknown_byte(), 92),
        source(NumericValue::constant(U256::ONE), 92),
        source(
            NumericValue::unsigned_range(U256::from(17), U256::from(17)).unwrap(),
            92,
        ),
        finite,
        ite,
        value.clone().with_expression(ExprId::constant(U256::ZERO)),
    ] {
        fallback(TestBytes::memory(), constant(0), rejected, 32, domain);
    }

    // Keep missing-Expr rejection: prebinding this input would enable A1.
    // Independent staging instead creates one source leaf per invocation.
    let unbound = TestValue::top();
    let before = TestBytes::memory();
    let offset = constant(0);
    let canonical_source = ExprId::fresh().unwrap();
    assert!(unbound.expression().is_none());
    assert!(!eligible(&before, &offset, &unbound, 32, domain.borrowed()));
    assert_eq!(
        before.uniform_word_write_work(&offset, &unbound, domain.borrowed()),
        None
    );
    assert!(eligible(
        &before,
        &offset,
        &unbound.clone().with_expression(canonical_source.clone()),
        32,
        domain.borrowed(),
    ));
    assert!(before.fresh_leaves().is_empty());
    let mut actual = before.clone();
    let mut expected = before.clone();
    NUMERIC_READS.set(0);
    actual
        .write_word(&offset, &unbound, domain.borrowed())
        .unwrap();
    assert_eq!(NUMERIC_READS.get(), 32);
    NUMERIC_READS.set(0);
    original(&mut expected, &offset, &unbound, 32, domain).unwrap();
    assert_eq!(NUMERIC_READS.get(), 32);
    for array in [&mut actual, &mut expected] {
        let leaves = array.fresh_leaves();
        assert_eq!(leaves.len(), 1);
        let generated = leaves.into_iter().next().unwrap();
        let ByteStateKind::WriteWord {
            previous,
            offset,
            value: written,
        } = array.root.kind()
        else {
            panic!("the unbound source must retain the original word root")
        };
        assert_eq!(previous, before.state_root());
        assert_eq!(offset.as_constant(), Some(U256::ZERO));
        assert_eq!(written, &generated);
        assert_eq!(array.operands.len(), 1);
        assert_eq!(array.operands.get(&generated), Some(&unbound));
        assert_eq!(array.bytes.len(), 32);
        for byte in array.bytes.values() {
            assert!(
                matches!(byte.expression().unwrap().kind(), ExprKind::ByteRead { state, .. }
                if state == array.state_root())
            );
        }
        // One bijection spans every root, value, fact and allocation. Exact
        // inverse equality also guards against metadata loss during renaming.
        let saved = array.clone();
        array
            .rename_fresh(
                &BTreeMap::from([(generated.clone(), canonical_source.clone())]),
                super::super::limits(domain.borrowed()),
                domain.borrowed(),
            )
            .unwrap();
        let mut restored = array.clone();
        restored
            .rename_fresh(
                &BTreeMap::from([(canonical_source.clone(), generated)]),
                super::super::limits(domain.borrowed()),
                domain.borrowed(),
            )
            .unwrap();
        assert_eq!(restored, saved);
    }
    assert_complete(&actual, &expected, domain);
    assert_eq!(before, TestBytes::memory());

    for offset in [
        constant(1),
        constant(0).with_origin(Origin::Storage),
        constant(0).with_expression(ExprId::fresh().unwrap()),
    ] {
        fallback(TestBytes::memory(), offset, value.clone(), 32, domain);
    }
    let mut with_bytes = TestBytes::memory();
    with_bytes.bytes.insert(0, constant(0));
    let mut with_facts = TestBytes::memory();
    with_facts
        .remember(
            value.expression().unwrap().clone(),
            &value,
            domain.borrowed(),
        )
        .unwrap();
    let guarded = with_facts
        .join_guarded(
            &TestBytes::memory(),
            domain.borrowed(),
            ExprId::fresh().unwrap(),
            ExprLimits::default(),
        )
        .unwrap();
    assert!(guarded.conditional.len() > 0);
    let mut written = TestBytes::memory();
    written
        .write_byte(&constant(64), &constant(42), domain.borrowed())
        .unwrap();
    for array in [TestBytes::empty(), with_bytes, with_facts, guarded, written] {
        fallback(array, constant(0), value.clone(), 32, domain);
    }
    fallback(TestBytes::memory(), constant(0), value.clone(), 1, domain);
    fallback(
        TestBytes::memory(),
        constant(0),
        value,
        32,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
            crate::domain::DomainSpec::new(
                NonZeroUsize::new(8).unwrap(),
                std::num::NonZeroUsize::new(8).unwrap(),
                std::num::NonZeroUsize::new(256).unwrap(),
            ),
        )),
    );
}

#[test]
fn the_same_root_with_different_source_facts_gets_independent_numeric_results() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let expression = ExprId::operation(
        revm_bytecode::opcode::NOT,
        &[ExprId::fresh().unwrap()],
        ExprLimits::default(),
    )
    .unwrap();
    let mut snapshots = Vec::new();
    for origin in [Origin::Calldata, Origin::Storage] {
        let mut value = TestValue::top().with_expression(expression.clone());
        value.set_origin(origin);
        let mut actual = TestBytes::memory();
        let mut expected = actual.clone();
        original(&mut expected, &constant(0), &value, 32, domain).unwrap();
        actual
            .write_word(&constant(0), &value, domain.borrowed())
            .unwrap();
        assert_complete(&actual, &expected, domain);
        snapshots.push(actual);
    }
    assert_eq!(snapshots[0].root, snapshots[1].root);
    assert_ne!(snapshots[0].bytes, snapshots[1].bytes);
    assert!(
        !snapshots[0]
            .byte_at(0, domain.borrowed())
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Storage)
    );
    assert!(
        !snapshots[1]
            .byte_at(0, domain.borrowed())
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Calldata)
    );
}

#[test]
fn staged_limits_flags_and_cancel_keep_full_wrappers_atomic_and_prepaid_work() {
    let normal = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value = source(NumericValue::top(), 101);
    let mut touched = TestBytes::memory();
    touched
        .expand(&constant(0), &constant(32), normal.borrowed())
        .unwrap();
    for (max_nodes, max_depth) in [(1, 64), (3, 64), (1024, 1)] {
        let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
            normal.spec().with_relations(RelationLimits {
                max_nodes,
                max_depth,
                ..normal.spec().relations()
            }),
        ));
        let mut actual = touched.clone();
        let mut expected = touched.clone();
        let result = actual.write_word(&constant(0), &value, domain.borrowed());
        assert!(matches!(
            result,
            Err(RangeError::State(StateError::Limit { .. }))
        ));
        assert_eq!(
            result,
            original(&mut expected, &constant(0), &value, 32, domain)
        );
        assert_eq!(actual, touched);
        assert_eq!(expected, touched);
    }
    let mut actual = touched.clone();
    let mut expected = touched.clone();
    let limited = value.clone().with_symbolic_limit();
    assert_eq!(
        actual.write_word(&constant(0), &limited, normal.borrowed()),
        original(&mut expected, &constant(0), &limited, 32, normal)
    );
    assert_eq!(actual, touched);
    assert_eq!(expected, touched);

    // Preserve the old marker behavior when staging fits but byte expressions
    // cannot be retained. There is no successful partial numeric projection.
    let read_limit = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        normal.spec().with_relations(RelationLimits {
            max_nodes: 4,
            ..normal.spec().relations()
        }),
    ));
    assert_eq!(
        actual.write_word(&constant(0), &value, read_limit.borrowed()),
        original(&mut expected, &constant(0), &value, 32, read_limit)
    );
    assert_eq!(actual, expected);
    assert!(actual.bytes.values().all(TestValue::symbolic_limit_reached));

    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let fee = touched
            .uniform_word_write_work(&constant(0), &value, normal.borrowed())
            .unwrap();
        let mut budget = crate::resource::WorkBudget::new(fee);
        assert!(budget.charge(fee));
        cancellation.cancel();
        let mut actual = touched.clone();
        let mut expected = touched.clone();
        let result = actual.write_word(&constant(0), &value, normal.borrowed());
        assert_eq!(result, Err(RangeError::Expression(ExprError::Cancelled)));
        assert_eq!(
            result,
            original(&mut expected, &constant(0), &value, 32, normal)
        );
        assert_eq!(actual, touched);
        assert_eq!(expected, touched);
        assert_eq!(budget.used(), fee);
    });
}
