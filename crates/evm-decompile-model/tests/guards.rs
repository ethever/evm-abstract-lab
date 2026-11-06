//! Guard metadata admission is strict, bounded, and explicitly schema-three.
use evm_decompile_model::{
    Artifact, EntityId, LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind,
    LiftedFailure, LiftedGuard, LiftedGuardContinuation, LiftedGuardPayload, LiftedModelError,
    LiftedProgram, LiftedTransfer, LiftedType, MAX_LIFTED_RECORDS, MAX_STRUCTURED_WIRE_NODES,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn program() -> LiftedProgram {
    let block = |id, source| LiftedBlock {
        id: EntityId(id),
        block: EntityId(source),
        parameters: vec![],
        statements: vec![],
        condition: None,
        target: None,
        exit_values: vec![],
    };
    LiftedProgram {
        expressions: vec![LiftedExpression {
            id: EntityId(1),
            block: EntityId(100),
            ty: LiftedType::Predicate,
            kind: LiftedExpressionKind::Predicate(true),
        }],
        blocks: vec![block(2, 100), block(3, 101), block(4, 102)],
        transfers: vec![
            LiftedTransfer {
                id: EntityId(5),
                edge: EntityId(200),
                arguments: vec![],
            },
            LiftedTransfer {
                id: EntityId(6),
                edge: EntityId(201),
                arguments: vec![],
            },
        ],
        controls: vec![LiftedControl {
            id: EntityId(7),
            node: EntityId(300),
            condition: EntityId(1),
            guard: Some(LiftedGuard {
                block: EntityId(100),
                pass_nonzero: true,
                pass_edge: EntityId(200),
                failure_edge: EntityId(201),
                pass_node: EntityId(301),
                failure_node: EntityId(302),
                payload: LiftedGuardPayload::EmptyRevert,
                continuation: Some(LiftedGuardContinuation {
                    sequence: EntityId(303),
                    start: 1,
                    end: 2,
                    pass_falls_through: false,
                }),
            }),
        }],
        failures: vec![LiftedFailure {
            block: EntityId(102),
            rank: 0,
        }],
    }
}

fn strict<T: Serialize + DeserializeOwned>(value: &T) {
    let wire = serde_json::to_value(value).unwrap();
    let object = wire.as_object().unwrap();
    let _: T = serde_json::from_value(wire.clone()).unwrap();
    for key in object.keys() {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<T>(missing).is_err(),
            "missing {key}"
        );
    }
    let mut extra = wire.clone();
    extra["unexpected_guard_field"] = json!(false);
    assert!(serde_json::from_value::<T>(extra).is_err());
    assert!(serde_json::from_value::<T>(json!(object.values().collect::<Vec<_>>())).is_err());
    for (key, value) in object {
        let text = serde_json::to_string(&wire).unwrap();
        let duplicate = format!("{{{key:?}:{value},{}", &text[1..]);
        assert!(
            serde_json::from_str::<T>(&duplicate).is_err(),
            "duplicate {key}"
        );
    }
}

#[test]
fn all_new_guard_records_are_strict_named_objects_even_for_nullable_fields() {
    let program = program();
    program.validate().unwrap();
    let guard = program.controls[0].guard.as_ref().unwrap();
    strict(guard);
    strict(&program.failures[0]);
    strict(guard.continuation.as_ref().unwrap());
    let mut null = guard.clone();
    null.continuation = None;
    strict(&null);
    assert!(serde_json::from_str::<LiftedGuard>(r#"{"block":"100","pass_nonzero":true,"pass_edge":"200","failure_edge":"201","pass_node":"301","failure_node":"302","payload":"empty_revert","continuation":null,"continuation":null}"#).is_err());
    let mut wire = serde_json::to_value(&program).unwrap();
    wire["controls"][0]["guard"]["payload"] = json!("error_string");
    assert!(serde_json::from_value::<LiftedProgram>(wire).is_err());
    let restored: LiftedProgram =
        serde_json::from_slice(&serde_json::to_vec(&program).unwrap()).unwrap();
    assert_eq!(restored, program);
    assert!(restored.has_guards());
    assert_eq!(restored.record_count(), restored.entity_ids().count() + 3);
}

#[test]
fn absent_new_fields_preserve_the_exact_legacy_four_field_projection() {
    let mut program = program();
    program.failures.clear();
    program.controls[0].guard = None;
    let encoded = serde_json::to_string(&program).unwrap();
    assert!(!encoded.contains("\"failures\"") && !encoded.contains("\"guard\""));
    let decoded: LiftedProgram = serde_json::from_str(&encoded).unwrap();
    assert!(!decoded.has_guards());
    assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
    let mut explicit = serde_json::to_value(&program).unwrap();
    explicit["failures"] = json!([]);
    explicit["controls"][0]["guard"] = json!(null);
    assert_eq!(
        serde_json::from_value::<LiftedProgram>(explicit).unwrap(),
        program
    );
    let duplicate = encoded.replacen('{', "{\"failures\":[],\"failures\":[],", 1);
    assert!(serde_json::from_str::<LiftedProgram>(&duplicate).is_err());
    let mut unknown = serde_json::to_value(&program).unwrap();
    unknown["controls"][0]["guards"] = json!([]);
    assert!(serde_json::from_value::<LiftedProgram>(unknown).is_err());
    let legacy = include_str!("fixtures/partial-artifact.json").trim_end();
    let artifact: Artifact = serde_json::from_str(legacy).unwrap();
    assert!(artifact.admits_schema());
    assert_eq!(serde_json::to_string(&artifact).unwrap(), legacy);
}

#[test]
fn model_admission_rejects_malformed_guard_bounds_and_references() {
    let program = program();
    for field in [
        "unknown_failure",
        "duplicate_failure",
        "rank",
        "edge",
        "same_edge",
        "same_node",
        "scope",
        "start",
        "empty_range",
        "end",
    ] {
        let mut wrong = program.clone();
        let guard = wrong.controls[0].guard.as_mut().unwrap();
        match field {
            "unknown_failure" => wrong.failures[0].block = EntityId(999),
            "duplicate_failure" => wrong.failures.push(wrong.failures[0].clone()),
            "rank" => wrong.failures[0].rank = u32::MAX,
            "edge" => guard.pass_edge = EntityId(999),
            "same_edge" => guard.pass_edge = guard.failure_edge,
            "same_node" => guard.pass_node = guard.failure_node,
            "scope" => guard.continuation.as_mut().unwrap().sequence = EntityId(0),
            "start" => guard.continuation.as_mut().unwrap().start = 0,
            "empty_range" => guard.continuation.as_mut().unwrap().end = 1,
            "end" => guard.continuation.as_mut().unwrap().end = MAX_STRUCTURED_WIRE_NODES + 1,
            _ => unreachable!(),
        }
        assert!(wrong.validate().is_err(), "accepted {field}");
        assert!(
            serde_json::from_slice::<LiftedProgram>(&serde_json::to_vec(&wrong).unwrap()).is_err(),
            "wire accepted {field}"
        );
    }
}

#[test]
fn failure_records_and_nested_guard_continuations_count_toward_the_combined_ceiling() {
    let mut program = program();
    let old_count = program.entity_ids().count();
    program.failures = vec![program.failures[0].clone(); MAX_LIFTED_RECORDS - old_count - 1];
    // The guard and its continuation are two extra records beyond entity IDs.
    assert_eq!(program.record_count(), MAX_LIFTED_RECORDS + 1);
    assert_eq!(program.validate(), Err(LiftedModelError::RecordLimit));
}

#[test]
fn oversized_failure_wire_array_is_refused_at_the_declared_record_boundary() {
    let prefix = r#"{"expressions":[],"blocks":[],"transfers":[],"controls":[],"failures":["#;
    let fact = r#"{"block":"1","rank":0}"#;
    let mut wire =
        String::with_capacity(prefix.len() + (fact.len() + 1) * (MAX_LIFTED_RECORDS + 1) + 2);
    wire.push_str(prefix);
    for position in 0..=MAX_LIFTED_RECORDS {
        if position != 0 {
            wire.push(',');
        }
        wire.push_str(fact);
    }
    wire.push_str("]}");
    let error = serde_json::from_str::<LiftedProgram>(&wire)
        .unwrap_err()
        .to_string();
    assert!(error.contains("record limit"), "{error}");
}
