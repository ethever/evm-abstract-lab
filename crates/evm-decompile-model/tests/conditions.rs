//! Check normalized predicates against independently evaluated word boundaries.
use evm_decompile_model::{
    EntityId, Ir, LiftedExpression, LiftedExpressionKind as Kind, LiftedProgram,
    LiftedRelation as Relation, LiftedRenderError, LiftedValue, MAX_LIFTED_EXPRESSION_TEXT, U256,
};

fn expression(id: u64, kind: Kind) -> LiftedExpression {
    LiftedExpression {
        id: EntityId(id),
        block: EntityId(100),
        ty: kind.ty(),
        kind,
    }
}

fn truth(text: &str, left: U256, right: U256) -> bool {
    if let Some(inner) = text
        .strip_prefix("!(")
        .and_then(|text| text.strip_suffix(')'))
    {
        return !truth(inner, left, right);
    }
    match text {
        "(v101 == v102)" => left == right,
        "(v101 != v102)" => left != right,
        "(v101 < v102)" => left < right,
        "(v101 <= v102)" => left <= right,
        "(v101 > v102)" => left > right,
        "(v101 >= v102)" => left >= right,
        "evm.slt(v101, v102)" => {
            evm_bytecode::word::evaluate(0x12, &[left, right]).unwrap() != U256::ZERO
        }
        "evm.sgt(v101, v102)" => {
            evm_bytecode::word::evaluate(0x13, &[left, right]).unwrap() != U256::ZERO
        }
        other => panic!("unexpected predicate spelling {other}"),
    }
}

#[test]
fn comparison_polarities_preserve_unsigned_and_signed_word_boundaries() {
    let boundaries = [
        U256::ZERO,
        U256::from(1),
        U256::from(1) << 255,
        (U256::from(1) << 255) - U256::from(1),
        U256::MAX,
    ];
    for relation in [
        Relation::Eq,
        Relation::Ne,
        Relation::Ult,
        Relation::Ule,
        Relation::Ugt,
        Relation::Uge,
        Relation::Slt,
        Relation::Sle,
        Relation::Sgt,
        Relation::Sge,
    ] {
        let program = LiftedProgram {
            expressions: vec![
                expression(1, Kind::Value(EntityId(101))),
                expression(2, Kind::Value(EntityId(102))),
                expression(
                    3,
                    Kind::Compare {
                        relation,
                        left: EntityId(1),
                        right: EntityId(2),
                    },
                ),
            ],
            ..LiftedProgram::default()
        };
        program.validate().unwrap();
        let ir = Ir::default();
        let renderer = program.render_index(&ir);
        for invert in [false, true] {
            let text = renderer
                .render_condition(EntityId(3), invert, |_| true)
                .unwrap();
            assert!(!text.contains("evm.sle") && !text.contains("evm.sge"));
            for left in boundaries {
                for right in boundaries {
                    let original = program.evaluate_expression(EntityId(3), |id| match id.0 {
                        101 => Some(left),
                        102 => Some(right),
                        _ => None,
                    });
                    assert_eq!(
                        original,
                        Some(LiftedValue::Predicate(truth(&text, left, right) != invert)),
                        "relation={relation:?}, invert={invert}, left={left}, right={right}: {text}"
                    );
                }
            }
        }
    }
}

#[test]
fn double_negation_and_inverted_truth_tests_keep_materialized_words() {
    let program = LiftedProgram {
        expressions: vec![
            expression(1, Kind::Value(EntityId(12563))),
            expression(2, Kind::NonZero(EntityId(1))),
            expression(3, Kind::Not(EntityId(2))),
            expression(4, Kind::Not(EntityId(3))),
            expression(5, Kind::WordFromPredicate(EntityId(4))),
            expression(6, Kind::NonZero(EntityId(5))),
        ],
        ..LiftedProgram::default()
    };
    program.validate().unwrap();
    let original = program.clone();
    let ir = Ir::default();
    let renderer = program.render_index(&ir);
    for root in [2, 4, 6] {
        assert_eq!(
            renderer
                .render_condition(EntityId(root), false, |_| true)
                .unwrap(),
            "(v12563 != 0)"
        );
        assert_eq!(
            renderer
                .render_condition(EntityId(root), true, |_| true)
                .unwrap(),
            "(v12563 == 0)"
        );
        assert_eq!(program.evaluate_expression(EntityId(root), |_| None), None);
        for value in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
            assert_eq!(
                program.evaluate_expression(EntityId(root), |_| Some(value)),
                Some(LiftedValue::Predicate(value != U256::ZERO))
            );
        }
    }
    // The explicit numeric cast remains available to non-condition consumers.
    assert!(
        renderer
            .render_expression(EntityId(5), |_| true)
            .unwrap()
            .starts_with("word(")
    );
    assert_eq!(program, original);
}

#[test]
fn predicate_constants_follow_polarity_but_words_require_an_explicit_truth_test() {
    let program = LiftedProgram {
        expressions: vec![
            expression(1, Kind::Predicate(true)),
            expression(2, Kind::Word(U256::MAX)),
        ],
        ..LiftedProgram::default()
    };
    let ir = Ir::default();
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer
            .render_condition(EntityId(1), false, |_| true)
            .unwrap(),
        "true"
    );
    assert_eq!(
        renderer
            .render_condition(EntityId(1), true, |_| true)
            .unwrap(),
        "false"
    );
    assert_eq!(
        renderer.render_condition(EntityId(2), false, |_| true),
        Err(LiftedRenderError::MissingWitness(EntityId(2)))
    );
}

#[test]
fn scope_mismatch_and_missing_predicate_witness_are_refused() {
    let mut program = LiftedProgram {
        expressions: vec![
            expression(1, Kind::Predicate(true)),
            expression(2, Kind::Not(EntityId(1))),
        ],
        ..LiftedProgram::default()
    };
    program.expressions[1].block = EntityId(999);
    let ir = Ir::default();
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(2), false, |_| true),
        Err(LiftedRenderError::MissingWitness(EntityId(1)))
    );
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(3), false, |_| true),
        Err(LiftedRenderError::MissingExpression(EntityId(3)))
    );
}

#[test]
fn caller_refusal_and_unbounded_negation_cycles_return_no_partial_text() {
    let mut program = LiftedProgram {
        expressions: vec![expression(1, Kind::Predicate(true))],
        ..LiftedProgram::default()
    };
    let ir = Ir::default();
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(1), false, |_| false),
        Err(LiftedRenderError::Limit)
    );
    program.expressions[0].kind = Kind::Not(EntityId(1));
    let mut charged = 0;
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(1), false, |amount| {
                charged += amount;
                true
            }),
        Err(LiftedRenderError::Limit)
    );
    assert_eq!(charged, MAX_LIFTED_EXPRESSION_TEXT);
}

#[test]
fn exponentially_shared_word_expansion_is_refused_before_publication() {
    let mut expressions = vec![expression(1, Kind::Value(EntityId(101)))];
    for id in 2..24 {
        expressions.push(expression(
            id,
            Kind::WordOp {
                opcode: 0x01,
                arguments: vec![EntityId(id - 1); 2],
            },
        ));
    }
    expressions.push(expression(24, Kind::NonZero(EntityId(23))));
    let program = LiftedProgram {
        expressions,
        ..LiftedProgram::default()
    };
    program.validate().unwrap();
    let ir = Ir::default();
    assert_eq!(
        program
            .render_index(&ir)
            .render_condition(EntityId(24), false, |_| true),
        Err(LiftedRenderError::Limit)
    );
}
