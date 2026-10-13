//! Whole-word source reads compared with the independent demanded-byte encoder.

use super::{BV, Encoder, ExprId, valid_sum, word};
use crate::domain::{
    NumericValue,
    relational::{CheckResult, QueryReason, RelationLimits, SmtProvider},
    symbolic::{ByteStateId, ByteView, SourceId, StateBinding},
};
use alloy_primitives::U256;
use embedded_smt::Bool;

use super::super::super::checked;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        max_nodes: 8192,
        max_depth: 128,
        // The differential oracle retains all 32 independent bounds. This is
        // the workbench's default solver allowance, not a production change.
        rlimit: 10_000_000,
        ..RelationLimits::default()
    }
}

fn reference_word(encoder: &mut Encoder<'_>, view: &ByteView, offset: &ExprId) -> BV {
    let offset = encoder.expression(offset).unwrap();
    let mut result = word(U256::ZERO);
    for j in 0..32 {
        let (index, valid) = valid_sum(&offset, &word(U256::from(j)));
        let byte = encoder.view_byte(view, &index).unwrap();
        let byte = valid.ite(&byte, &BV::from_u64(0, 8));
        // Sequential concatenation also avoids sharing the optimized packing
        // tree. No whole-word read or prefix-mask helper enters this oracle.
        result = result.bvshl(word(U256::from(8))).bvor(byte.zero_ext(248));
    }
    result
}

fn equivalent(
    encoder: &mut Encoder<'_>,
    view: &ByteView,
    fast: BV,
    offset: &ExprId,
    mut assumptions: Vec<Bool>,
    limits: &RelationLimits,
) {
    let reference = reference_word(encoder, view, offset);
    encoder.emit_definitions(&mut assumptions);
    assumptions.push(fast.eq(&reference).not());
    assert_eq!(
        checked(&assumptions, limits),
        CheckResult::Unsat,
        "{:?}: view={view:?}, relative offset={offset:?}",
        limits.provider
    );
}

#[test]
fn dynamic_input_word_matches_byte_reference_for_symbolic_length_and_start() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::fresh().unwrap();
        let start = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
        for requested in [c(32), length.clone()] {
            let view = ByteView::new(input.clone(), start.clone(), requested);
            let mut encoder = Encoder::new(&limits);
            let fast = encoder.input_word(&view).unwrap().unwrap();
            // Every independent byte miter quantifies over the complete source,
            // length and start. All 32 UNSAT results imply the full word is
            // equal, without asking one solver call to combine every circuit.
            for j in 0..32 {
                let reference = encoder.view_byte(&view, &word(U256::from(j))).unwrap();
                let high = 255 - j * 8;
                let mut assertions = Vec::new();
                encoder.emit_definitions(&mut assertions);
                assertions.push(fast.extract(high, high - 7).eq(&reference).not());
                assert_eq!(
                    checked(&assertions, &limits),
                    CheckResult::Unsat,
                    "{provider:?}: byte={j}, view={view:?}"
                );
            }
        }
    }
}

#[test]
fn dynamic_and_max_input_words_preserve_length_and_overflow_boundaries() {
    let max = U256::MAX;
    let cases = [
        (U256::ZERO, U256::ZERO),
        (U256::ZERO, U256::ONE),
        (U256::ZERO, U256::from(31)),
        (U256::ZERO, U256::from(32)),
        (U256::ONE, U256::ONE),
        (U256::ONE, U256::from(2)),
        (U256::from(32), U256::from(31)),
        (U256::from(32), U256::from(33)),
        (max - U256::from(32), max),
        (max - U256::from(31), max),
        (max - U256::from(16), max),
        (max - U256::ONE, max),
        (max, max),
        (max, U256::ZERO),
    ];
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for (at, size) in cases {
            for dynamic_start in [false, true] {
                let start = if dynamic_start {
                    ExprId::fresh().unwrap()
                } else {
                    ExprId::constant(at)
                };
                let length = ExprId::fresh().unwrap();
                let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
                let view = ByteView::new(input, start.clone(), c(32));
                let mut encoder = Encoder::new(&limits);
                let fast = encoder.input_word(&view).unwrap().unwrap();
                let assumptions = vec![
                    encoder.expression(&start).unwrap().eq(word(at)),
                    encoder.expression(&length).unwrap().eq(word(size)),
                ];
                equivalent(&mut encoder, &view, fast, &c(0), assumptions, &limits);
            }
        }
    }
}

#[test]
fn input_words_retain_short_unrelated_views_and_relative_offset_guards() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length);
        for requested in [c(0), c(1), c(31), ExprId::fresh().unwrap()] {
            let view = ByteView::new(input.clone(), c(0), requested);
            let mut encoder = Encoder::new(&limits);
            assert!(encoder.input_word(&view).unwrap().is_none());
            let read = encoder.word_read(&view, &c(0)).unwrap();
            equivalent(&mut encoder, &view, read, &c(0), Vec::new(), &limits);
        }
        let view = ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::MAX)),
            c(0),
            ExprId::constant(U256::MAX),
        );
        // A nonzero relative offset cannot enter input_word without preserving
        // its independent mathematical overflow and view-boundary guards.
        let offset = ExprId::constant(U256::MAX - U256::ONE);
        let mut encoder = Encoder::new(&limits);
        let read = encoder.word_read(&view, &offset).unwrap();
        equivalent(&mut encoder, &view, read, &offset, Vec::new(), &limits);
    }
}

#[test]
fn dynamic_input_word_preserves_observed_bytes_and_masks_wrapped_low_cells() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let length = ExprId::fresh().unwrap();
        let start = ExprId::fresh().unwrap();
        let input = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            length.clone(),
            [
                (U256::ZERO, 0xb7),
                (U256::MAX - U256::ONE, 0x55),
                (U256::MAX, 0xee),
            ]
            .into_iter()
            .map(|(index, byte)| StateBinding {
                index,
                value: ExprId::fresh().unwrap(),
                numeric: NumericValue::constant(U256::from(byte)),
            })
            .collect(),
            limits.expressions().into(),
        )
        .unwrap();
        let view = ByteView::new(input, start.clone(), c(32));
        for (at, size, first) in [
            (U256::ZERO, U256::ONE, 0xb7),
            (U256::MAX - U256::ONE, U256::MAX, 0x55),
            (U256::MAX, U256::MAX, 0),
        ] {
            let mut encoder = Encoder::new(&limits);
            let fast = encoder.input_word(&view).unwrap().unwrap();
            let mut assumptions = vec![
                encoder.expression(&start).unwrap().eq(word(at)),
                encoder.expression(&length).unwrap().eq(word(size)),
            ];
            // Retain the initial source bindings for both independent checks;
            // each emit advances the encoder's incremental definition cursor.
            encoder.emit_definitions(&mut assumptions);
            equivalent(
                &mut encoder,
                &view,
                fast.clone(),
                &c(0),
                assumptions.clone(),
                &limits,
            );
            encoder.emit_definitions(&mut assumptions);
            assumptions.push(fast.eq(word(U256::from(first) << 248)).not());
            assert_eq!(checked(&assumptions, &limits), CheckResult::Unsat);
        }
    }
}

#[test]
fn a_fully_masked_input_word_still_asserts_inconsistent_observed_bindings() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for (start, length) in [(U256::ZERO, U256::ZERO), (U256::MAX, U256::MAX)] {
            let input = ByteStateId::observed(
                SourceId::fresh().unwrap(),
                ExprId::constant(length),
                vec![StateBinding {
                    index: U256::ZERO,
                    value: c(0xcc),
                    numeric: NumericValue::constant(U256::from(0xb7)),
                }],
                limits.expressions().into(),
            )
            .unwrap();
            let view = ByteView::new(input, ExprId::constant(start), c(32));
            let mut encoder = Encoder::new(&limits);
            let read = encoder.input_word(&view).unwrap().unwrap();
            let mut assertions = Vec::new();
            encoder.emit_definitions(&mut assertions);
            assertions.push(read.eq(word(U256::ZERO)));
            assert_eq!(checked(&assertions, &limits), CheckResult::Unsat);
        }
    }
}

#[test]
fn input_word_encoding_respects_low_budget_and_cancellation() {
    let limits = RelationLimits {
        max_nodes: 16,
        ..RelationLimits::default()
    };
    let input = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::fresh().unwrap());
    for start in [c(0), ExprId::fresh().unwrap(), ExprId::constant(U256::MAX)] {
        let view = ByteView::new(input.clone(), start, c(32));
        let mut encoder = Encoder::new(&limits);
        assert!(matches!(
            encoder.input_word(&view),
            Err(QueryReason::ExpressionLimit)
        ));
        let cancellation = embedded_smt::Cancellation::new();
        cancellation.cancel();
        embedded_smt::with_cancellation(&cancellation, || {
            let normal_limits = RelationLimits::default();
            let mut encoder = Encoder::new(&normal_limits);
            assert!(matches!(
                encoder.input_word(&view),
                Err(QueryReason::Cancelled)
            ));
        });
    }
}

#[test]
fn input_word_masks_keep_shared_sources_correlated_and_separate_sources_independent() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let input = ByteStateId::input(SourceId::fresh().unwrap(), c(32));
        let mut encoder = Encoder::new(&limits);
        let view = ByteView::new(input, c(0), c(32));
        let first = encoder.input_word(&view).unwrap().unwrap();
        let again = encoder.input_word(&view).unwrap().unwrap();
        let independent = encoder
            .input_word(&ByteView::new(
                ByteStateId::input(SourceId::fresh().unwrap(), c(32)),
                c(0),
                c(32),
            ))
            .unwrap()
            .unwrap();
        let mut definitions = Vec::new();
        encoder.emit_definitions(&mut definitions);
        let mut assertions = definitions.clone();
        assertions.push(first.eq(&again).not());
        assert_eq!(checked(&assertions, &limits), CheckResult::Unsat);
        definitions.push(first.eq(&independent).not());
        assert_eq!(checked(&definitions, &limits), CheckResult::Sat);
    }
}
