use super::super::{ROOT, arguments, expression, word_op, words};
use evm_decompile_model::{
    EntityId, LiftedExpressionKind as Kind, LiftedRelation, LiftedRenderError, LiftedValue, U256,
};

#[test]
fn operators_and_positional_word_helpers_preserve_pop_order() {
    for (opcode, count, expected) in [
        (0x05, 2, "evm.sdiv(0x11, 0x22)"),
        (0x07, 2, "evm.smod(0x11, 0x22)"),
        (0x08, 3, "evm.addmod(0x11, 0x22, 0x33)"),
        (0x09, 3, "evm.mulmod(0x11, 0x22, 0x33)"),
        (0x0a, 2, "(0x11 ** 0x22)"),
        (0x0b, 2, "evm.signextend(0x11, 0x22)"),
        (0x12, 2, "evm.slt(0x11, 0x22)"),
        (0x13, 2, "evm.sgt(0x11, 0x22)"),
        (0x15, 1, "word((0x11 == 0))"),
        (0x19, 1, "(~0x11)"),
        (0x1a, 2, "evm.byte(0x11, 0x22)"),
        (0x1b, 2, "(0x22 << 0x11)"),
        (0x1c, 2, "(0x22 >> 0x11)"),
        (0x1d, 2, "evm.sar(0x11, 0x22)"),
        (0x1e, 1, "evm.clz(0x11)"),
    ] {
        let program = word_op(opcode, count);
        program.validate().unwrap();
        let original = program.clone();
        assert_eq!(
            program.render_expression(ROOT),
            expected,
            "opcode={opcode:#x}"
        );
        assert_eq!(program, original);
    }
}

#[test]
fn signed_predicates_include_positional_synthetic_sle_and_sge() {
    for (relation, expected) in [
        (LiftedRelation::Slt, "evm.slt(0x11, 0x22)"),
        (LiftedRelation::Sle, "evm.sle(0x11, 0x22)"),
        (LiftedRelation::Sgt, "evm.sgt(0x11, 0x22)"),
        (LiftedRelation::Sge, "evm.sge(0x11, 0x22)"),
    ] {
        let mut program = words(2);
        program.expressions.push(expression(
            ROOT,
            Kind::Compare {
                relation,
                left: EntityId(1),
                right: EntityId(2),
            },
        ));
        program.validate().unwrap();
        assert_eq!(program.render_expression(ROOT), expected);
    }
}

#[test]
fn infix_expressions_and_numeric_predicate_cast_keep_their_existing_syntax() {
    for (opcode, expected) in [
        (0x01, "(0x11 + 0x22)"),
        (0x02, "(0x11 * 0x22)"),
        (0x03, "(0x11 - 0x22)"),
        (0x04, "(0x11 / 0x22)"),
        (0x06, "(0x11 % 0x22)"),
        (0x16, "(0x11 & 0x22)"),
        (0x17, "(0x11 | 0x22)"),
        (0x18, "(0x11 ^ 0x22)"),
    ] {
        assert_eq!(word_op(opcode, 2).render_expression(ROOT), expected);
    }
    for (relation, expected) in [
        (LiftedRelation::Eq, "(0x11 == 0x22)"),
        (LiftedRelation::Ne, "(0x11 != 0x22)"),
        (LiftedRelation::Ult, "(0x11 < 0x22)"),
        (LiftedRelation::Ule, "(0x11 <= 0x22)"),
        (LiftedRelation::Ugt, "(0x11 > 0x22)"),
        (LiftedRelation::Uge, "(0x11 >= 0x22)"),
    ] {
        let mut program = words(2);
        program.expressions.push(expression(
            ROOT,
            Kind::Compare {
                relation,
                left: EntityId(1),
                right: EntityId(2),
            },
        ));
        program
            .expressions
            .push(expression(EntityId(101), Kind::WordFromPredicate(ROOT)));
        program.validate().unwrap();
        assert_eq!(program.render_expression(ROOT), expected);
        assert_eq!(
            program.render_expression(EntityId(101)),
            format!("word({expected})")
        );
    }
    let mut program = words(1);
    program.expressions.extend([
        expression(EntityId(2), Kind::NonZero(EntityId(1))),
        expression(EntityId(3), Kind::Not(EntityId(2))),
        expression(EntityId(4), Kind::WordFromPredicate(EntityId(3))),
    ]);
    assert_eq!(
        program.render_expression(EntityId(4)),
        "word(!((0x11 != 0)))"
    );
}

#[test]
fn malformed_and_unknown_word_functions_keep_every_positional_argument() {
    for (opcode, count, expected) in [
        (0x0a, 0, "evm.exp()"),
        (0x0a, 1, "evm.exp(0x11)"),
        (0x0a, 3, "evm.exp(0x11, 0x22, 0x33)"),
        (0x08, 2, "evm.addmod(0x11, 0x22)"),
        (0x08, 4, "evm.addmod(0x11, 0x22, 0x33, 0x44)"),
        (0x19, 2, "evm.not(0x11, 0x22)"),
        (0x1b, 3, "evm.shl(0x11, 0x22, 0x33)"),
        (0x1c, 1, "evm.shr(0x11)"),
        (0x15, 2, "evm.iszero(0x11, 0x22)"),
        (0x01, 3, "evm.word_op(0x11, 0x22, 0x33)"),
        (0x0c, 0, "evm.word_op()"),
        (0x0c, 3, "evm.word_op(0x11, 0x22, 0x33)"),
    ] {
        let program = word_op(opcode, count);
        assert_eq!(program.render_expression(ROOT), expected);
        assert_eq!(
            program.expressions.last().unwrap().kind,
            Kind::WordOp {
                opcode,
                arguments: arguments(count),
            }
        );
    }
}

#[test]
fn word_comparisons_keep_the_numeric_cast_and_predicates_stay_boolean() {
    for (opcode, relation, predicate) in [
        (0x10, LiftedRelation::Ult, "(0x11 < 0x22)"),
        (0x11, LiftedRelation::Ugt, "(0x11 > 0x22)"),
        (0x14, LiftedRelation::Eq, "(0x11 == 0x22)"),
    ] {
        let mut program = word_op(opcode, 2);
        let comparison = EntityId(ROOT.0 + 1);
        program.expressions.push(expression(
            comparison,
            Kind::Compare {
                relation,
                left: EntityId(1),
                right: EntityId(2),
            },
        ));
        program.validate().unwrap();
        let original = program.clone();
        let ir = Default::default();
        let renderer = program.render_index(&ir);
        assert_eq!(
            program.render_expression(ROOT),
            format!("word({predicate})")
        );
        assert_eq!(program.render_expression(comparison), predicate);
        assert_eq!(
            renderer
                .render_condition(comparison, false, |_| true)
                .unwrap(),
            predicate
        );
        assert_eq!(
            renderer.render_condition(ROOT, false, |_| true),
            Err(LiftedRenderError::MissingWitness(ROOT))
        );
        let expected = opcode == 0x10;
        assert_eq!(
            program.evaluate_expression(ROOT, |_| None),
            Some(LiftedValue::Word(U256::from(u8::from(expected))))
        );
        assert_eq!(
            program.evaluate_expression(comparison, |_| None),
            Some(LiftedValue::Predicate(expected))
        );
        assert_eq!(program, original);
    }
    let program = word_op(0x15, 1);
    assert_eq!(program.render_expression(ROOT), "word((0x11 == 0))");
    assert_eq!(
        program.evaluate_expression(ROOT, |_| None),
        Some(LiftedValue::Word(U256::ZERO))
    );
}

#[test]
fn nested_exponent_xor_not_and_shifts_keep_each_expression_boundary() {
    let mut program = words(3);
    program.expressions.extend([
        expression(
            EntityId(4),
            Kind::WordOp {
                opcode: 0x01,
                arguments: vec![EntityId(1), EntityId(2)],
            },
        ),
        expression(
            EntityId(5),
            Kind::WordOp {
                opcode: 0x18,
                arguments: vec![EntityId(2), EntityId(3)],
            },
        ),
        expression(
            EntityId(6),
            Kind::WordOp {
                opcode: 0x0a,
                arguments: vec![EntityId(4), EntityId(5)],
            },
        ),
        expression(
            EntityId(7),
            Kind::WordOp {
                opcode: 0x19,
                arguments: vec![EntityId(6)],
            },
        ),
        expression(
            EntityId(8),
            Kind::WordOp {
                opcode: 0x1b,
                arguments: vec![EntityId(1), EntityId(7)],
            },
        ),
        expression(
            EntityId(9),
            Kind::WordOp {
                opcode: 0x1c,
                arguments: vec![EntityId(2), EntityId(8)],
            },
        ),
    ]);
    program.validate().unwrap();
    assert_eq!(
        program.render_expression(EntityId(9)),
        "(((~((0x11 + 0x22) ** (0x22 ^ 0x33))) << 0x11) >> 0x22)"
    );
}

#[test]
fn exponent_association_follows_the_arena_tree() {
    let mut program = words(3);
    for (id, arguments) in [(4, [1, 2]), (5, [4, 3]), (6, [2, 3]), (7, [1, 6])] {
        program.expressions.push(expression(
            EntityId(id),
            Kind::WordOp {
                opcode: 0x0a,
                arguments: arguments.map(EntityId).into(),
            },
        ));
    }
    program.validate().unwrap();
    assert_eq!(
        program.render_expression(EntityId(5)),
        "((0x11 ** 0x22) ** 0x33)"
    );
    assert_eq!(
        program.render_expression(EntityId(7)),
        "(0x11 ** (0x22 ** 0x33))"
    );
}

#[test]
fn an_or_of_numeric_predicates_remains_a_bitwise_word_expression() {
    let mut program = words(2);
    program.expressions.extend([
        expression(
            EntityId(3),
            Kind::Compare {
                relation: LiftedRelation::Ult,
                left: EntityId(1),
                right: EntityId(2),
            },
        ),
        expression(EntityId(4), Kind::NonZero(EntityId(1))),
        expression(EntityId(5), Kind::WordFromPredicate(EntityId(3))),
        expression(EntityId(6), Kind::WordFromPredicate(EntityId(4))),
        expression(
            EntityId(7),
            Kind::WordOp {
                opcode: 0x17,
                arguments: vec![EntityId(5), EntityId(6)],
            },
        ),
        expression(EntityId(8), Kind::NonZero(EntityId(7))),
    ]);
    program.validate().unwrap();
    let ir = Default::default();
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(8), false, |_| true)
            .unwrap(),
        "((word((0x11 < 0x22)) | word((0x11 != 0))) != 0)"
    );
}

#[test]
fn modular_helpers_keep_the_wide_intermediate_operation() {
    for (opcode, name, right, result) in [
        (0x08, "addmod", U256::from(1), U256::from(1)),
        (0x09, "mulmod", U256::MAX, U256::ZERO),
    ] {
        let mut program = word_op(opcode, 3);
        program.expressions[0].kind = Kind::Word(U256::MAX);
        program.expressions[1].kind = Kind::Word(right);
        program.expressions[2].kind = Kind::Word(U256::from(3));
        program.validate().unwrap();
        assert_eq!(
            program.render_expression(ROOT),
            format!("evm.{name}(0x{:x}, 0x{right:x}, 0x3)", U256::MAX)
        );
        assert_eq!(
            program.evaluate_expression(ROOT, |_| None),
            Some(LiftedValue::Word(result))
        );
    }
}
