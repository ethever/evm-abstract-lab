//! Batched evaluation keeps the original values and operand environment.

use super::{
    Facts, Reader, WORD_CACHE_ENTRIES, WORD_CACHE_WORK, constant, numeric_domain, scalar, zero,
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

#[test]
fn literal_and_complete_finite_writes_keep_exact_bytes_and_untouched_suffixes() {
    let domain = numeric_domain(Domain::default());
    for value in [
        constant(0x1122),
        domain.join(&constant(0x1122), &constant(0x3344)),
    ] {
        let key = expression();
        let value = value
            .with_origin(Origin::Storage)
            .with_identity(982_137, 1)
            .with_symbolic_limit();
        let facts = Facts::from([(key.clone(), value.clone())]);
        let previous = ByteStateId::exact(std::sync::Arc::from([0xa5_u8; 72]));
        let at = ExprId::constant(U256::from(8));
        for word in [false, true] {
            let state = if word {
                previous
                    .write_word(at.clone(), key.clone(), StateLimits::default())
                    .unwrap()
            } else {
                previous
                    .write_byte(at.clone(), key.clone(), StateLimits::default())
                    .unwrap()
            };
            let mut reader = Reader::new(&facts, domain);
            for index in 0..80 {
                let expected = if word && (8..40).contains(&index) {
                    domain.apply(opcode::BYTE, &[constant(index - 8), value.clone()])
                } else if !word && index == 8 {
                    domain.apply(opcode::AND, &[value.clone(), constant(255)])
                } else {
                    constant(if index < 72 { 0xa5 } else { 0 })
                };
                let actual = reader.read(&state, &ExprId::constant(U256::from(index)));
                assert_eq!(actual, expected, "word={word}, index={index}");
                assert_eq!(actual.constants(), expected.constants());
                assert!(!actual.same_identity(&value));
            }
            assert_eq!(reader.cache_stats(), (0, 0));
        }
    }
}

#[test]
fn open_word_suffixes_remain_exact_through_reader_byte_observations() {
    use crate::domain::facts::{BitConstraints, UnaryPredicate};
    let domain = numeric_domain(Domain::default());
    let mut suffix = domain
        .from_facts(&[UnaryPredicate::KnownBits(
            BitConstraints::new(U256::from(0xffff ^ 0x1122), U256::from(0x1122)).unwrap(),
        )])
        .unwrap()
        .with_symbolic_limit();
    suffix.set_origin(Origin::Storage);
    assert!(suffix.constants().is_none());
    assert!(suffix.singleton().is_none());
    let key = expression();
    let facts = Facts::from([(key.clone(), suffix)]);
    let previous = ByteStateId::exact(std::sync::Arc::from([0xa5_u8; 72]));
    let at = ExprId::constant(U256::from(8));
    let word = previous
        .write_word(at.clone(), key.clone(), StateLimits::default())
        .unwrap();
    let byte = previous
        .write_byte(at.clone(), key, StateLimits::default())
        .unwrap();
    let mut reader = Reader::new(&facts, domain);
    for (state, index, expected) in [(&word, 38, 0x11), (&word, 39, 0x22), (&byte, 8, 0x22)] {
        let actual = reader.read(state, &ExprId::constant(U256::from(index)));
        assert_eq!(actual.numeric(), constant(expected).numeric());
        assert!(!actual.symbolic_limit_reached());
        let sources = actual.provenance().origins().sources().unwrap();
        assert!(sources.contains(&Origin::Storage));
        assert!(sources.contains(&Origin::Constant));
        assert!(sources.contains(&Origin::Arithmetic));
    }
    for (state, index) in [(&word, 40), (&byte, 9)] {
        assert_eq!(
            reader.read(state, &ExprId::constant(U256::from(index))),
            constant(0xa5)
        );
    }
    for high in [U256::ZERO, U256::ONE, U256::MAX >> 16usize] {
        let concrete = ((high << 16usize) | U256::from(0x1122)).to_be_bytes::<32>();
        for (index, expected) in concrete.into_iter().enumerate() {
            assert!(
                reader
                    .read(&word, &ExprId::constant(U256::from(index + 8)))
                    .contains(U256::from(expected))
            );
        }
    }
}

#[test]
fn ordinary_scalar_byte_operations_keep_the_generic_transfer() {
    let domain = numeric_domain(Domain::default());
    let key = expression();
    let value = AbstractValue::unsigned_range(U256::from(1), U256::from(9)).unwrap();
    let facts = Facts::from([(key.clone(), value.clone())]);
    for (op, args, numeric) in [
        (
            opcode::AND,
            [key.clone(), ExprId::constant(U256::from(255))],
            [value.clone(), constant(255)],
        ),
        (
            opcode::BYTE,
            [ExprId::constant(U256::from(31)), key],
            [constant(31), value],
        ),
    ] {
        let expression = ExprId::operation(op, &args, ExprLimits::default()).unwrap();
        assert_eq!(
            scalar(&expression, &facts, domain),
            domain.apply(op, &numeric)
        );
    }
}

fn expression() -> ExprId {
    ExprId::fresh().unwrap()
}

fn join(out: Option<AbstractValue>, value: AbstractValue, domain: Domain) -> AbstractValue {
    out.map_or_else(|| value.clone(), |old| domain.join(&old, &value))
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

pub(super) static WORD_BYTE_UNION_CALLS: crate::test_instrumentation::Counter<(usize, usize)> =
    crate::test_instrumentation::Counter::new((0, 0));

mod top_word_byte_union {
    use super::super::{Context, scalar_in};
    use super::{
        Facts, Reader, WORD_BYTE_UNION_CALLS, WORD_CACHE_WORK, constant, expression, join,
        numeric_domain, original_union, uncached,
    };
    use crate::{
        domain::{
            AbstractValue, Domain, DomainSpec, Profile,
            facts::{BitIndex, UnaryPredicate},
            identity::Symbol,
            provenance::{Origin, OriginSet},
            relational::RelationLimits,
            symbolic::{ExprError, ExprId, ExprLimits},
        },
        world::bytes::conditional::ConditionalFacts,
    };
    use alloy_primitives::U256;
    use revm_bytecode::opcode;
    use std::num::NonZeroUsize;

    fn product(capacity: usize, rounds: usize, fact_limit: usize) -> Domain {
        numeric_domain(Domain::from_spec(DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(rounds).unwrap(),
            NonZeroUsize::new(fact_limit).unwrap(),
        )))
    }

    fn counted_union(
        reader: &mut Reader<'_>,
        expression: &ExprId,
        expected: &AbstractValue,
        calls: (usize, usize),
    ) -> AbstractValue {
        let _calls = WORD_BYTE_UNION_CALLS.scope();
        WORD_BYTE_UNION_CALLS.set((0, 0));
        let actual = reader.word_byte_union(expression);
        assert_eq!(WORD_BYTE_UNION_CALLS.replace((0, 0)), calls);
        assert_eq!(&actual, expected);
        actual
    }

    // Unlike original_union, this oracle preserves an explicit guarded context
    // and relations-on policy, which Reader's public constructors normalize off.
    fn original_in_context(
        expression: &ExprId,
        context: &Context<'_>,
        domain: Domain,
    ) -> AbstractValue {
        (0..32)
            .fold(None, |out, index| {
                Some(join(
                    out,
                    domain.apply(
                        opcode::BYTE,
                        &[constant(index), scalar_in(expression, context, domain)],
                    ),
                    domain,
                ))
            })
            .unwrap()
    }

    #[test]
    fn complete_top_keeps_all_layers_of_the_original_union_under_limits() {
        let mut sourced = AbstractValue::top();
        sourced.set_origin(Origin::Calldata);
        sourced = sourced.with_origin(Origin::Storage);
        let role = Domain::default()
            .from_facts(&[
                UnaryPredicate::PossibleOrigins(
                    OriginSet::from_sources([Origin::Calldata, Origin::Storage]).unwrap(),
                ),
                UnaryPredicate::IsCodeAddress,
            ])
            .unwrap();
        assert!(role.numeric().is_top());
        assert!(role.provenance().is_code_address());
        let runtime = sourced.clone().with_identity(71, 9);
        let input = sourced
            .clone()
            .with_symbol(Symbol::CalldataWord(U256::from(32)), Some(72));
        let combined = role
            .clone()
            .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(73))
            .with_identity(74, 10)
            .with_symbolic_limit();
        let values = [
            AbstractValue::top(),
            sourced.clone(),
            role,
            runtime.clone(),
            input.clone(),
            input.clone().with_identity(75, 11),
            sourced.with_symbolic_limit(),
            runtime.with_symbolic_limit(),
            input.with_symbolic_limit(),
            combined,
        ];
        for capacity in [8, 512] {
            for (rounds, fact_limit) in [(4, 256), (4, 1024), (1, 256), (4, 1), (1, 1)] {
                let domain = product(capacity, rounds, fact_limit);
                for value in &values {
                    let key = value.expression().cloned().unwrap_or_else(expression);
                    let facts = Facts::from([(key.clone(), value.clone())]);
                    let mut reader = Reader::new(&facts, domain);
                    let scalar = scalar_in(&key, &reader.context, reader.domain);
                    assert_eq!(&scalar, value);
                    assert!(scalar.numeric().is_top());
                    reader.context.check().unwrap();
                    assert!(!reader.context.cache_refused());
                    if value.same_identity(value) {
                        assert!(scalar.same_identity(value));
                    }
                    let expected = original_union(&key, &facts, domain);
                    for index in 0..32 {
                        assert_eq!(
                            domain.apply(opcode::BYTE, &[constant(index), scalar.clone()]),
                            expected
                        );
                    }
                    assert_eq!(domain.join(&expected, &expected), expected);
                    let actual = counted_union(&mut reader, &key, &expected, (1, 0));
                    assert!(!actual.provenance().is_code_address());
                    assert!(actual.expression().is_none());
                    assert!(actual.identity().input().is_none());
                    assert!(!actual.same_identity(value));
                    assert_eq!(
                        actual.symbolic_limit_reached(),
                        value.symbolic_limit_reached()
                    );
                    if let Some(sources) = value.provenance().origins().sources() {
                        let output = actual.provenance().origins().sources().unwrap();
                        assert!(sources.is_subset(output));
                        assert!(output.contains(&Origin::Constant));
                        assert!(output.contains(&Origin::Arithmetic));
                    }
                    assert_eq!(reader.word_bytes.len(), 1);
                    assert_eq!(reader.retained_work, key.work_size() + actual.work_size());
                    counted_union(&mut reader, &key, &expected, (0, 0));

                    let mut refused = uncached(&facts, domain);
                    counted_union(&mut refused, &key, &expected, (1, 0));
                    counted_union(&mut refused, &key, &expected, (1, 0));
                    assert!(refused.word_bytes.is_empty());
                    assert_eq!(refused.retained_work, WORD_CACHE_WORK);
                }
            }
        }
    }

    #[test]
    fn finite_top_with_other_numeric_constraints_uses_every_old_operation() {
        let constructor = Domain::default();
        let constrained = [
            constructor
                .from_facts(&[UnaryPredicate::BitClear(BitIndex::new(255).unwrap())])
                .unwrap(),
            AbstractValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap(),
            constructor.from_facts(&[UnaryPredicate::NonZero]).unwrap(),
            constructor
                .from_facts(&[UnaryPredicate::congruent(U256::from(3), U256::ONE).unwrap()])
                .unwrap(),
        ];
        assert_ne!(
            constrained[0].known_bits(),
            &crate::domain::known_bits::KnownBits::top()
        );
        assert_ne!(
            constrained[1].interval(),
            &crate::domain::interval::Interval::top()
        );
        assert!(!constrained[2].may_be_zero());
        assert!(!constrained[3].congruence().is_top());
        for value in &constrained {
            assert!(value.finite_constants().is_top());
            assert!(!value.numeric().is_top());
        }
        let concrete = constant(0x1122);
        for capacity in [8, 512] {
            for (rounds, fact_limit) in [(4, 256), (1, 1)] {
                let domain = product(capacity, rounds, fact_limit);
                for (index, value) in constrained.iter().chain([&concrete]).enumerate() {
                    let key = expression();
                    let facts = Facts::from([(key.clone(), value.clone())]);
                    let expected = original_union(&key, &facts, domain);
                    let mut reader = Reader::new(&facts, domain);
                    let calls = if matches!(index, 0 | 2) {
                        (1, 0)
                    } else {
                        (32, 31)
                    };
                    counted_union(&mut reader, &key, &expected, calls);
                    counted_union(&mut reader, &key, &expected, (0, 0));
                }
            }
        }
    }

    #[test]
    fn constants_only_and_relations_on_keep_the_ordered_loop() {
        let key = expression();
        let input = AbstractValue::top()
            .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(81))
            .with_symbolic_limit();
        let facts = Facts::from([(key.clone(), input.clone())]);
        for profile in [Profile::Product, Profile::ConstantsOnly] {
            for enabled in [false, true] {
                let domain = Domain::from_spec(
                    DomainSpec::new(
                        profile,
                        NonZeroUsize::new(8).unwrap(),
                        NonZeroUsize::new(4).unwrap(),
                        NonZeroUsize::new(256).unwrap(),
                    )
                    .with_relations(RelationLimits {
                        enabled,
                        ..Default::default()
                    }),
                );
                let mut reader = Reader::new(&facts, domain);
                assert!(!reader.domain.spec().relations().enabled);
                // Private assignment exercises the guard if Reader policy ever
                // changes; no public relation-enabled constructor is added.
                reader.domain = domain;
                let expected = original_in_context(&key, &reader.context, domain);
                if enabled {
                    let one = domain.apply(opcode::BYTE, &[constant(0), input.clone()]);
                    assert!(one.expression().is_some());
                    assert!(expected.expression().is_none());
                    assert_ne!(one, expected);
                }
                let calls = if profile == Profile::Product && !enabled {
                    (1, 0)
                } else {
                    (32, 31)
                };
                counted_union(&mut reader, &key, &expected, calls);
                counted_union(&mut reader, &key, &expected, (0, 0));
            }
        }
    }

    #[test]
    fn sources_and_selected_context_facts_get_separate_word_unions() {
        let domain = product(8, 4, 256);
        let key = expression();
        let guard = expression();
        let mut yes = AbstractValue::top();
        yes.set_origin(Origin::Calldata);
        // ConditionalFacts requires retained values; refusal flags are covered
        // separately by the direct full-value transfer controls above.
        let mut no = AbstractValue::top();
        no.set_origin(Origin::Storage);
        let then_facts = Facts::from([(key.clone(), yes)]);
        let else_facts = Facts::from([(key.clone(), no)]);
        let common = Facts::new();
        let empty = ConditionalFacts::default();
        let conditional = ConditionalFacts::join(
            (&then_facts, &empty),
            (&else_facts, &empty),
            &common,
            guard.clone(),
            ExprLimits::default(),
        )
        .unwrap();
        let context = Context::new(&common, Some(&conditional));
        let mut results = Vec::new();
        for selected in [
            context.clone(),
            context.branch(&guard, true),
            context.branch(&guard, false),
        ] {
            let expected = original_in_context(&key, &selected, domain);
            let mut reader = Reader::with_context(selected, domain);
            results.push(counted_union(&mut reader, &key, &expected, (1, 0)));
            counted_union(&mut reader, &key, &expected, (0, 0));
        }
        assert_ne!(results[0], results[1]);
        assert_ne!(results[0], results[2]);
        assert_ne!(results[1], results[2]);
        for nonzero in [false, true] {
            let selected = context.branch(&guard, true).branch(&key, nonzero);
            assert!(!scalar_in(&key, &selected, domain).numeric().is_top());
            let expected = original_in_context(&key, &selected, domain);
            let mut reader = Reader::with_context(selected, domain);
            // Selected nonzero facts also narrow the interval to start at one;
            // that interval does not contain the complete zero-based bit cube.
            counted_union(&mut reader, &key, &expected, (32, 31));
            counted_union(&mut reader, &key, &expected, (0, 0));
        }
    }

    #[test]
    fn cancelled_top_demand_still_precedes_the_warm_cache_lookup() {
        let domain = product(8, 4, 256);
        let key = expression();
        let facts = Facts::from([(key.clone(), AbstractValue::top())]);
        let expected = original_union(&key, &facts, domain);
        let mut reader = Reader::new(&facts, domain);
        counted_union(&mut reader, &key, &expected, (1, 0));
        let cache = reader.word_bytes.clone();
        let retained = reader.retained_work;
        let cancellation = analysis_control::Cancellation::new();
        cancellation.cancel();
        analysis_control::with_cancellation(&cancellation, || {
            counted_union(&mut reader, &key, &AbstractValue::top(), (0, 0));
            assert!(matches!(reader.context.check(), Err(ExprError::Cancelled)));
            assert_eq!(reader.word_bytes, cache);
            assert_eq!(reader.retained_work, retained);
        });
    }

    mod full_image {
        include!("full_image/tests.rs");
    }
}
