//! Strict schema-four records are admitted independently of producer discovery.
use evm_decompile_model::{
    Artifact, EntityId, LiftedBlock, LiftedConstruction, LiftedErrorPayload, LiftedGuardPayload,
    LiftedModelError, LiftedProgram, LiftedRevert, LiftedStatement, MAX_ERROR_PAYLOAD_BYTES,
    MAX_LIFTED_RECORDS, SCHEMA_VERSION, StageKind, U256,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn payload(message: &str) -> LiftedErrorPayload {
    let mut bytes = vec![0; 68 + message.len().div_ceil(32) * 32];
    bytes[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    bytes[35] = 32;
    bytes[36..68].copy_from_slice(&U256::from(message.len()).to_be_bytes::<32>());
    bytes[68..68 + message.len()].copy_from_slice(message.as_bytes());
    LiftedErrorPayload {
        id: EntityId(1000),
        message: message.into(),
        bytes,
    }
}

fn program() -> LiftedProgram {
    LiftedProgram {
        blocks: vec![LiftedBlock {
            id: EntityId(1),
            block: EntityId(100),
            parameters: vec![],
            statements: vec![LiftedStatement {
                id: EntityId(2),
                operation: EntityId(200),
                inputs: vec![],
                outputs: vec![],
            }],
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        payloads: vec![payload("ds-math-mul-overflow")],
        reverts: vec![LiftedRevert {
            id: EntityId(1001),
            block: EntityId(100),
            operation: EntityId(200),
            memory: EntityId(300),
            offset: EntityId(400),
            size: EntityId(401),
            payload: EntityId(1000),
            writes: vec![EntityId(210)],
        }],
        constructions: vec![LiftedConstruction {
            id: EntityId(1002),
            block: EntityId(100),
            start: 0,
            end: 1,
            revert: EntityId(1001),
        }],
        ..LiftedProgram::default()
    }
}

fn strict<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(record: &T) {
    let wire = serde_json::to_value(record).unwrap();
    let object = wire.as_object().unwrap();
    assert_eq!(serde_json::from_value::<T>(wire.clone()).unwrap(), *record);
    for (key, value) in object {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<T>(missing).is_err(),
            "missing {key}"
        );
        let mut null = wire.clone();
        null[key] = json!(null);
        assert!(serde_json::from_value::<T>(null).is_err(), "null {key}");
        let text = serde_json::to_string(&wire).unwrap();
        let duplicate = format!("{{{key:?}:{value},{}", &text[1..]);
        assert!(
            serde_json::from_str::<T>(&duplicate).is_err(),
            "duplicate {key}"
        );
    }
    let mut extra = wire.clone();
    extra["unknown_payload_field"] = json!(false);
    assert!(serde_json::from_value::<T>(extra).is_err());
    assert!(serde_json::from_value::<T>(json!(object.values().collect::<Vec<_>>())).is_err());
}

#[test]
fn every_new_semantic_record_is_a_strict_nonnullable_named_object() {
    let program = program();
    program.validate().unwrap();
    strict(&program.payloads[0]);
    strict(&program.reverts[0]);
    strict(&program.constructions[0]);
    assert_eq!(
        serde_json::from_slice::<LiftedProgram>(&serde_json::to_vec(&program).unwrap()).unwrap(),
        program
    );
    assert!(program.has_payloads());
    let tag = LiftedGuardPayload::ErrorString(program.payloads[0].id);
    assert_eq!(
        serde_json::to_value(tag).unwrap(),
        json!({"error_string":"1000"})
    );
    assert_eq!(
        serde_json::from_value::<LiftedGuardPayload>(json!({"error_string":"1000"})).unwrap(),
        tag
    );
    for malformed in [
        json!({"error_string":null}),
        json!({"error_string":1000}),
        json!({"error_string":"1000","empty_revert":null}),
        json!(["error_string", "1000"]),
    ] {
        assert!(serde_json::from_value::<LiftedGuardPayload>(malformed).is_err());
    }
}

#[test]
fn canonical_content_admission_checks_every_header_message_and_padding_byte() {
    for message in [
        "",
        "ds-math-mul-overflow",
        "TransferHelper: TRANSFER_FAILED",
        "quote\" slash\\\n\0 中文",
    ] {
        let value = payload(message);
        assert!(value.is_canonical());
        assert_eq!(
            serde_json::from_slice::<LiftedErrorPayload>(&serde_json::to_vec(&value).unwrap())
                .unwrap(),
            value
        );
        for position in 0..value.bytes.len() {
            let mut corrupt = value.clone();
            corrupt.bytes[position] ^= 1;
            assert!(
                !corrupt.is_canonical(),
                "accepted byte mutation at {position}"
            );
            assert!(
                serde_json::from_slice::<LiftedErrorPayload>(
                    &serde_json::to_vec(&corrupt).unwrap()
                )
                .is_err()
            );
        }
        let mut wrong_message = value;
        wrong_message.message.push('x');
        assert!(!wrong_message.is_canonical());
    }
}

#[test]
fn payloads_and_capsules_cannot_claim_older_semantic_versions() {
    assert_eq!(
        SCHEMA_VERSION, 4,
        "nonempty payload and lazy fault semantics change the wire contract"
    );
    let mut artifact: Artifact =
        serde_json::from_str(include_str!("fixtures/partial-artifact.json")).unwrap();
    artifact.schema_version = 4;
    let stage = artifact.stages.last_mut().unwrap();
    stage.kind = StageKind::Lifted;
    stage.ir.lifted = Some(program());
    assert!(artifact.admits_schema());
    for version in [1, 2, 3, 5] {
        let mut wrong = artifact.clone();
        wrong.schema_version = version;
        assert!(!wrong.admits_schema());
        let wire = serde_json::to_vec(&wrong).unwrap();
        let decoded: Artifact = serde_json::from_slice(&wire).unwrap();
        assert!(!decoded.admits_schema(), "decoded forged version {version}");
    }
    // New empty fields are omitted, preserving exact v2 / v3 old records.
    let legacy = r#"{"expressions":[],"blocks":[],"transfers":[],"controls":[]}"#;
    let empty: LiftedProgram = serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_string(&empty).unwrap(), legacy);
    for field in ["payloads", "reverts", "constructions"] {
        let mut wire: serde_json::Value = serde_json::from_str(legacy).unwrap();
        wire[field] = json!(null);
        assert!(
            serde_json::from_value::<LiftedProgram>(wire).is_err(),
            "null {field}"
        );
    }
}

#[test]
fn recursive_dangling_duplicate_and_out_of_region_certificates_are_rejected() {
    for field in [
        "self_payload",
        "self_revert",
        "duplicate",
        "unknown_block",
        "reverse_interval",
        "end",
        "duplicate_block",
    ] {
        let mut program = program();
        match field {
            "self_payload" => program.reverts[0].payload = program.reverts[0].id,
            "self_revert" => program.constructions[0].revert = program.constructions[0].id,
            "duplicate" => program.constructions[0].id = program.payloads[0].id,
            "unknown_block" => program.constructions[0].block = EntityId(999),
            "reverse_interval" => program.constructions[0].start = 2,
            "end" => program.constructions[0].end = 2,
            "duplicate_block" => {
                let mut other = program.constructions[0].clone();
                other.id = EntityId(1003);
                program.constructions.push(other);
            }
            _ => unreachable!(),
        }
        assert!(program.validate().is_err(), "accepted {field}");
        assert!(
            serde_json::from_slice::<LiftedProgram>(&serde_json::to_vec(&program).unwrap())
                .is_err()
        );
    }
}

#[test]
fn individual_and_combined_transport_budgets_charge_proof_bytes_and_links() {
    let largest = payload(&"x".repeat(MAX_ERROR_PAYLOAD_BYTES - 68));
    assert_eq!(largest.bytes.len(), MAX_ERROR_PAYLOAD_BYTES);
    assert!(largest.is_canonical());
    strict(&largest);
    let mut too_long = largest.clone();
    too_long.message.push('x');
    too_long.bytes.push(0);
    assert!(
        serde_json::from_slice::<LiftedErrorPayload>(&serde_json::to_vec(&too_long).unwrap())
            .is_err()
    );
    let mut too_many_writes = program().reverts.remove(0);
    too_many_writes
        .writes
        .resize(MAX_ERROR_PAYLOAD_BYTES + 1, EntityId(210));
    assert!(
        serde_json::from_slice::<LiftedRevert>(&serde_json::to_vec(&too_many_writes).unwrap())
            .is_err()
    );
    let mut combined = program();
    let before = combined.record_count();
    combined.payloads = vec![largest; MAX_LIFTED_RECORDS / (MAX_ERROR_PAYLOAD_BYTES * 2) + 2];
    assert!(combined.record_count() > MAX_LIFTED_RECORDS);
    assert_eq!(combined.validate(), Err(LiftedModelError::RecordLimit));
    assert!(
        before > combined.entity_ids().take(2).count(),
        "metadata is charged beyond identity count"
    );
}
