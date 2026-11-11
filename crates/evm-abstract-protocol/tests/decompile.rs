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
        failures: vec![],
        ..LiftedProgram::default()
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

fn guard_reply() -> evm_abstract_protocol::DecompileJobResultReply {
    use evm_abstract_protocol::{
        DecimalU64, DecompileArtifact, DecompileJobResult, DecompileJobResultReply, JobId,
        PublicationVersion,
        decompile_model::{
            LiftedBlock, LiftedControl, LiftedExpression, LiftedExpressionKind, LiftedFailure,
            LiftedGuard, LiftedGuardContinuation, LiftedGuardPayload, LiftedProgram,
            LiftedTransfer, LiftedType, SCHEMA_VERSION, StageKind,
        },
    };
    let mut artifact: DecompileArtifact = serde_json::from_str(include_str!(
        "../../evm-decompile-model/tests/fixtures/partial-artifact.json"
    ))
    .unwrap();
    artifact.schema_version = SCHEMA_VERSION;
    let stage = artifact.stages.last_mut().unwrap();
    stage.kind = StageKind::Lifted;
    let block = stage.ir.blocks[0].id;
    stage.ir.lifted = Some(LiftedProgram {
        expressions: vec![LiftedExpression {
            id: EntityId(u64::MAX - 4),
            block,
            ty: LiftedType::Predicate,
            kind: LiftedExpressionKind::Predicate(true),
        }],
        blocks: vec![LiftedBlock {
            id: EntityId(u64::MAX - 3),
            block,
            parameters: vec![],
            statements: vec![],
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        transfers: vec![
            LiftedTransfer {
                id: EntityId(u64::MAX - 2),
                edge: EntityId(1000),
                arguments: vec![],
            },
            LiftedTransfer {
                id: EntityId(u64::MAX - 1),
                edge: EntityId(1001),
                arguments: vec![],
            },
        ],
        controls: vec![LiftedControl {
            id: EntityId(u64::MAX),
            node: EntityId(1002),
            condition: EntityId(u64::MAX - 4),
            guard: Some(LiftedGuard {
                block,
                pass_nonzero: true,
                pass_edge: EntityId(1000),
                failure_edge: EntityId(1001),
                pass_node: EntityId(1003),
                failure_node: EntityId(1004),
                payload: LiftedGuardPayload::EmptyRevert,
                continuation: Some(LiftedGuardContinuation {
                    sequence: EntityId(1005),
                    start: 1,
                    end: 2,
                    pass_falls_through: false,
                }),
            }),
        }],
        failures: vec![LiftedFailure {
            block,
            rank: 0,
            payload: LiftedGuardPayload::EmptyRevert,
        }],
        ..LiftedProgram::default()
    });
    stage.ir.lifted.as_ref().unwrap().validate().unwrap();
    DecompileJobResultReply {
        result: Ok(DecompileJobResult {
            id: JobId(u64::MAX),
            publication: PublicationVersion(DecimalU64(u64::MAX)),
            artifact,
        }),
    }
}

#[test]
fn schema_three_guard_plan_roundtrips_losslessly_in_the_protocol_envelope() {
    use evm_abstract_protocol::DecompileJobResultReply;
    let mut reply = guard_reply();
    // Empty guards retain the legacy v3 representation, with no new fields.
    reply.result.as_mut().unwrap().artifact.schema_version = 3;
    let wire = serde_json::to_vec(&reply).unwrap();
    let restored: DecompileJobResultReply = serde_json::from_slice(&wire).unwrap();
    assert_eq!(restored, reply);
    let artifact = &restored.result.as_ref().unwrap().artifact;
    assert_eq!(artifact.schema_version, 3);
    assert!(artifact.admits_schema());
    for version in [1, 2] {
        let mut legacy = artifact.clone();
        legacy.schema_version = version;
        assert!(!legacy.admits_schema());
    }
}

fn error_reply() -> evm_abstract_protocol::DecompileJobResultReply {
    use evm_abstract_protocol::decompile_model::{
        LiftedConstruction, LiftedErrorPayload, LiftedGuardPayload, LiftedRevert, LiftedStatement,
    };
    let mut reply = guard_reply();
    let program = reply
        .result
        .as_mut()
        .unwrap()
        .artifact
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    let message = "ds-math-mul-overflow";
    let mut bytes = vec![0_u8; 100];
    bytes[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    bytes[35] = 32;
    bytes[67] = 20;
    bytes[68..88].copy_from_slice(message.as_bytes());
    let payload = EntityId(900_001);
    let proof = EntityId(900_002);
    let block = program.blocks[0].block;
    program.payloads.push(LiftedErrorPayload {
        id: payload,
        message: message.into(),
        bytes,
    });
    program.blocks[0].statements.push(LiftedStatement {
        id: EntityId(900_004),
        operation: EntityId(900_005),
        inputs: vec![],
        outputs: vec![],
    });
    program.reverts.push(LiftedRevert {
        execution_domain: Default::default(),
        execution_fork: None,
        code_hash: None,
        id: proof,
        block,
        operation: EntityId(900_005),
        memory: EntityId(900_006),
        offset: EntityId(900_007),
        size: EntityId(900_008),
        payload,
        writes: vec![EntityId(900_009)],
    });
    program.constructions.push(LiftedConstruction {
        id: EntityId(900_003),
        block,
        start: 0,
        end: 1,
        revert: proof,
    });
    program.failures[0].payload = LiftedGuardPayload::ErrorString(payload);
    program.controls[0].guard.as_mut().unwrap().payload = LiftedGuardPayload::ErrorString(payload);
    program.validate().unwrap();
    reply
}

#[test]
fn exact_error_bytes_and_lazy_construction_links_survive_the_result_envelope() {
    use evm_abstract_protocol::DecompileJobResultReply;
    let reply = error_reply();
    let encoded = serde_json::to_vec(&reply).unwrap();
    let restored: DecompileJobResultReply = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, reply);
    let artifact = &restored.result.as_ref().unwrap().artifact;
    assert_eq!(
        artifact.schema_version,
        evm_abstract_protocol::decompile_model::SCHEMA_VERSION
    );
    assert!(artifact.admits_schema());
    for old in [1, 2, 3] {
        let mut wrong = artifact.clone();
        wrong.schema_version = old;
        assert!(
            !wrong.admits_schema(),
            "nonempty payload advertised as v{old}"
        );
    }
    let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert_eq!(program.payloads[0].bytes.len(), 100);
    assert_eq!(&program.payloads[0].bytes[88..], &[0_u8; 12]);
    assert_eq!(program.constructions[0].revert, program.reverts[0].id);
}

#[test]
fn schema_six_procedure_invocation_entities_and_sparse_maps_survive_result_envelope() {
    use evm_abstract_protocol::decompile_model::{
        SharedBinding, SharedInterface, SharedInvocation, SharedProcedure,
    };
    let mut reply = guard_reply();
    let artifact = &mut reply.result.as_mut().unwrap().artifact;
    let stage = artifact.stages.last_mut().unwrap();
    let high = stage.ir.lifted.as_mut().unwrap();
    let procedure = EntityId(u64::MAX - 20);
    high.procedures.push(SharedProcedure {
        kind: evm_abstract_protocol::decompile_model::SharedProcedureKind::Normal,
        id: procedure,
        body: EntityId(1002),
        interface: SharedInterface {
            inputs: vec![EntityId(2000), EntityId(2001)],
            outputs: vec![EntityId(2002), EntityId(2003)],
            memory_inputs: vec![EntityId(2004)],
            memory_outputs: vec![EntityId(2005)],
            effect_inputs: vec![EntityId(2006)],
            effect_outputs: vec![EntityId(2007)],
            continuations: vec![],
        },
    });
    high.invocations.push(SharedInvocation {
        id: EntityId(u64::MAX - 19),
        procedure,
        source: EntityId(1003),
        owner: Some(EntityId(1002)),
        position: 1,
        continuation: Some(EntityId(1004)),
        bindings: vec![SharedBinding {
            formal: EntityId(2000),
            actual: EntityId(u64::MAX - 18),
        }],
    });
    assert!(artifact.admits_schema());
    let wire = serde_json::to_vec(&reply).unwrap();
    let restored: evm_abstract_protocol::DecompileJobResultReply =
        serde_json::from_slice(&wire).unwrap();
    assert_eq!(restored, reply);
    let artifact = &restored.result.as_ref().unwrap().artifact;
    let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert_eq!(high.procedures.len(), 1);
    assert_eq!(high.invocations.len(), 1);
    assert_eq!(
        high.invocations[0].actual(EntityId(2000)),
        Some(EntityId(u64::MAX - 18))
    );
    assert_eq!(
        high.invocations[0].actual(EntityId(2001)),
        Some(EntityId(2001))
    );
    for version in [1, 2, 3, 4, 5] {
        let mut old = artifact.clone();
        old.schema_version = version;
        assert!(
            !old.admits_schema(),
            "new executable records advertised as v{version}"
        );
    }
}

#[test]
fn nested_payload_proofs_are_strict_even_inside_the_protocol_result() {
    use evm_abstract_protocol::DecompileJobResultReply;
    let reply = error_reply();
    let wire = serde_json::to_value(&reply).unwrap();
    let position = wire["result"]["Ok"]["artifact"]["stages"]
        .as_array()
        .unwrap()
        .len()
        - 1;
    for arena in ["payloads", "reverts", "constructions"] {
        let path = format!("/result/Ok/artifact/stages/{position}/ir/lifted/{arena}/0");
        let object = wire.pointer(&path).unwrap().as_object().unwrap();
        for (field, value) in object {
            let mut missing = wire.clone();
            missing
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<DecompileJobResultReply>(missing).is_err(),
                "missing {arena}.{field}"
            );
            let mut null = wire.clone();
            null.pointer_mut(&path).unwrap()[field] = serde_json::json!(null);
            assert!(
                serde_json::from_value::<DecompileJobResultReply>(null).is_err(),
                "null {arena}.{field}"
            );
            let text = serde_json::to_string(&wire).unwrap();
            let record = serde_json::to_string(object).unwrap();
            let duplicate = format!("{{{field:?}:{value},{}", &record[1..]);
            assert!(text.contains(&record));
            assert!(
                serde_json::from_str::<DecompileJobResultReply>(
                    &text.replacen(&record, &duplicate, 1)
                )
                .is_err()
            );
        }
        let mut unknown = wire.clone();
        unknown.pointer_mut(&path).unwrap()["unexpected_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<DecompileJobResultReply>(unknown).is_err());
        let mut positional = wire.clone();
        *positional.pointer_mut(&path).unwrap() =
            serde_json::json!(object.values().collect::<Vec<_>>());
        assert!(serde_json::from_value::<DecompileJobResultReply>(positional).is_err());
    }
}

#[test]
fn protocol_guard_failure_and_continuation_payloads_reject_unknown_duplicate_and_positional_fields()
{
    use evm_abstract_protocol::DecompileJobResultReply;
    let reply = guard_reply();
    let wire = serde_json::to_value(&reply).unwrap();
    let stage = reply.result.as_ref().unwrap().artifact.stages.len() - 1;
    let base = format!("/result/Ok/artifact/stages/{stage}/ir/lifted");
    for suffix in [
        "/controls/0/guard",
        "/controls/0/guard/continuation",
        "/failures/0",
    ] {
        let path = format!("{base}{suffix}");
        let object = wire.pointer(&path).unwrap().as_object().unwrap();
        for key in object.keys() {
            let mut missing = wire.clone();
            missing
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(
                serde_json::from_value::<DecompileJobResultReply>(missing).is_err(),
                "missing {key} at {path}"
            );
        }
        let mut extra = wire.clone();
        extra.pointer_mut(&path).unwrap()["unknown_guard_field"] = serde_json::json!(0);
        assert!(serde_json::from_value::<DecompileJobResultReply>(extra).is_err());
        let mut positional = wire.clone();
        *positional.pointer_mut(&path).unwrap() =
            serde_json::json!(object.values().collect::<Vec<_>>());
        assert!(serde_json::from_value::<DecompileJobResultReply>(positional).is_err());
    }
    let text = serde_json::to_string(&reply).unwrap();
    for (needle, duplicate) in [
        (
            "\"pass_nonzero\":true",
            "\"pass_nonzero\":true,\"pass_nonzero\":false",
        ),
        ("\"rank\":0", "\"rank\":0,\"rank\":0"),
        (
            "\"pass_falls_through\":false",
            "\"pass_falls_through\":false,\"pass_falls_through\":false",
        ),
    ] {
        assert!(text.contains(needle));
        assert!(
            serde_json::from_str::<DecompileJobResultReply>(&text.replacen(needle, duplicate, 1))
                .is_err()
        );
    }
}

#[test]
fn execution_domain_is_explicit_optional_for_legacy_requests_and_strictly_typed() {
    use evm_abstract_protocol::decompile_model::ExecutionDomain;
    let generic = DecompileRequest::default();
    let legacy = serde_json::to_value(&generic).unwrap();
    assert!(legacy.get("execution_domain").is_none());
    assert_eq!(
        serde_json::from_value::<DecompileRequest>(legacy.clone())
            .unwrap()
            .execution_domain,
        ExecutionDomain::Generic
    );
    let selected = DecompileRequest {
        execution_domain: ExecutionDomain::EthereumTransactions,
        ..generic
    };
    let wire = serde_json::to_value(&selected).unwrap();
    assert_eq!(wire["execution_domain"], "ethereum_transactions");
    assert_eq!(
        serde_json::from_value::<DecompileRequest>(wire.clone()).unwrap(),
        selected
    );
    for invalid in [
        serde_json::json!(null),
        serde_json::json!(false),
        serde_json::json!({}),
        serde_json::json!("eth_call"),
    ] {
        let mut changed = wire.clone();
        changed["execution_domain"] = invalid;
        assert!(serde_json::from_value::<DecompileRequest>(changed).is_err());
    }
    let raw = serde_json::to_string(&selected).unwrap();
    let duplicate = raw.replace(
        "\"execution_domain\":",
        "\"execution_domain\":\"generic\",\"execution_domain\":",
    );
    assert!(serde_json::from_str::<DecompileRequest>(&duplicate).is_err());
}
