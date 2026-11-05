//! Flat semantic projection admission, typed evaluation and legacy wire behavior.
use evm_decompile_model::{
    EntityId, Ir, LiftedBlock, LiftedExpression, LiftedExpressionKind as Kind, LiftedProgram,
    LiftedRelation, LiftedType, LiftedValue, SCHEMA_VERSION, U256,
};
use serde_json::json;

fn expression(id: u64, kind: Kind) -> LiftedExpression {
    LiftedExpression {
        id: EntityId(id),
        block: EntityId(100),
        ty: kind.ty(),
        kind,
    }
}
fn program() -> LiftedProgram {
    LiftedProgram {
        expressions: vec![
            expression(1, Kind::Value(EntityId(101))),
            expression(2, Kind::Word(U256::from(3))),
            expression(
                3,
                Kind::Compare {
                    relation: LiftedRelation::Uge,
                    left: EntityId(1),
                    right: EntityId(2),
                },
            ),
            expression(4, Kind::WordFromPredicate(EntityId(3))),
        ],
        blocks: vec![LiftedBlock {
            id: EntityId(5),
            block: EntityId(100),
            parameters: vec![EntityId(101)],
            statements: Vec::new(),
            condition: Some(EntityId(3)),
            target: None,
            exit_values: vec![EntityId(4)],
        }],
        transfers: Vec::new(),
        controls: Vec::new(),
    }
}
#[test]
fn typed_predicates_keep_numeric_cast_and_unsigned_boundary() {
    let p = program();
    p.validate().unwrap();
    for value in [
        U256::ZERO,
        U256::from(2),
        U256::from(3),
        U256::from(1) << 255,
        U256::MAX,
    ] {
        let expected = value >= U256::from(3);
        assert_eq!(
            p.evaluate_expression(EntityId(3), |_| Some(value)),
            Some(LiftedValue::Predicate(expected))
        );
        assert_eq!(
            p.evaluate_expression(EntityId(4), |_| Some(value)),
            Some(LiftedValue::Word(U256::from(u8::from(expected))))
        );
    }
    assert_eq!(p.render_expression(EntityId(3)), "(v101 >= 0x3)");
    assert_eq!(p.render_expression(EntityId(4)), "word((v101 >= 0x3))");
    assert!(p.evaluate_expression(EntityId(4), |_| None).is_none());
}
#[test]
fn flat_codec_rejects_cycles_scope_and_sort_corruption() {
    let p = program();
    let wire = serde_json::to_value(&p).unwrap();
    assert_eq!(
        serde_json::from_value::<LiftedProgram>(wire.clone()).unwrap(),
        p
    );
    let mut malformed = wire.clone();
    malformed["expressions"][0]["kind"] = json!({"non_zero":"4"});
    malformed["expressions"][0]["ty"] = json!("predicate");
    assert!(serde_json::from_value::<LiftedProgram>(malformed).is_err());
    let mut malformed = wire.clone();
    malformed["expressions"][1]["block"] = json!("999");
    assert!(serde_json::from_value::<LiftedProgram>(malformed).is_err());
    let mut malformed = wire.clone();
    malformed["expressions"][3]["kind"] = json!({"word_from_predicate":"1"});
    assert!(serde_json::from_value::<LiftedProgram>(malformed).is_err());
    let mut malformed = wire.clone();
    malformed["blocks"][0]["condition"] = json!("4");
    assert!(serde_json::from_value::<LiftedProgram>(malformed).is_err());
    let mut malformed = wire;
    malformed["expressions"][3]["id"] = json!("3");
    assert!(serde_json::from_value::<LiftedProgram>(malformed).is_err());
}
#[test]
fn semantic_named_payloads_and_records_are_strict_objects() {
    for malformed in [
        json!({"compare":["uge","1","2"]}),
        json!({"compare":{"relation":"uge","left":"1","right":"2","typo":false}}),
        json!({"compare":{"relation":"uge","left":"1"}}),
        json!({"word_op":[1,["1","2"]]}),
    ] {
        assert!(serde_json::from_value::<Kind>(malformed).is_err());
    }
    assert!(
        serde_json::from_str::<Kind>(
            r#"{"compare":{"relation":"uge","left":"1","left":"2","right":"2"}}"#
        )
        .is_err()
    );
    let text = serde_json::to_string(&program()).unwrap();
    assert!(
        serde_json::from_str::<LiftedProgram>(&format!("{{\"controls\":[],{}", &text[1..]))
            .is_err()
    );
    let mut wire = serde_json::to_value(program()).unwrap();
    wire["unknown"] = json!(null);
    assert!(serde_json::from_value::<LiftedProgram>(wire).is_err());
    assert!(serde_json::from_value::<LiftedProgram>(json!([[], [], [], []])).is_err());
}
#[test]
fn legacy_missing_lifted_defaults_only_that_field_and_keeps_serialization() {
    assert_eq!(SCHEMA_VERSION, 2);
    let ir = Ir::default();
    let mut wire = serde_json::to_value(&ir).unwrap();
    assert!(wire.get("lifted").is_none());
    assert_eq!(serde_json::from_value::<Ir>(wire.clone()).unwrap(), ir);
    wire["lifted"] = json!(null);
    assert_eq!(serde_json::from_value::<Ir>(wire.clone()).unwrap(), ir);
    wire.as_object_mut().unwrap().remove("structured");
    assert!(serde_json::from_value::<Ir>(wire).is_err());
    let legacy = include_str!("fixtures/partial-artifact.json").trim_end();
    let artifact: evm_decompile_model::Artifact = serde_json::from_str(legacy).unwrap();
    assert_eq!(artifact.schema_version, 1);
    assert_eq!(serde_json::to_string(&artifact).unwrap(), legacy);
}
#[test]
fn deep_shared_dag_decodes_evaluates_and_renders_without_recursive_stacks() {
    let mut p = LiftedProgram::default();
    p.expressions.push(expression(1, Kind::Word(U256::from(1))));
    for id in 2..4096 {
        p.expressions.push(expression(
            id,
            Kind::WordOp {
                opcode: 0x01,
                arguments: vec![EntityId(id - 1), EntityId(id - 1)],
            },
        ));
    }
    let encoded = serde_json::to_vec(&p).unwrap();
    let decoded: LiftedProgram = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, p);
    assert_eq!(
        decoded.evaluate_expression(EntityId(4095), |_| None),
        Some(LiftedValue::Word(U256::ZERO))
    );
    let text = decoded.render_expression(EntityId(4095));
    assert!(text.len() < 128 && text.starts_with("expression_limit("));
    assert_eq!(decoded.expressions.last().unwrap().ty, LiftedType::Word256);
}
