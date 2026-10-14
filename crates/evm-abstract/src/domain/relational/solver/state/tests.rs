//! Native proofs for packed hash contents, including the former byte-array oracle.

use super::{Array, BV, Bool, ByteStateId, ByteView, Encoder, ExprId, byte_zero, word};
use crate::domain::{
    NumericValue,
    relational::{CheckResult, RelationLimits, RelationState, SmtProvider, ValueQuery},
    symbolic::{SourceId, StateBinding, StateLimits},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

mod finite;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn policy(provider: SmtProvider) -> RelationLimits {
    // The byte-wise reference needs more encoding nodes, but no extra solver fuel.
    RelationLimits {
        provider,
        max_nodes: 8192,
        max_depth: 128,
        ..RelationLimits::default()
    }
}

fn hash(state: ByteStateId, offset: u64, length: u64, limits: RelationLimits) -> ExprId {
    ExprId::keccak(
        ByteView::new(state, c(offset), c(length)),
        limits.expressions(),
    )
    .unwrap()
}

fn equal(left: ExprId, right: ExprId, limits: RelationLimits) -> ExprId {
    ExprId::operation(opcode::EQ, &[left, right], limits.expressions()).unwrap()
}

#[test]
fn an_unsuccessful_whole_array_lowering_cannot_suppress_initial_byte_facts() {
    let state = ByteStateId::observed(
        SourceId::fresh().unwrap(),
        c(1),
        vec![StateBinding {
            index: U256::ZERO,
            value: ExprId::fresh().unwrap(),
            numeric: NumericValue::constant(U256::from(0xaa)),
        }],
        StateLimits::default(),
    )
    .unwrap();
    let limits = RelationLimits {
        rlimit: 10_000_000,
        ..RelationLimits::default()
    };
    let mut encoder = Encoder::new(&limits);
    assert!(encoder.byte_array(&state).unwrap().is_none());
    let value = encoder.byte(&state, &word(U256::ZERO)).unwrap();
    let mut assertions = Vec::new();
    encoder.emit_definitions(&mut assertions);
    assertions.push(value.eq(BV::from_u64(0xaa, 8)).not());
    assert_eq!(
        super::super::checked(&assertions, &limits),
        CheckResult::Unsat
    );
}

#[test]
fn normalized_hashes_forward_two_mstore_words_at_the_default_policy() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        let x = ExprId::fresh().unwrap();
        let y = ExprId::fresh().unwrap();
        let mut hashes = Vec::new();
        for offset in [0, 128] {
            let state = ByteStateId::zero()
                .write_word(c(offset), x.clone(), limits.expressions().into())
                .unwrap()
                .write_word(c(offset + 32), y.clone(), limits.expressions().into())
                .unwrap();
            hashes.push(hash(state, offset, 64, limits));
        }
        let equality = equal(hashes.remove(0), hashes.remove(0), limits);
        assert_eq!(
            RelationState::default().unique_value(&equality, &limits),
            ValueQuery::Unique(U256::ONE),
            "{provider:?}: identical two-word contents at independent addresses"
        );
    }
}

#[test]
fn normalized_hashes_preserve_partial_cross_address_copies() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let original = ByteStateId::zero()
            .write_word(c(0), ExprId::fresh().unwrap(), limits.expressions().into())
            .unwrap();
        let copied = ByteStateId::zero()
            .copy(
                c(128),
                ByteView::new(original.clone(), c(0), c(32)),
                c(32),
                limits.expressions().into(),
            )
            .unwrap();
        // A 31-byte prefix is not the complete-copy canonicalization case.
        let equality = equal(
            hash(original, 0, 31, limits),
            hash(copied, 128, 31, limits),
            limits,
        );
        assert_eq!(
            RelationState::default().unique_value(&equality, &limits),
            ValueQuery::Unique(U256::ONE),
            "{provider:?}: copied partial content shares the normalized signature"
        );
    }
}

#[test]
fn normalized_hashes_keep_length_separate_and_allow_content_collisions() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let byte = ExprId::fresh().unwrap();
        let original = ByteStateId::zero()
            .write_byte(c(0), byte.clone(), limits.expressions().into())
            .unwrap();
        // Both packed contents have the same unknown first byte and zero suffix.
        // Different logical lengths must not force the hash outputs to agree.
        let different_length = equal(
            hash(original.clone(), 0, 31, limits),
            hash(original.clone(), 0, 32, limits),
            limits,
        );
        let opposite_byte =
            ExprId::operation(opcode::XOR, &[byte, c(255)], limits.expressions()).unwrap();
        let different = ByteStateId::zero()
            .write_byte(c(128), opposite_byte, limits.expressions().into())
            .unwrap();
        // The first bytes differ for every valuation. Equal digests remain
        // possible in the conservative function model; no injectivity is added.
        let different_content = equal(
            hash(original, 0, 32, limits),
            hash(different, 128, 32, limits),
            limits,
        );
        for (label, equality) in [("length", different_length), ("content", different_content)] {
            for result in [U256::ZERO, U256::ONE] {
                assert_eq!(
                    RelationState::default().can_equal(&equality, result, &limits),
                    CheckResult::Sat,
                    "{provider:?}: different {label}, equality={result}"
                );
            }
        }
    }
}

fn assert_byte_array_reference(
    view: ByteView,
    length: usize,
    relations: &RelationState,
    limits: RelationLimits,
) {
    let (mut assertions, mut encoder) = super::super::prepared(relations, None, &limits).unwrap();
    let packed = encoder.normalized_hash_content(&view, length).unwrap();
    // This is the previous signature-0 content construction, independent of
    // word forwarding. Each of the 64 cells must equal the corresponding byte.
    let mut reference = Array::constant(256, &byte_zero());
    for index in 0..length {
        let at = word(U256::from(index));
        let byte = encoder.view_byte(&view, &at).unwrap();
        reference = reference.store(&at, &byte);
    }
    let cells = (0..64)
        .map(|index| {
            let high = 511 - index * 8;
            packed
                .extract(high, high - 7)
                .eq(reference.select(&word(U256::from(index))))
        })
        .collect::<Vec<_>>();
    encoder.emit_definitions(&mut assertions);
    assertions.push(Bool::and(&cells).not());
    assert_eq!(
        super::super::checked(&assertions, &limits),
        CheckResult::Unsat,
        "{:?}: length={length}, offset={:?}",
        limits.provider,
        view.offset
    );
}

#[test]
fn normalized_hash_content_matches_byte_array_reference_and_never_wraps() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let cases = [
            (0, U256::from(128), false),
            (1, U256::from(128), false),
            (31, U256::from(128), false),
            (32, U256::from(128), false),
            (33, U256::from(128), false),
            (63, U256::from(128), false),
            (64, U256::from(128), false),
            (33, U256::MAX - U256::from(32), false),
            (64, U256::MAX - U256::from(31), false),
            (64, U256::MAX - U256::ONE, false),
            (32, U256::MAX, false),
            (64, U256::MAX, true),
            (33, U256::MAX - U256::from(31), true),
            (64, U256::ZERO, true),
        ];
        for (length, start, symbolic) in cases {
            let offset = if symbolic {
                ExprId::fresh().unwrap()
            } else {
                ExprId::constant(start)
            };
            let mut relations = RelationState::default();
            if symbolic {
                relations
                    .observe(&offset, &NumericValue::constant(start), &limits)
                    .unwrap();
            }
            let memory = ByteStateId::zero()
                .write_word(c(0), ExprId::fresh().unwrap(), limits.expressions().into())
                .unwrap()
                .write_word(
                    offset.clone(),
                    ExprId::fresh().unwrap(),
                    limits.expressions().into(),
                )
                .unwrap();
            assert_byte_array_reference(
                ByteView::new(memory, offset, ExprId::constant(U256::from(length))),
                length,
                &relations,
                limits,
            );
        }
    }
}

#[test]
fn normalized_hash_content_preserves_short_source_padding() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for length in [1, 31, 33, 64] {
            let input = ByteStateId::input(SourceId::fresh().unwrap(), c(length));
            assert_byte_array_reference(
                ByteView::new(input, c(0), c(64)),
                64,
                &RelationState::default(),
                limits,
            );
        }
    }
}
