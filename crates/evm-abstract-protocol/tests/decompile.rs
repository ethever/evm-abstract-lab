//! The independent request retains strict configuration and lossless identities.

use evm_abstract_protocol::{DecompileRequest, DecompileStrategy, decompile_model::EntityId};

#[test]
fn decompiler_request_roundtrips_without_execution_environment_fields() {
    let request = DecompileRequest::default();
    let json = serde_json::to_value(&request).unwrap();
    assert!(json.get("environment").is_none());
    assert_eq!(request.strategy, DecompileStrategy::Dispatcher);
    assert_eq!(
        serde_json::from_value::<DecompileRequest>(json).unwrap(),
        request
    );
}

#[test]
fn unknown_or_missing_configuration_fields_are_rejected() {
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["environment"] = serde_json::json!({});
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["limits"]["max_cloned_bloks"] = serde_json::json!(0);
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
    let mut request = serde_json::to_value(DecompileRequest::default()).unwrap();
    request["limits"]
        .as_object_mut()
        .unwrap()
        .remove("max_work");
    assert!(serde_json::from_value::<DecompileRequest>(request).is_err());
}

#[test]
fn entity_identity_remains_exact_above_javascript_safe_integer_range() {
    let identity = EntityId(u64::MAX);
    let json = serde_json::to_string(&identity).unwrap();
    assert_eq!(json, "\"18446744073709551615\"");
    assert_eq!(serde_json::from_str::<EntityId>(&json).unwrap(), identity);
    assert!(serde_json::from_str::<EntityId>("18446744073709551615").is_err());
}

#[test]
fn limits_are_named_objects_with_no_duplicate_fields() {
    let request = serde_json::to_value(DecompileRequest::default()).unwrap();
    let mut positional = request.clone();
    positional["limits"] = serde_json::json!([
        100000, 8192, 100000, 500000, 100000000, 100000, 8, 2048, 128, 2000000
    ]);
    assert!(serde_json::from_value::<DecompileRequest>(positional).is_err());
    let mut wrong_type = request.clone();
    wrong_type["limits"]["max_work"] = serde_json::json!("100");
    assert!(serde_json::from_value::<DecompileRequest>(wrong_type).is_err());
    let raw = serde_json::to_string(&request).unwrap();
    let duplicate = raw.replace("\"max_work\":", "\"max_work\":1,\"max_work\":");
    assert!(serde_json::from_str::<DecompileRequest>(&duplicate).is_err());
}

#[test]
fn lifted_stage_with_deep_flat_expression_dag_roundtrips_in_the_result_envelope() {
    use evm_abstract_protocol::{
        DecimalU64, DecompileArtifact, DecompileJobResult, DecompileJobResultReply, JobId,
        PublicationVersion,
        decompile_model::{
            LiftedBlock, LiftedExpression, LiftedExpressionKind, LiftedProgram, LiftedType,
            SCHEMA_VERSION, StageId, StageKind,
        },
    };

    let mut artifact: DecompileArtifact = serde_json::from_str(include_str!(
        "../../evm-decompile-model/tests/fixtures/partial-artifact.json"
    ))
    .unwrap();
    artifact.schema_version = SCHEMA_VERSION;
    let mut stage = artifact.current().unwrap().clone();
    stage.parent = Some(stage.id);
    stage.id = StageId(stage.id.0 + 1);
    stage.kind = StageKind::Lifted;
    let block = stage.ir.blocks[0].id;
    // Flat references keep JSON nesting bounded even for a long expression DAG.
    let expressions: Vec<_> = (0..320_u64)
        .map(|offset| {
            let id = EntityId(u64::MAX - 320 + offset);
            LiftedExpression {
                id,
                block,
                ty: LiftedType::Predicate,
                kind: if offset == 0 {
                    LiftedExpressionKind::Predicate(false)
                } else {
                    LiftedExpressionKind::Not(EntityId(id.0 - 1))
                },
            }
        })
        .collect();
    stage.ir.lifted = Some(LiftedProgram {
        expressions,
        blocks: vec![LiftedBlock {
            id: EntityId(u64::MAX),
            block,
            parameters: vec![],
            statements: vec![],
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        transfers: vec![],
        controls: vec![],
    });
    stage.ir.lifted.as_ref().unwrap().validate().unwrap();
    artifact.current_stage = Some(stage.id);
    artifact.stages.push(stage);
    let reply = DecompileJobResultReply {
        result: Ok(DecompileJobResult {
            id: JobId(u64::MAX),
            publication: PublicationVersion(DecimalU64(u64::MAX)),
            artifact,
        }),
    };
    let wire = serde_json::to_vec(&reply).unwrap();
    let decoded: DecompileJobResultReply = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded, reply);
    assert_eq!(
        decoded.result.unwrap().artifact.current().unwrap().kind,
        StageKind::Lifted
    );
}
