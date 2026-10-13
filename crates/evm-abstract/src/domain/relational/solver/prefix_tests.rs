//! Native equivalence proofs against the original complete word encoding.

use super::{CheckResult, Encoder, ExprId, QueryReason, RelationLimits, checked, word};
use crate::domain::{
    NumericValue,
    relational::SmtProvider,
    symbolic::{ByteStateId, ByteView, SourceId, StateBinding},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn c(value: u64) -> ExprId {
    ExprId::constant(U256::from(value))
}

fn policy(provider: SmtProvider) -> RelationLimits {
    RelationLimits {
        provider,
        // The independent full-word oracle encodes the discarded 28 bytes.
        max_nodes: 8192,
        max_depth: 128,
        ..RelationLimits::default()
    }
}

fn selector(read: ExprId, op: u8, limits: &RelationLimits) -> ExprId {
    let args = if op == opcode::SHR {
        [c(224), read]
    } else {
        [read, ExprId::constant(U256::ONE << 224)]
    };
    ExprId::operation(op, &args, limits.expressions()).unwrap()
}

fn equivalent(view: ByteView, offset: ExprId, limits: RelationLimits) {
    for op in [opcode::SHR, opcode::DIV] {
        let read = ExprId::word_read(view.clone(), offset.clone(), limits.expressions()).unwrap();
        let expression = selector(read.clone(), op, &limits);
        let mut encoder = Encoder::new(&limits);
        let prefix = encoder.expression(&expression).unwrap();
        assert_eq!(prefix.width(), 256);
        if matches!(read.kind(), super::ExprKind::WordRead { .. }) {
            assert!(
                !encoder.memo.contains_key(&read),
                "a selector must not memoize a truncated value as its full word"
            );
        }
        // Calling word_read directly bypasses the new operation lowering. The
        // same encoder retains identical SourceId arrays and observed bindings.
        let full = encoder.word_read(&view, &offset).unwrap();
        let args = if op == opcode::SHR {
            [word(U256::from(224)), full]
        } else {
            [full, word(U256::ONE << 224)]
        };
        let reference = encoder.operation(op, &args, None).unwrap();
        let mut assertions = Vec::new();
        encoder.emit_definitions(&mut assertions);
        assertions.push(prefix.eq(&reference).not());
        assert_eq!(
            checked(&assertions, &limits),
            CheckResult::Unsat,
            "{:?}: opcode={op:#x}, view={view:?}, offset={offset:?}",
            limits.provider
        );
    }
}

#[test]
fn selector_prefix_matches_full_word_with_short_and_symbolic_padding() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        for length in [0, 1, 3, 4, 32] {
            let input = ByteStateId::input(SourceId::fresh().unwrap(), c(length));
            // A 32-byte view still obeys the shorter backing source's padding.
            equivalent(ByteView::new(input, c(0), c(32)), c(0), limits);
        }
        let length = ExprId::fresh().unwrap();
        let input = ByteStateId::input(SourceId::fresh().unwrap(), length.clone());
        equivalent(ByteView::new(input.clone(), c(0), length), c(0), limits);
        // Short view padding is separate from the backing source length.
        for length in [0, 1, 3, 4] {
            equivalent(ByteView::new(input.clone(), c(0), c(length)), c(0), limits);
        }
    }
}

#[test]
fn selector_prefix_preserves_relative_and_backing_mathematical_overflow() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let input = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::MAX));
        for start in [U256::MAX - U256::from(2), U256::MAX] {
            equivalent(
                ByteView::new(input.clone(), ExprId::constant(start), c(32)),
                c(0),
                limits,
            );
            equivalent(
                ByteView::new(input.clone(), c(0), ExprId::constant(U256::MAX)),
                ExprId::constant(start),
                limits,
            );
        }
        equivalent(
            ByteView::new(input, ExprId::fresh().unwrap(), c(32)),
            ExprId::fresh().unwrap(),
            limits,
        );
    }
}

#[test]
fn selector_prefix_retains_captured_copy_and_observed_source_bindings() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let input = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            c(4),
            vec![StateBinding {
                index: U256::ZERO,
                value: ExprId::fresh().unwrap(),
                numeric: NumericValue::constant(U256::from(0xab)),
            }],
            limits.expressions().into(),
        )
        .unwrap();
        let view = ByteView::new(input, c(0), c(4));
        let copied = ByteStateId::zero()
            .copy(c(128), view.clone(), c(4), limits.expressions().into())
            .unwrap();
        equivalent(ByteView::new(copied, c(128), c(32)), c(0), limits);
        let read = ExprId::word_read(view, c(0), limits.expressions()).unwrap();
        let mut encoder = Encoder::new(&limits);
        let value = encoder
            .expression(&selector(read, opcode::DIV, &limits))
            .unwrap();
        let mut assertions = Vec::new();
        encoder.emit_definitions(&mut assertions);
        assertions.push(
            value
                .extract(31, 24)
                .eq(word(U256::from(0xab)).extract(7, 0))
                .not(),
        );
        assert_eq!(checked(&assertions, &limits), CheckResult::Unsat);
    }
}

#[test]
fn selector_prefix_keeps_independent_sources_independent() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = policy(provider);
        let mut encoder = Encoder::new(&limits);
        let mut values = Vec::new();
        for _ in 0..2 {
            let input = ByteStateId::input(SourceId::fresh().unwrap(), c(4));
            let read =
                ExprId::word_read(ByteView::new(input, c(0), c(4)), c(0), limits.expressions())
                    .unwrap();
            values.push(
                encoder
                    .expression(&selector(read, opcode::SHR, &limits))
                    .unwrap(),
            );
        }
        let mut assertions = Vec::new();
        encoder.emit_definitions(&mut assertions);
        assertions.push(values[0].eq(&values[1]).not());
        assert_eq!(checked(&assertions, &limits), CheckResult::Sat);
    }
}

#[test]
fn nonselector_operations_keep_the_full_word_fallback() {
    let limits = RelationLimits::default();
    let input = ByteStateId::input(SourceId::fresh().unwrap(), c(32));
    let read = ExprId::word_read(
        ByteView::new(input, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let mut encoder = Encoder::new(&limits);
    for (op, args) in [
        (opcode::SHR, [c(223), read.clone()]),
        (opcode::SHR, [c(216), read.clone()]),
        (opcode::SHR, [ExprId::fresh().unwrap(), read.clone()]),
        (
            opcode::DIV,
            [read.clone(), ExprId::constant(U256::ONE << 223)],
        ),
        (opcode::DIV, [read.clone(), c(3)]),
        (
            opcode::SDIV,
            [read.clone(), ExprId::constant(U256::ONE << 224)],
        ),
        (opcode::SHR, [c(224), ExprId::fresh().unwrap()]),
        (opcode::DIV, [ExprId::constant(U256::ONE << 224), read]),
    ] {
        assert!(encoder.selector_prefix(op, &args).unwrap().is_none());
    }
}

#[test]
fn selector_prefix_respects_encoding_budget_and_cancellation() {
    let limits = RelationLimits::default();
    let input = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::fresh().unwrap());
    let read = ExprId::word_read(
        ByteView::new(input, c(0), c(32)),
        c(0),
        limits.expressions(),
    )
    .unwrap();
    let expression = selector(read, opcode::SHR, &limits);
    let tiny = RelationLimits {
        max_nodes: expression.nodes(),
        ..limits
    };
    assert_eq!(
        Encoder::new(&tiny).expression(&expression).unwrap_err(),
        QueryReason::ExpressionLimit
    );
    let token = embedded_smt::Cancellation::new();
    token.cancel();
    let result =
        embedded_smt::with_cancellation(&token, || Encoder::new(&limits).expression(&expression));
    assert_eq!(result.unwrap_err(), QueryReason::Cancelled);
}
