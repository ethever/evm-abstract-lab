//! Pin the existing enum wire shapes and reject malformed named payloads.
use evm_decompile_model::{
    Artifact, DispatchCase, DispatchTransition, EntityId, ExitKind, IndirectArm, StructuredKind,
    StructuredNode, StructuredProgram, Terminator, U256, ValueKind,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::fmt::Debug;

fn decode<T: DeserializeOwned>(value: &Value) -> serde_json::Result<T> {
    serde_json::from_slice(&serde_json::to_vec(value).unwrap())
}
fn roundtrip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, expected: Value) {
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
    assert_eq!(&decode::<T>(&expected).unwrap(), value);
}
fn named<T: Serialize + DeserializeOwned + PartialEq + Debug>(
    value: T,
    expected: Value,
    fields: &[&str],
) {
    roundtrip(&value, expected.clone());
    let (tag, payload) = expected.as_object().unwrap().iter().next().unwrap();
    let payload = payload.as_object().unwrap();
    assert_eq!(fields.len(), payload.len());
    let mut unknown = expected.clone();
    unknown[tag]["unexpected_field"] = json!(0);
    assert!(decode::<T>(&unknown).is_err(), "unknown field in {tag}");
    let mut positional = expected.clone();
    positional[tag] = Value::Array(fields.iter().map(|field| payload[*field].clone()).collect());
    assert!(decode::<T>(&positional).is_err(), "positional {tag}");
    for field in fields {
        let mut missing = expected.clone();
        missing[tag].as_object_mut().unwrap().remove(*field);
        assert!(decode::<T>(&missing).is_err(), "missing {tag}.{field}");
        let mut wrong = expected.clone();
        wrong[tag][*field] = json!({"invalid_type":true});
        assert!(decode::<T>(&wrong).is_err(), "wrong type in {tag}.{field}");
        let payload_text = serde_json::to_string(payload).unwrap();
        let duplicate_payload = format!("{{{field:?}:{},{}", payload[*field], &payload_text[1..]);
        let duplicate = format!("{{{tag:?}:{duplicate_payload}}}");
        assert!(
            serde_json::from_str::<T>(&duplicate).is_err(),
            "duplicate {tag}.{field}"
        );
    }
}

#[test]
fn value_kind_named_payloads_keep_shape_and_require_all_fields() {
    named(
        ValueKind::Operation {
            operation: EntityId(1),
            index: 2,
        },
        json!({"operation":{"operation":"1","index":2}}),
        &["operation", "index"],
    );
    named(
        ValueKind::Parameter {
            block: EntityId(1),
            index: 2,
        },
        json!({"parameter":{"block":"1","index":2}}),
        &["block", "index"],
    );
    for operation in [None, Some(EntityId(1))] {
        named(
            ValueKind::Memory { operation },
            json!({"memory":{"operation":operation}}),
            &["operation"],
        );
        named(
            ValueKind::Effect { operation },
            json!({"effect":{"operation":operation}}),
            &["operation"],
        );
    }
    named(
        ValueKind::MemoryParameter { block: EntityId(1) },
        json!({"memory_parameter":{"block":"1"}}),
        &["block"],
    );
    named(
        ValueKind::EffectParameter { block: EntityId(1) },
        json!({"effect_parameter":{"block":"1"}}),
        &["block"],
    );
}

#[test]
fn terminator_named_payloads_keep_shape_and_require_nullable_else_edge() {
    named(
        Terminator::Fallthrough { edge: EntityId(1) },
        json!({"fallthrough":{"edge":"1"}}),
        &["edge"],
    );
    named(
        Terminator::Jump {
            target: EntityId(1),
            edges: vec![EntityId(2)],
            unresolved: true,
            invalid_targets: vec![U256::from(9)],
        },
        json!({"jump":{"target":"1","edges":["2"],"unresolved":true,"invalid_targets":["0x9"]}}),
        &["target", "edges", "unresolved", "invalid_targets"],
    );
    for else_edge in [None, Some(EntityId(3))] {
        named(
            Terminator::Branch {
                condition: EntityId(1),
                target: EntityId(2),
                then_edges: vec![],
                else_edge,
                unresolved: true,
                invalid_targets: vec![],
            },
            json!({"branch":{"condition":"1","target":"2","then_edges":[],"else_edge":else_edge,"unresolved":true,"invalid_targets":[]}}),
            &[
                "condition",
                "target",
                "then_edges",
                "else_edge",
                "unresolved",
                "invalid_targets",
            ],
        );
    }
    named(
        Terminator::Exit {
            kind: ExitKind::Return,
            values: vec![EntityId(1)],
        },
        json!({"exit":{"kind":"return","values":["1"]}}),
        &["kind", "values"],
    );
    named(
        Terminator::Trap {
            reason: "halt".into(),
        },
        json!({"trap":{"reason":"halt"}}),
        &["reason"],
    );
}

#[test]
fn structured_named_payloads_keep_shape_and_require_nullable_loop_fields() {
    type Kind = StructuredKind<EntityId>;
    named(
        Kind::Block { block: EntityId(1) },
        json!({"block":{"block":"1"}}),
        &["block"],
    );
    named(
        Kind::Transfer { edge: EntityId(1) },
        json!({"transfer":{"edge":"1"}}),
        &["edge"],
    );
    named(
        Kind::If {
            condition: EntityId(1),
            then_branch: Box::new(EntityId(2)),
            else_branch: Box::new(EntityId(3)),
        },
        json!({"if":{"condition":"1","then_branch":"2","else_branch":"3"}}),
        &["condition", "then_branch", "else_branch"],
    );
    for (condition, exit_edge) in [(None, None), (Some(EntityId(4)), Some(EntityId(5)))] {
        named(
            Kind::While {
                header: Box::new(EntityId(2)),
                condition,
                condition_nonzero: true,
                body: Box::new(EntityId(3)),
                exit_edge,
            },
            json!({"while":{"header":"2","condition":condition,"condition_nonzero":true,"body":"3","exit_edge":exit_edge}}),
            &[
                "header",
                "condition",
                "condition_nonzero",
                "body",
                "exit_edge",
            ],
        );
    }
    named(
        Kind::Exit { block: EntityId(1) },
        json!({"exit":{"block":"1"}}),
        &["block"],
    );
    named(
        Kind::PartialExit { block: EntityId(1) },
        json!({"partial_exit":{"block":"1"}}),
        &["block"],
    );
    named(
        Kind::Indirect {
            block: EntityId(1),
            arms: vec![IndirectArm {
                target_pc: 5,
                body: EntityId(3),
            }],
            unresolved: true,
        },
        json!({"indirect":{"block":"1","arms":[{"target_pc":5,"body":"3"}],"unresolved":true}}),
        &["block", "arms", "unresolved"],
    );
    named(
        Kind::Dispatcher {
            entry: EntityId(1),
            cases: vec![DispatchCase {
                block: EntityId(1),
                transitions: vec![DispatchTransition {
                    id: EntityId(4),
                    edge: EntityId(5),
                    target: EntityId(6),
                }],
            }],
        },
        json!({"dispatcher":{"entry":"1","cases":[{"block":"1","transitions":[{"id":"4","edge":"5","target":"6"}]}]}}),
        &["entry", "cases"],
    );
}

#[test]
fn unit_newtype_and_list_variants_retain_their_existing_representation() {
    roundtrip(
        &ValueKind::Constant(U256::from(42)),
        json!({"constant":"0x2a"}),
    );
    roundtrip(&Terminator::Pending, json!("pending"));
    roundtrip(&StructuredKind::<EntityId>::Break, json!("break"));
    roundtrip(&StructuredKind::<EntityId>::Continue, json!("continue"));
    roundtrip(
        &StructuredKind::Sequence(vec![EntityId(1), EntityId(2)]),
        json!({"sequence":["1","2"]}),
    );
    // Existing standard unit-variant object forms remain accepted too.
    assert_eq!(
        decode::<Terminator>(&json!({"pending":null})).unwrap(),
        Terminator::Pending
    );
    assert_eq!(
        decode::<StructuredKind<EntityId>>(&json!({"break":null})).unwrap(),
        StructuredKind::Break
    );
}

#[test]
fn owned_rust_ast_variants_keep_their_recursive_serialized_shape() {
    named(
        StructuredKind::If {
            condition: EntityId(1),
            then_branch: Box::new(StructuredNode {
                id: EntityId(2),
                kind: StructuredKind::Block { block: EntityId(8) },
            }),
            else_branch: Box::new(StructuredNode {
                id: EntityId(3),
                kind: StructuredKind::Sequence(Vec::new()),
            }),
        },
        json!({"if":{"condition":"1","then_branch":{"id":"2","kind":{"block":{"block":"8"}}},"else_branch":{"id":"3","kind":{"sequence":[]}}}}),
        &["condition", "then_branch", "else_branch"],
    );
}

#[test]
fn populated_prechange_artifact_serialization_stays_byte_identical() {
    // Produced by the reviewed compiled core for an actual memory/call-input
    // dependent partial jump, before any enum decoder was changed.
    let original = include_str!("fixtures/partial-artifact.json").trim_end();
    let artifact: Artifact = serde_json::from_str(original).unwrap();
    assert_eq!(serde_json::to_string(&artifact).unwrap(), original);
    let value: Value = serde_json::from_str(original).unwrap();
    let mut pointers = Vec::new();
    for (stage_index, stage) in value["stages"].as_array().unwrap().iter().enumerate() {
        for (index, definition) in stage["ir"]["values"].as_array().unwrap().iter().enumerate() {
            for tag in ["memory", "effect"] {
                if definition["kind"].get(tag).is_some() {
                    pointers.push((
                        format!("/stages/{stage_index}/ir/values/{index}/kind/{tag}"),
                        "operation",
                    ));
                }
            }
        }
        for (index, block) in stage["ir"]["blocks"].as_array().unwrap().iter().enumerate() {
            if block["terminator"].get("branch").is_some() {
                pointers.push((
                    format!("/stages/{stage_index}/ir/blocks/{index}/terminator/branch"),
                    "else_edge",
                ));
            }
        }
    }
    assert!(pointers.iter().any(|(p, _)| p.ends_with("/memory")));
    assert!(pointers.iter().any(|(p, _)| p.ends_with("/effect")));
    assert!(pointers.iter().any(|(p, _)| p.ends_with("/branch")));
    for (pointer, nullable) in pointers {
        let mut missing = value.clone();
        missing
            .pointer_mut(&pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(nullable);
        assert!(
            decode::<Artifact>(&missing).is_err(),
            "missing {pointer}/{nullable}"
        );
        let mut unknown = value.clone();
        unknown.pointer_mut(&pointer).unwrap()["unexpected_field"] = json!(true);
        assert!(
            decode::<Artifact>(&unknown).is_err(),
            "unknown field in {pointer}"
        );
    }
}

#[test]
fn flat_wire_named_variants_are_strict_and_still_check_graph_references() {
    let valid = json!({"root":"1","nodes":[
        {"id":"1","kind":{"while":{"header":"2","condition":null,"condition_nonzero":true,"body":"3","exit_edge":null}}},
        {"id":"2","kind":{"sequence":[]}}, {"id":"3","kind":"continue"}
    ]});
    let program: StructuredProgram = decode(&valid).unwrap();
    assert_eq!(serde_json::to_value(program).unwrap(), valid);
    let pointer = "/nodes/0/kind/while";
    for field in ["condition", "exit_edge"] {
        let mut missing = valid.clone();
        missing
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode::<StructuredProgram>(&missing).is_err());
    }
    let mut array = valid.clone();
    *array.pointer_mut(pointer).unwrap() = json!(["2", null, true, "3", null]);
    assert!(decode::<StructuredProgram>(&array).is_err());
    let mut shared = valid.clone();
    shared.pointer_mut(pointer).unwrap()["body"] = json!("2");
    assert!(decode::<StructuredProgram>(&shared).is_err());
    let mut cycle = valid.clone();
    cycle.pointer_mut(pointer).unwrap()["header"] = json!("1");
    assert!(decode::<StructuredProgram>(&cycle).is_err());
    assert!(
        decode::<StructuredProgram>(
            &json!({"root":"1","nodes":[{"id":"1","kind":{"block":["1"]}}]})
        )
        .is_err()
    );
}
