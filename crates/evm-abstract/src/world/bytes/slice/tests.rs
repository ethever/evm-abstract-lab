//! Slice presentation certificates are checked against authoritative reads.

use super::*;
use crate::{
    domain::{
        identity::Symbol,
        provenance::Origin,
        symbolic::{ByteStateId, ExprLimits, SourceId, StateBinding, StateLimits},
    },
    world::bytes::{conditional, constant},
};

fn observed(length: TestValue) -> TestBytes {
    let mut source = TestBytes::unknown();
    let length_expr = ExprId::fresh().unwrap();
    source.length = length.clone().with_expression(length_expr.clone());
    source.operands.insert(length_expr.clone(), length);
    source.root = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        length_expr,
        vec![StateBinding {
            index: U256::from(7),
            value: ExprId::constant(U256::from(0xaa)),
            numeric: constant::<crate::domain::NumericValue>(0xaa)
                .numeric()
                .clone()
                .into(),
        }],
        StateLimits::default(),
    )
    .unwrap();
    // An observed binding can be outside the actual length. This raw cell is
    // deliberately different from a padded authoritative read.
    source.bytes.insert(7, constant(0xaa));
    source
}

#[test]
fn exact_offsets_translate_certified_cells_against_the_byte_oracle() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let source = TestBytes::exact(&[0xaa, 0, 0xbb, 0xcc]);
    for start in [
        U256::ZERO,
        U256::ONE,
        U256::from(3),
        U256::from(4),
        U256::MAX,
    ] {
        for length in [U256::ONE, U256::from(4), U256::from(10), U256::MAX] {
            let output = source
                .slice(
                    &TestValue::constant(start),
                    &TestValue::constant(length),
                    domain.borrowed(),
                )
                .unwrap();
            assert_eq!(output.len().singleton(), Some(length));
            for (index, value) in output.stored_bytes() {
                assert!(U256::from(index) < length);
                let at = start.checked_add(U256::from(index)).unwrap();
                let oracle = source
                    .read_byte_checked(&TestValue::constant(at), domain.borrowed())
                    .unwrap();
                assert_eq!(value, &oracle);
                let mut observed = output.byte_at(index, domain.borrowed());
                // A slice read may retain its view term even when the exact
                // source folds to a constant; every value layer still agrees.
                observed.forget_expression();
                assert_eq!(observed, oracle);
            }
            for index in 0..12 {
                let expected = if U256::from(index) >= length {
                    zero()
                } else if let Some(at) = start.checked_add(U256::from(index)) {
                    source
                        .read_byte_checked(&TestValue::constant(at), domain.borrowed())
                        .unwrap()
                } else {
                    zero()
                };
                assert_eq!(
                    output.byte_at(index, domain.borrowed()).numeric(),
                    expected.numeric()
                );
            }
        }
    }
    let output = source
        .slice(&constant(1), &constant(3), domain.borrowed())
        .unwrap();
    assert_eq!(
        output.stored_bytes().map(|(i, _)| i).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn observed_input_bounds_apply_source_padding_before_cache_transfer() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    for (length, expected) in [
        (
            TestValue::unsigned_range(U256::ZERO, U256::from(6)).unwrap(),
            vec![0],
        ),
        (
            TestValue::unsigned_range(U256::from(8), U256::from(16)).unwrap(),
            vec![0xaa],
        ),
        (
            TestValue::unsigned_range(U256::from(6), U256::from(8)).unwrap(),
            vec![0, 0xaa],
        ),
        (TestValue::top(), vec![0, 0xaa]),
    ] {
        let source = observed(length);
        let output = source
            .slice(&constant(2), &constant(12), domain.borrowed())
            .unwrap();
        let cached = output
            .bytes
            .get(&5)
            .expect("locally certified observed cell");
        assert_eq!(
            cached.numeric(),
            source.byte_at(7, domain.borrowed()).numeric()
        );
        assert_eq!(
            cached.numeric(),
            output.byte_at(5, domain.borrowed()).numeric()
        );
        for byte in expected {
            assert!(cached.contains(U256::from(byte)));
        }
        assert!(output.default_byte().contains(U256::ZERO));
    }
}

#[test]
fn expanding_wrapper_length_does_not_unpad_the_captured_input_root() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut source = observed(TestValue::unsigned_range(U256::ZERO, U256::from(6)).unwrap());
    let root = source.state_root().clone();
    source
        .expand(&zero(), &constant(8), domain.borrowed())
        .unwrap();
    assert_eq!(source.state_root(), &root);
    assert!(source.len().interval().unsigned_bounds().0 >= U256::from(8));
    assert_eq!(
        source.byte_at(7, domain.borrowed()).singleton(),
        Some(U256::ZERO)
    );
    let output = source
        .slice(&constant(2), &constant(10), domain.borrowed())
        .unwrap();
    assert_eq!(output.bytes.get(&5).unwrap().singleton(), Some(U256::ZERO));
    assert_eq!(
        output.byte_at(5, domain.borrowed()).singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(output.operands, source.operands);
}

#[test]
fn uncertain_output_lengths_include_padding_and_retain_the_requested_term() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    for (lo, hi) in [(0, 3), (1, 3), (4, 8)] {
        let size = TestValue::unsigned_range(U256::from(lo), U256::from(hi))
            .unwrap()
            .with_expression(ExprId::fresh().unwrap());
        let mut source = TestBytes::exact(&[0xaa; 8]);
        source
            .operands
            .insert(size.expression().unwrap().clone(), size.clone());
        let output = source
            .slice(&constant(2), &size, domain.borrowed())
            .unwrap();
        assert_eq!(output.len(), &size);
        assert_eq!(output.operands, source.operands);
        for (index, cached) in output.stored_bytes() {
            assert!(index < hi);
            let oracle = output
                .read_byte_checked(&constant(index), domain.borrowed())
                .unwrap();
            assert_eq!(cached.numeric(), oracle.numeric());
            assert!(cached.contains(U256::from(0xaa)));
            assert_eq!(cached.may_be_zero(), index >= lo);
            if index >= lo {
                assert!(cached.expression().is_none());
                assert!(cached.identity().input().is_none());
            }
        }
        assert!(!output.bytes.is_empty());
    }
}

#[test]
fn symbolic_reads_ignore_the_presentation_cache_after_slicing() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let pristine = TestBytes::exact(&[0xaa, 0xbb, 0xcc]);
    let mut poisoned = pristine.clone();
    poisoned.bytes.insert(1, constant(0x11));
    poisoned.default = constant(0x22);
    let output = poisoned
        .slice(&constant(1), &constant(3), domain.borrowed())
        .unwrap();
    let expected = pristine
        .slice(&constant(1), &constant(3), domain.borrowed())
        .unwrap();
    assert!(output.bytes.get(&0).unwrap().contains(U256::from(0xbb)));
    assert_eq!(output.view(), expected.view());
    assert_eq!(
        output.exact_bytes(
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        Some(vec![0xbb, 0xcc, 0])
    );
    assert_eq!(
        output.summary_projection(domain.borrowed()).unwrap(),
        expected.summary_projection(domain.borrowed()).unwrap()
    );
    for index in 0..5 {
        assert_eq!(
            output.byte_at(index, domain.borrowed()),
            expected.byte_at(index, domain.borrowed())
        );
    }
}

#[test]
fn changed_facts_do_not_certify_stale_numeric_or_promote_branch_facts() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let value_expr = ExprId::fresh().unwrap();
    let mut source = TestBytes::memory();
    source
        .write_word(
            &zero(),
            &constant(7).with_expression(value_expr.clone()),
            domain.borrowed(),
        )
        .unwrap();
    source.operands.insert(
        value_expr.clone(),
        constant(20).with_expression(value_expr.clone()),
    );
    let output = source
        .slice(&zero(), &constant(32), domain.borrowed())
        .unwrap();
    assert_eq!(
        output.read_word(&zero(), domain.borrowed()).singleton(),
        Some(U256::from(20))
    );
    assert_eq!(
        output.operands.get(&value_expr),
        source.operands.get(&value_expr)
    );

    let mut other = TestBytes::memory();
    other
        .write_byte(&constant(31), &constant(9), domain.borrowed())
        .unwrap();
    let guard = ExprId::fresh().unwrap();
    let joined = source
        .join_guarded(
            &other,
            domain.borrowed(),
            guard.clone(),
            ExprLimits::default(),
        )
        .unwrap();
    let output = joined
        .slice(&zero(), &constant(32), domain.borrowed())
        .unwrap();
    assert!(!output.operands.contains_key(&value_expr));
    assert_eq!(output.conditional, joined.conditional);
    for nonzero in [false, true] {
        let context = conditional::Context::new(
            &output.operands,
            Some(&output.conditional),
            domain.borrowed(),
        )
        .branch(&guard, nonzero);
        assert_eq!(
            context.value(&value_expr, domain.borrowed()).is_some(),
            nonzero
        );
    }
    for (nonzero, expected) in [(true, 20), (false, 9)] {
        let mut selected = output.clone();
        selected.operands.insert(
            guard.clone(),
            constant(usize::from(nonzero)).with_expression(guard.clone()),
        );
        assert_eq!(
            selected.read_word(&zero(), domain.borrowed()).singleton(),
            Some(U256::from(expected))
        );
        assert_eq!(selected.root, output.root);
    }
}

#[test]
fn range_context_changes_omit_cached_values_and_unknown_offsets_omit_coordinates() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let source = TestBytes::exact(&[0xaa, 0xbb]);
    let offset = constant(1).with_origin(Origin::CallValue);
    let output = source
        .slice(&offset, &constant(1), domain.borrowed())
        .unwrap();
    assert!(output.bytes.get(&0).unwrap().contains(U256::from(0xbb)));
    assert_eq!(
        output.byte_at(0, domain.borrowed()).singleton(),
        Some(U256::from(0xbb))
    );
    let offset = TestValue::unsigned_range(U256::ZERO, U256::ONE)
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let output = source
        .slice(&offset, &constant(1), domain.borrowed())
        .unwrap();
    assert!(output.bytes.is_empty());
    assert!(output.default_byte().contains(U256::ZERO));
    for byte in [0xaa, 0xbb] {
        assert!(
            output
                .byte_at(0, domain.borrowed())
                .contains(U256::from(byte))
        );
    }
}

#[test]
fn uncertified_coordinates_still_drive_authoritative_summary_projection() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut source = TestBytes::memory();
    source
        .write_byte(&constant(7), &constant(0xaa), domain.borrowed())
        .unwrap();
    source
        .expand(&zero(), &constant(512), domain.borrowed())
        .unwrap();
    let output = source
        .slice(&zero(), &constant(512), domain.borrowed())
        .unwrap();
    assert!(output.bytes.contains_key(&7));
    let independent = output
        .summary_projection(domain.borrowed())
        .unwrap()
        .instantiate(domain.borrowed())
        .unwrap();
    assert_eq!(independent.len().singleton(), Some(U256::from(512)));
    assert_eq!(
        independent.byte_at(7, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(
        output.byte_at(7, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
}

#[test]
fn captured_source_history_and_observed_provenance_survive_later_writes() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut value = TestValue::top().with_symbol(Symbol::CalldataWord(U256::ZERO), Some(1701));
    value.set_origin(Origin::Calldata);
    value = value.with_numeric(constant(7).numeric().clone());
    let mut source = TestBytes::memory();
    source
        .write_word(&zero(), &value, domain.borrowed())
        .unwrap();
    let history = source
        .read_word_checked(&zero(), domain.borrowed())
        .unwrap();
    let output = source
        .slice(&zero(), &constant(32), domain.borrowed())
        .unwrap();
    let view = output.view();
    source
        .write_word(&zero(), &constant(11), domain.borrowed())
        .unwrap();
    let retained = output
        .read_word_checked(&zero(), domain.borrowed())
        .unwrap();
    assert_eq!(retained.singleton(), Some(U256::from(7)));
    assert_eq!(retained.expression(), history.expression());
    assert_eq!(retained.provenance(), history.provenance());
    assert!(retained.same_identity(&value));
    assert!(
        retained
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Calldata)
    );
    assert_eq!(output.view(), view);
    assert_eq!(
        source.read_word(&zero(), domain.borrowed()).singleton(),
        Some(U256::from(11))
    );
}

#[test]
fn sliced_one_byte_return_preserves_the_strict_aa_bb_suffix() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut prior = vec![0xaa; 32];
    prior.extend([0xbb; 32]);
    let mut memory = TestBytes::memory();
    memory
        .copy_from(
            &constant(32),
            &TestBytes::exact(&prior),
            &zero(),
            &constant(64),
            domain.borrowed(),
        )
        .unwrap();
    let returned = TestBytes::exact(&[0x11, 0x22])
        .slice(&zero(), &constant(1), domain.borrowed())
        .unwrap();
    memory
        .copy_return_data(&constant(32), &returned, &constant(16), domain.borrowed())
        .unwrap();
    let output = memory
        .slice(&constant(32), &constant(64), domain.borrowed())
        .unwrap();
    let mut expected = vec![0x11];
    expected.extend([0xaa; 31]);
    expected.extend([0xbb; 32]);
    assert_eq!(output.len().singleton(), Some(U256::from(64)));
    assert_eq!(
        output.exact_bytes(
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed()
        ),
        Some(expected)
    );
    assert_eq!(memory.len().singleton(), Some(U256::from(96)));
}

#[test]
fn explicitly_certified_current_root_bytes_keep_complete_metadata() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut source = TestBytes::unknown();
    source.length = constant(8);
    source.root = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(8)));
    let value = source
        .read_byte_checked(&constant(3), domain.borrowed())
        .unwrap();
    assert!(matches!(
        value.expression().unwrap().kind(),
        ExprKind::ByteRead { .. }
    ));
    source
        .operands
        .insert(value.expression().unwrap().clone(), value.clone());
    source.bytes.insert(3, value.clone());
    let output = source
        .slice(&constant(2), &constant(4), domain.borrowed())
        .unwrap();
    assert_eq!(output.bytes.get(&1), Some(&value));
    assert_eq!(output.operands, source.operands);
    assert_eq!(
        output.byte_at(1, domain.borrowed()).numeric(),
        value.numeric()
    );
}

#[test]
fn retention_and_its_precharge_are_independent_of_logical_length() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let source = TestBytes::exact(&[0xaa; 512]);
    let short = constant(512);
    let huge = TestValue::constant(U256::MAX);
    assert_eq!(
        source.slice_work(&zero(), &short, domain.borrowed()),
        source.slice_work(&zero(), &huge, domain.borrowed())
    );
    let output = source.slice(&zero(), &huge, domain.borrowed()).unwrap();
    assert_eq!(output.len().singleton(), Some(U256::MAX));
    assert_eq!(output.stored_bytes().len(), COPY_FACT_LIMIT);
    assert!(output.default_byte().contains(U256::ZERO));
    assert!(output.default_byte().contains(U256::from(0xaa)));
    assert_eq!(
        output.byte_at(511, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(
        output.byte_at(512, domain.borrowed()).singleton(),
        Some(U256::ZERO)
    );
}
