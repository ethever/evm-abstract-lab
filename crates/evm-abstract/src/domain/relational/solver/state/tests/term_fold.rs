use super::{
    BV, Bool, ByteStateId, ByteView, Encoder, ExprId, NumericValue, RelationLimits, RelationState,
    SmtProvider, SourceId, StateBinding, U256, c, word,
};
use crate::domain::relational::{CheckResult, QueryReason};

#[test]
fn literal_byte_guards_preserve_copied_word_and_padding_relationships() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        let value = ExprId::fresh().unwrap();
        let source = ByteStateId::zero()
            .write_word(c(8), value.clone(), limits.expressions().into())
            .unwrap();
        let copied = ByteStateId::zero()
            .copy(
                c(64),
                ByteView::new(source, c(8), c(32)),
                c(32),
                limits.expressions().into(),
            )
            .unwrap();
        let (mut assertions, mut encoder) =
            super::super::super::prepared(&RelationState::default(), None, &limits).unwrap();
        let encoded = encoder.expression(&value).unwrap();
        let mut different = Vec::new();
        for index in [63, 64, 65, 95, 96] {
            let read = ExprId::byte_read(copied.clone(), c(index), limits.expressions()).unwrap();
            let actual = encoder.expression(&read).unwrap();
            let expected = if (64..96).contains(&index) {
                let high = 255 - (index as u32 - 64) * 8;
                encoded.extract(high, high - 7).zero_ext(248)
            } else {
                word(U256::ZERO)
            };
            different.push(actual.eq(&expected).not());
        }
        encoder.emit_definitions(&mut assertions);
        assertions.push(Bool::or(&different));
        assert_eq!(
            super::super::super::checked(&assertions, &limits),
            CheckResult::Unsat,
            "{provider:?}"
        );
    }
}

#[test]
fn selected_write_keeps_unselected_observation_facts_and_encoding_charges() {
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        let observed = ExprId::fresh().unwrap();
        let previous = ByteStateId::observed(
            SourceId::fresh().unwrap(),
            c(1),
            vec![StateBinding {
                index: U256::ZERO,
                value: observed.clone(),
                // An inconsistent byte observation remains UNSAT even though
                // a later write supplies the demanded value at this address.
                numeric: NumericValue::constant(U256::from(256)),
            }],
            limits.expressions().into(),
        )
        .unwrap();
        let state = previous
            .clone()
            .write_word(c(0), ExprId::fresh().unwrap(), limits.expressions().into())
            .unwrap();
        let read = ExprId::byte_read(state, c(0), limits.expressions()).unwrap();
        let mut encoder = Encoder::new(&limits);
        let value = encoder.expression(&read).unwrap();
        assert!(encoder.observed_bytes.contains(&previous));
        assert!(encoder.memo.contains_key(&observed));
        assert!(!encoder.definitions.is_empty());
        let mut assertions = Vec::new();
        encoder.emit_definitions(&mut assertions);
        assert_eq!(
            embedded_smt::check(&assertions, Some(&value), provider, limits.rlimit),
            embedded_smt::Outcome::Unsat
        );
        let tiny = RelationLimits {
            max_nodes: encoder.work - 1,
            ..limits
        };
        assert!(matches!(
            Encoder::new(&tiny).expression(&read),
            Err(QueryReason::ExpressionLimit)
        ));
    }
}

#[test]
fn unselected_written_expression_still_obeys_the_node_limit() {
    let limits = RelationLimits::default();
    let expensive = ExprId::operation(
        revm_bytecode::opcode::EXP,
        &[ExprId::fresh().unwrap(), ExprId::fresh().unwrap()],
        limits.expressions(),
    )
    .unwrap();
    let memory = ByteStateId::zero()
        .write_word(c(128), expensive, limits.expressions().into())
        .unwrap();
    // The byte is outside the write; encoding the written branch must still
    // charge its original exponent work before choosing the old zero byte.
    assert!(matches!(
        Encoder::new(&limits).byte(&memory, &word(U256::ZERO)),
        Err(QueryReason::ExpressionLimit)
    ));
}

#[test]
#[should_panic(expected = "conditional branches must have equal widths")]
fn local_choose_checks_width_before_selection() {
    super::super::choose(
        &Bool::from_bool(false),
        &BV::from_u64(0, 8),
        &BV::from_u64(0, 9),
    );
}
