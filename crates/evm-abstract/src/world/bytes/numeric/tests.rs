//! Batched evaluation keeps the original values and operand environment.

use super::{
    Facts, Reader, WORD_CACHE_ENTRIES, WORD_CACHE_WORK, constant, join, numeric_domain, scalar,
    zero,
};
use crate::{
    domain::{
        AbstractValue, Domain,
        provenance::Origin,
        symbolic::{ByteStateId, ByteView, ExprId, ExprLimits, StateLimits},
    },
    world::ByteArray,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

fn expression() -> ExprId {
    ExprId::fresh().unwrap()
}

// Keep the original ordered BYTE/join evaluation as a full-value oracle. The
// source lookup is deliberately repeated, including its constant-key handling.
fn original_union(expression: &ExprId, facts: &Facts, domain: Domain) -> AbstractValue {
    let domain = numeric_domain(domain);
    (0..32)
        .fold(None, |out, index| {
            Some(join(
                out,
                domain.apply(
                    opcode::BYTE,
                    &[constant(index), scalar(expression, facts, domain)],
                ),
                domain,
            ))
        })
        .unwrap()
}

fn uncached(facts: &Facts, domain: Domain) -> Reader<'_> {
    let mut reader = Reader::new(facts, domain);
    // A full retention budget makes every cache miss use the ordinary evaluator.
    reader.retained_work = WORD_CACHE_WORK;
    reader
}

fn word(previous: &ByteStateId, at: &ExprId, value: &ExprId) -> ByteStateId {
    previous
        .write_word(at.clone(), value.clone(), StateLimits::default())
        .unwrap()
}

#[test]
fn ambiguous_word_reads_reuse_the_complete_value_and_sources() {
    let domain = Domain::default();
    let value = expression();
    let at = expression();
    let mut bounded = AbstractValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap();
    bounded.set_origin(Origin::Calldata);
    let facts = BTreeMap::from([(value.clone(), bounded)]);
    let state = word(&ByteStateId::zero(), &at, &value);
    let mut reader = Reader::new(&facts, domain);
    let expected = domain.join(&zero(), &original_union(&value, &facts, domain));
    for index in 0..256 {
        assert_eq!(
            reader.read(&state, &ExprId::constant(U256::from(index))),
            expected
        );
    }
    assert_eq!(reader.word_bytes.len(), 1);
    let sources = expected.provenance().origins().sources().unwrap();
    assert!(sources.contains(&Origin::Calldata));
    assert!(sources.contains(&Origin::Arithmetic));

    // Reusing the word union does not reuse a complete byte read. Its previous
    // state is still evaluated and joined on every demand.
    let previous = ByteStateId::exact(std::sync::Arc::from([0xaa; 256]));
    let other = word(&previous, &at, &value);
    assert_eq!(
        reader.read(&other, &ExprId::constant(U256::ZERO)),
        uncached(&facts, domain).read(&other, &ExprId::constant(U256::ZERO))
    );
}

#[test]
fn later_facts_and_another_domain_get_independent_evaluations() {
    let domain = Domain::default();
    let value = expression();
    let at = expression();
    let state = word(&ByteStateId::zero(), &at, &value);
    let mut facts = BTreeMap::from([(value.clone(), constant(0x1122))]);
    let before = Reader::new(&facts, domain).read(&state, &ExprId::constant(U256::ZERO));
    facts.insert(value.clone(), constant(0x3344));
    let after = Reader::new(&facts, domain).read(&state, &ExprId::constant(U256::ZERO));
    assert_ne!(before, after);
    assert_eq!(
        after,
        domain.join(&zero(), &original_union(&value, &facts, domain))
    );
    facts.insert(at.clone(), constant(64));
    assert_eq!(
        Reader::new(&facts, domain).read(&state, &ExprId::constant(U256::ZERO)),
        zero()
    );

    facts.remove(&at);
    facts.insert(
        value.clone(),
        AbstractValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap(),
    );
    let constants = Domain::new(std::num::NonZeroUsize::new(8).unwrap());
    let product = Reader::new(&facts, domain).read(&state, &ExprId::constant(U256::ZERO));
    let finite = Reader::new(&facts, constants).read(&state, &ExprId::constant(U256::ZERO));
    assert_ne!(product.numeric(), finite.numeric());
    assert_eq!(
        finite,
        constants.join(&zero(), &original_union(&value, &facts, constants))
    );
}

#[test]
fn copies_slices_aliases_and_overflow_keep_the_original_read_result() {
    let domain = Domain::default();
    let value = expression();
    let at = expression();
    let facts = BTreeMap::from([(value.clone(), constant(0x1122))]);
    let state = word(&ByteStateId::zero(), &at, &value);
    let copied = state
        .copy(
            ExprId::constant(U256::from(128)),
            ByteView::new(
                state.clone(),
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(64)),
            ),
            ExprId::constant(U256::from(64)),
            StateLimits::default(),
        )
        .unwrap();
    let overflowing = state
        .slice(
            ExprId::constant(U256::MAX),
            ExprId::constant(U256::from(2)),
            StateLimits::default(),
        )
        .unwrap();
    let near_max = word(&ByteStateId::zero(), &ExprId::constant(U256::MAX), &value);
    let mut reader = Reader::new(&facts, domain);
    let mut reference = uncached(&facts, domain);
    for state in [&state, &copied, &overflowing, &near_max] {
        for index in [
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(1)),
            ExprId::constant(U256::from(127)),
            ExprId::constant(U256::from(128)),
            ExprId::constant(U256::from(191)),
            ExprId::constant(U256::from(192)),
            ExprId::constant(U256::MAX),
            at.clone(),
        ] {
            assert_eq!(reader.read(state, &index), reference.read(state, &index));
        }
    }
    assert_eq!(
        reader.read(&overflowing, &ExprId::constant(U256::ONE)),
        zero()
    );
    assert_eq!(
        reader.read(&near_max, &ExprId::constant(U256::ZERO)),
        zero()
    );
    assert_eq!(reader.word_bytes.len(), 1);
}

#[test]
fn finite_index_and_offset_alternatives_still_select_their_own_byte() {
    let domain = Domain::default();
    let value = expression();
    let at = expression();
    let index = expression();
    let facts = BTreeMap::from([
        (value.clone(), constant(0x1122)),
        (at.clone(), domain.join(&constant(0), &constant(32))),
        (index.clone(), domain.join(&constant(30), &constant(63))),
    ]);
    let state = word(&ByteStateId::zero(), &at, &value);
    let mut reader = Reader::new(&facts, domain);
    assert_eq!(
        reader.read(&state, &index),
        uncached(&facts, domain).read(&state, &index)
    );
    assert_eq!(
        reader.read(&state, &index).constants(),
        Some(&std::collections::BTreeSet::from([
            U256::ZERO,
            U256::from(0x11),
            U256::from(0x22)
        ]))
    );
    assert!(reader.word_bytes.is_empty());
}

#[test]
fn cache_capacity_and_work_limits_fall_back_without_losing_values() {
    let domain = Domain::default();
    let values = (0..WORD_CACHE_ENTRIES + 2)
        .map(|_| expression())
        .collect::<Vec<_>>();
    let facts = values
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), constant(index + 1)))
        .collect();
    let mut reader = Reader::new(&facts, domain);
    for value in &values {
        assert_eq!(
            reader.word_byte_union(value),
            original_union(value, &facts, domain)
        );
    }
    assert_eq!(reader.word_bytes.len(), WORD_CACHE_ENTRIES);
    assert_eq!(
        reader.retained_work,
        reader
            .word_bytes
            .iter()
            .map(|(key, value)| key.work_size() + value.work_size())
            .sum::<usize>()
    );
    assert!(reader.retained_work <= WORD_CACHE_WORK);
    assert!(!reader.word_bytes.contains_key(values.last().unwrap()));

    let limits = ExprLimits {
        max_nodes: WORD_CACHE_WORK * 4,
        max_depth: 64,
    };
    let mut large = expression();
    while large.work_size() <= WORD_CACHE_WORK {
        large = ExprId::operation(opcode::ADD, &[large.clone(), large], limits).unwrap();
    }
    let large_facts = BTreeMap::from([(large.clone(), constant(0xaa))]);
    let mut reader = Reader::new(&large_facts, domain);
    assert_eq!(
        reader.word_byte_union(&large),
        original_union(&large, &large_facts, domain)
    );
    assert!(reader.word_bytes.is_empty());
    assert_eq!(reader.retained_work, 0);
}

fn projection_input() -> ByteArray {
    let at = expression();
    let value = expression();
    let mut array = ByteArray::memory();
    array.root = word(&array.root, &at, &value);
    array.length = constant(256);
    array.operands = BTreeMap::from([
        (
            at,
            AbstractValue::unsigned_range(U256::ZERO, U256::from(224)).unwrap(),
        ),
        (
            value,
            AbstractValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap(),
        ),
    ]);
    array
}

#[test]
fn real_summary_projection_matches_all_original_byte_bounds() {
    let domain = Domain::default();
    let input = projection_input();
    let summary = input.summary_projection(domain).unwrap();
    let restored = summary.instantiate(domain).unwrap();
    let mut reference = uncached(&input.operands, domain);
    for index in 0..256 {
        assert_eq!(
            restored.byte_at(index, domain).numeric(),
            reference
                .read(&input.root, &ExprId::constant(U256::from(index)))
                .numeric()
        );
    }
    assert_eq!(summary, restored.summary_projection(domain).unwrap());
}
