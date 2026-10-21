//! Whole-word forwarding retains complete values only inside proved windows.

use super::{Facts, Reader};
use alloy_primitives::U256;
use std::collections::BTreeMap;
use {
    crate::domain::identity::Symbol, crate::domain::provenance::Origin,
    crate::domain::symbolic::ByteStateId, crate::domain::symbolic::ByteView,
    crate::domain::symbolic::ExprError, crate::domain::symbolic::ExprId,
    crate::domain::symbolic::ExprLimits, crate::domain::symbolic::StateLimits,
    crate::world::RangeError, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestBytes, crate::world::test_support::TestValue,
};

fn value(n: usize) -> TestValue {
    TestValue::constant(U256::from(n))
}

fn bounded(scope: u64, lower: usize, upper: usize, origin: Origin) -> TestValue {
    let mut out = TestValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap();
    out.set_origin(origin);
    out.with_symbol(Symbol::CalldataWord(U256::ZERO), Some(scope))
}

fn source(input: &TestValue, domain: TestAlgebra) -> TestBytes {
    let mut out = TestBytes::memory();
    out.write_word(&value(64), input, domain.borrowed())
        .unwrap();
    out
}

#[test]
fn slice_copy_and_captured_self_copy_keep_the_full_bounded_word() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let input = bounded(7001, 3, 9, Origin::Returndata);
    let original = source(&input, domain);
    let expected = original
        .read_word_checked(&value(64), domain.borrowed())
        .unwrap();
    assert_eq!(expected, input);
    let view = original
        .slice(&value(32), &value(96), domain.borrowed())
        .unwrap();
    assert_eq!(
        view.read_word_checked(&value(32), domain.borrowed())
            .unwrap(),
        expected
    );
    let nested = view
        .slice(&value(16), &value(64), domain.borrowed())
        .unwrap();
    assert_eq!(
        nested
            .read_word_checked(&value(16), domain.borrowed())
            .unwrap(),
        expected
    );

    let mut copied = TestBytes::memory();
    copied
        .copy_from(
            &value(96),
            &nested,
            &value(16),
            &value(48),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(
        copied
            .read_word_checked(&value(96), domain.borrowed())
            .unwrap(),
        expected
    );
    let snapshot = copied.clone();
    copied
        .copy_from(
            &value(0),
            &snapshot,
            &value(96),
            &value(32),
            domain.borrowed(),
        )
        .unwrap();
    let read = copied
        .read_word_checked(&value(0), domain.borrowed())
        .unwrap();
    assert_eq!(read, expected);
    assert!(read.same_identity(&input));
    assert!(read.contains(U256::from(3)) && read.contains(U256::from(9)));
    assert!(!read.contains(U256::from(2)) && !read.contains(U256::from(10)));
    copied
        .write_word(&value(96), &value(10), domain.borrowed())
        .unwrap();
    assert_eq!(
        copied
            .read_word_checked(&value(0), domain.borrowed())
            .unwrap(),
        expected
    );
    assert_eq!(
        snapshot
            .read_word_checked(&value(96), domain.borrowed())
            .unwrap(),
        expected
    );
}

#[test]
fn guard_selected_window_bounds_and_values_do_not_escape_their_context() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = ExprLimits::default();
    let guard = ExprId::fresh().unwrap();
    // The two snapshots share content terms but have distinct guarded bounds.
    let token = ExprId::fresh().unwrap();
    let length = ExprId::fresh().unwrap();
    let make = |lower, upper, len| {
        let input = bounded(7002, lower, upper, Origin::Calldata).with_expression(token.clone());
        let size = value(len).with_expression(length.clone());
        source(&input, domain)
            .slice(&value(64), &size, domain.borrowed())
            .unwrap()
    };
    let yes = make(3, 9, 32);
    let no = make(11, 17, 31);
    let joined = yes
        .join_guarded(&no, domain.borrowed(), guard.clone(), limits)
        .unwrap();
    let frozen = joined.clone();
    let unselected = joined
        .read_word_checked(&value(0), domain.borrowed())
        .unwrap();
    for n in [0, 3, 9] {
        assert!(unselected.contains(U256::from(n)), "{unselected:?}");
    }
    for nonzero in [true, false, true] {
        let mut selected = joined.clone();
        assert!(
            selected
                .refine_guards(domain.borrowed(), limits, &mut |condition| {
                    Ok::<_, std::convert::Infallible>((condition == &guard).then_some(nonzero))
                })
                .unwrap()
        );
        let read = selected
            .read_word_checked(&value(0), domain.borrowed())
            .unwrap();
        if nonzero {
            assert!(read.contains(U256::from(3)) && read.contains(U256::from(9)));
            assert!(!read.contains(U256::from(2)) && !read.contains(U256::from(10)));
            assert_eq!(
                read.provenance(),
                yes.read_word_checked(&value(0), domain.borrowed())
                    .unwrap()
                    .provenance()
            );
        } else {
            // The last byte is padding even when the other BYTE summaries
            // cannot recover all of the source interval's leading zeros.
            assert!(read.contains(U256::ZERO));
            assert!(!read.contains(U256::ONE));
            assert!(read.identity().input().is_none());
        }
        assert_eq!(selected.state_root(), joined.state_root());
        assert_eq!(joined, frozen);
        assert_eq!(
            joined
                .read_word_checked(&value(0), domain.borrowed())
                .unwrap(),
            unselected
        );
    }
}

#[test]
fn copied_guarded_words_keep_whole_value_joins_and_separate_branch_readers() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = ExprLimits::default();
    let guard = ExprId::fresh().unwrap();
    let a = bounded(7010, 3, 9, Origin::Calldata);
    let b = bounded(7011, 11, 17, Origin::Returndata);
    let offset = bounded(7012, 64, 96, Origin::Calldata);
    let make = |input: &TestValue| {
        let mut memory = TestBytes::memory();
        memory
            .write_word(&offset, input, domain.borrowed())
            .unwrap();
        memory
    };
    let yes = make(&a);
    let no = make(&b);
    let joined = yes
        .join_guarded(&no, domain.borrowed(), guard.clone(), limits)
        .unwrap()
        .slice(&offset, &value(32), domain.borrowed())
        .unwrap();
    let mut copied = TestBytes::memory();
    copied
        .copy_from(
            &value(32),
            &joined,
            &value(0),
            &value(32),
            domain.borrowed(),
        )
        .unwrap();
    let expected = domain.join(&a, &b);
    let unselected = copied
        .read_word_checked(&value(32), domain.borrowed())
        .unwrap();
    assert_eq!(unselected.numeric(), expected.numeric());
    assert_eq!(unselected.provenance(), expected.provenance());
    assert!(!unselected.same_identity(&a) && !unselected.same_identity(&b));
    let frozen = copied.clone();
    for (nonzero, expected) in [(true, &a), (false, &b), (true, &a)] {
        let mut selected = copied.clone();
        assert!(
            selected
                .refine_guards(domain.borrowed(), limits, &mut |condition| {
                    Ok::<_, std::convert::Infallible>((condition == &guard).then_some(nonzero))
                })
                .unwrap()
        );
        let read = selected
            .read_word_checked(&value(32), domain.borrowed())
            .unwrap();
        assert_eq!(read.numeric(), expected.numeric());
        assert_eq!(read.provenance(), expected.provenance());
        assert!(read.same_identity(expected));
        assert_eq!(selected.state_root(), frozen.state_root());
        assert_eq!(copied, frozen);
    }
    assert_eq!(
        copied
            .read_word_checked(&value(32), domain.borrowed())
            .unwrap(),
        unselected
    );
}

#[test]
fn symbolic_nonwrapping_whole_windows_retain_source_identity() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let input = bounded(7003, 3, 9, Origin::Calldata);
    let offset = bounded(7004, 64, 96, Origin::Calldata);
    let length = bounded(7005, 32, 64, Origin::Calldata);
    let mut memory = TestBytes::memory();
    memory
        .write_word(&offset, &input, domain.borrowed())
        .unwrap();
    let sliced = memory.slice(&offset, &length, domain.borrowed()).unwrap();
    assert_eq!(
        sliced
            .read_word_checked(&value(0), domain.borrowed())
            .unwrap(),
        input
    );
    let mut copied = TestBytes::memory();
    copied
        .copy_from(&offset, &sliced, &value(0), &length, domain.borrowed())
        .unwrap();
    assert_eq!(
        copied
            .read_word_checked(&offset, domain.borrowed())
            .unwrap(),
        input
    );
}

#[test]
fn truncated_unaligned_and_short_return_windows_keep_padding_and_old_suffix() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let input = bounded(7006, 3, 9, Origin::Calldata).with_numeric(value(7).numeric().clone());
    let original = source(&input, domain);
    let truncated = original
        .slice(&value(64), &value(31), domain.borrowed())
        .unwrap();
    let read = truncated
        .read_word_checked(&value(0), domain.borrowed())
        .unwrap();
    assert_eq!(read.singleton(), Some(U256::ZERO));
    assert!(!read.same_identity(&input));

    let unaligned = original
        .slice(&value(65), &value(32), domain.borrowed())
        .unwrap();
    let read = unaligned
        .read_word_checked(&value(0), domain.borrowed())
        .unwrap();
    assert_eq!(read.singleton(), Some(U256::from(7) << 8));
    assert!(!read.same_identity(&input));

    let mut memory = TestBytes::memory();
    memory
        .write_word(
            &value(32),
            &TestValue::constant(U256::from_be_bytes([0xaa; 32])),
            domain.borrowed(),
        )
        .unwrap();
    memory
        .copy_return_data(&value(32), &truncated, &value(32), domain.borrowed())
        .unwrap();
    assert_eq!(
        memory
            .read_word_checked(&value(32), domain.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::from(0xaa))
    );
    assert_eq!(
        memory.byte_at(63, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    let returned = memory
        .slice(&value(32), &value(32), domain.borrowed())
        .unwrap();
    assert_eq!(
        returned
            .read_word_checked(&value(0), domain.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::from(0xaa))
    );
    // Ordinary COPY writes the requested padding; CALL writes only the actual
    // returned prefix. The same short source must distinguish these effects.
    memory
        .copy_from(
            &value(32),
            &truncated,
            &value(0),
            &value(32),
            domain.borrowed(),
        )
        .unwrap();
    assert_eq!(
        memory
            .read_word_checked(&value(32), domain.borrowed())
            .unwrap()
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn checked_window_addition_never_forwards_a_truncated_or_wrapped_word() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let expression = ExprId::fresh().unwrap();
    let mut input = TestValue::top().with_symbol(Symbol::CalldataWord(U256::ZERO), Some(7007));
    input.set_origin(Origin::Calldata);
    let input = input
        .with_numeric(crate::domain::NumericValue::constant(U256::MAX))
        .with_expression(expression.clone());
    let facts: Facts = BTreeMap::from([(expression.clone(), input.clone())]);
    let term = ExprId::constant;
    let limits = StateLimits::default();
    let near_max = ByteStateId::zero()
        .write_word(term(U256::MAX), expression.clone(), limits)
        .unwrap();
    let sliced = near_max
        .slice(term(U256::MAX), term(U256::from(32)), limits)
        .unwrap();
    let mut reader = Reader::new(&facts, domain.borrowed());
    let complete = ByteStateId::zero()
        .write_word(term(U256::MAX - U256::from(31)), expression.clone(), limits)
        .unwrap()
        .slice(
            term(U256::MAX - U256::from(31)),
            term(U256::from(32)),
            limits,
        )
        .unwrap();
    assert_eq!(reader.word(&complete, &term(U256::ZERO)), input);
    let read = reader.word(&sliced, &term(U256::ZERO));
    assert_eq!(read.singleton(), Some(U256::from(255) << 248));
    assert!(!read.same_identity(&input));
    let original = ByteStateId::zero()
        .write_word(term(U256::ZERO), expression, limits)
        .unwrap();
    let copied = ByteStateId::zero()
        .copy(
            term(U256::MAX),
            ByteView::new(original.clone(), term(U256::ZERO), term(U256::from(32))),
            term(U256::from(32)),
            limits,
        )
        .unwrap();
    assert_eq!(
        reader.word(&copied, &term(U256::MAX)).singleton(),
        Some(U256::from(255) << 248)
    );
    assert_eq!(
        reader.word(&copied, &term(U256::ZERO)).singleton(),
        Some(U256::ZERO)
    );
    let wrapped = original
        .slice(term(U256::MAX), term(U256::from(64)), limits)
        .unwrap();
    assert_eq!(
        reader.word(&wrapped, &term(U256::ONE)).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn a_later_proved_copy_minimum_preserves_the_uncopied_suffix_and_history() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut memory = TestBytes::memory();
    for (at, byte) in [(32, 0xaa), (64, 0xbb)] {
        memory
            .write_word(
                &value(at),
                &TestValue::constant(U256::from_be_bytes([byte; 32])),
                domain.borrowed(),
            )
            .unwrap();
    }
    memory
        .copy_return_data(
            &value(32),
            &TestBytes::unknown(),
            &value(16),
            domain.borrowed(),
        )
        .unwrap();
    let snapshot = memory.clone();
    let crate::domain::symbolic::ByteStateKind::Copy { length, .. } = memory.state_root().kind()
    else {
        panic!("unknown return copy must retain its symbolic minimum");
    };
    let length = length.clone();
    assert_eq!(
        memory.byte_at(48, domain.borrowed()).singleton(),
        Some(U256::from(0xaa))
    );
    // Import an externally proved min(16, returned_length) == 1, just as the
    // continuation's scalar refinement does. The content graph stays frozen.
    let mut imported = false;
    memory.refine_operands(&mut |expression, old| {
        if expression == &length {
            assert!(old.contains(U256::ONE));
            *old = old.clone().with_numeric(value(1).numeric().clone());
            imported = true;
        }
    });
    assert!(imported);
    assert_eq!(memory.state_root(), snapshot.state_root());
    let returned = memory
        .slice(&value(32), &value(64), domain.borrowed())
        .unwrap();
    for at in 1..32 {
        assert_eq!(
            returned.byte_at(at, domain.borrowed()).singleton(),
            Some(U256::from(0xaa))
        );
    }
    for at in 32..64 {
        assert_eq!(
            returned.byte_at(at, domain.borrowed()).singleton(),
            Some(U256::from(0xbb))
        );
    }
    assert!(snapshot.byte_at(33, domain.borrowed()).contains(U256::ZERO));
    assert!(
        snapshot
            .byte_at(33, domain.borrowed())
            .contains(U256::from(0xaa))
    );
}

#[test]
fn forwarded_reads_still_obey_typed_limits_and_cancellation() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let input = bounded(7008, 3, 9, Origin::Calldata);
    let sliced = source(&input, domain)
        .slice(&value(64), &value(32), domain.borrowed())
        .unwrap();
    let frozen = sliced.clone();
    let low = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        domain
            .spec()
            .with_relations(crate::domain::relational::RelationLimits {
                max_nodes: 1,
                ..domain.spec().relations()
            }),
    ));
    assert!(matches!(
        sliced.read_word_checked(&value(0), low.borrowed()),
        Err(RangeError::Expression(ExprError::Limit { .. }))
    ));
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(
            sliced.read_word_checked(&value(0), domain.borrowed()),
            Err(RangeError::Expression(ExprError::Cancelled))
        );
    });
    assert_eq!(sliced, frozen);
}
