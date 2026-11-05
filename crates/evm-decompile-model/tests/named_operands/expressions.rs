use super::super::{ROOT, arguments, expression, word_op, words};
use evm_decompile_model::{EntityId, LiftedExpressionKind as Kind, LiftedRelation};

#[test]
fn meaningful_word_function_forms_name_distinct_operands() {
    for (opcode, count, expected) in [
        (0x05, 2, "sdiv(dividend = 0x11, divisor = 0x22)"),
        (0x07, 2, "smod(dividend = 0x11, divisor = 0x22)"),
        (0x08, 3, "addmod(left = 0x11, right = 0x22, modulus = 0x33)"),
        (0x09, 3, "mulmod(left = 0x11, right = 0x22, modulus = 0x33)"),
        (0x0a, 2, "exp(base = 0x11, exponent = 0x22)"),
        (0x0b, 2, "signextend(byte_index = 0x11, value = 0x22)"),
        (0x15, 1, "iszero(value = 0x11)"),
        (0x19, 1, "bitnot(value = 0x11)"),
        (0x1a, 2, "byte(index = 0x11, value = 0x22)"),
        (0x1b, 2, "shl(shift = 0x11, value = 0x22)"),
        (0x1c, 2, "shr(shift = 0x11, value = 0x22)"),
        (0x1d, 2, "sar(shift = 0x11, value = 0x22)"),
        (0x1e, 1, "clz(value = 0x11)"),
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
fn signed_comparisons_include_synthetic_sle_and_sge_with_left_and_right() {
    for (relation, expected) in [
        (LiftedRelation::Slt, "slt(left = 0x11, right = 0x22)"),
        (LiftedRelation::Sle, "sle(left = 0x11, right = 0x22)"),
        (LiftedRelation::Sgt, "sgt(left = 0x11, right = 0x22)"),
        (LiftedRelation::Sge, "sge(left = 0x11, right = 0x22)"),
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
        (0x0a, 0, "exp()"),
        (0x0a, 1, "exp(0x11)"),
        (0x0a, 3, "exp(0x11, 0x22, 0x33)"),
        (0x08, 2, "addmod(0x11, 0x22)"),
        (0x08, 4, "addmod(0x11, 0x22, 0x33, 0x44)"),
        (0x19, 2, "bitnot(0x11, 0x22)"),
        (0x01, 3, "word_op(0x11, 0x22, 0x33)"),
        (0x0c, 0, "word_op()"),
        (0x0c, 3, "word_op(0x11, 0x22, 0x33)"),
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
