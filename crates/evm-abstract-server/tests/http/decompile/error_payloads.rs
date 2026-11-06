use super::*;
use evm_abstract_protocol::{DecompileJobResult, DecompileLimits};
use evm_abstract_server::jobs::Pool;

fn fetch(input: &DecompileRequest, assets: &Assets, pool: &Pool) -> DecompileJobResult {
    let body = serde_json::to_string(input).unwrap();
    let (headers, accepted): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request(&body, "http://localhost"),
        &assets.0,
        pool,
    ));
    assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
    let id = accepted.result.unwrap().id;
    let deadline = Instant::now() + Duration::from_secs(20);
    let terminal = loop {
        let status = pool.decompile_status(id).unwrap();
        if status.state == JobState::Completed {
            break status;
        }
        assert!(
            !matches!(status.state, JobState::Failed(_) | JobState::Cancelled),
            "{status:?}"
        );
        assert!(Instant::now() < deadline, "{status:?}");
        thread::sleep(Duration::from_millis(2));
    };
    let request = format!(
        "GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n\r\n"
    );
    let (headers, reply): (_, DecompileJobResultReply) =
        decode(&exchange_with_pool(request.as_bytes(), &assets.0, pool));
    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
    assert_eq!(header_value(&headers, "content-encoding"), Some("gzip"));
    let result = reply.result.unwrap();
    assert_eq!(result.id, id);
    assert_eq!(Some(result.publication), terminal.publication);
    assert_eq!(result.artifact.current_stage, terminal.progress.stage);
    assert_eq!(result.artifact.statistics.work, terminal.progress.work);
    result
}

#[test]
fn real_compressed_http_preserves_error_bytes_lazy_ranges_sources_and_legacy_version_boundary() {
    let assets = Assets::new();
    let pool = pool();
    for (bytecode, message) in [
        (
            include_str!(
                "../../../../evm-decompile/tests/fixtures/error-payloads/router-ds-math-guard.hex"
            )
            .trim(),
            "ds-math-mul-overflow",
        ),
        (
            include_str!(
                "../../../../evm-decompile/tests/fixtures/error-payloads/router-transfer-guard.hex"
            )
            .trim(),
            "TransferHelper: TRANSFER_FAILED",
        ),
    ] {
        let input = DecompileRequest {
            bytecode: bytecode.into(),
            ..DecompileRequest::default()
        };
        let result = fetch(&input, &assets, &pool);
        let artifact = &result.artifact;
        assert_eq!(artifact.completion, Completion::Complete);
        assert_eq!(artifact.schema_version, 4);
        assert!(artifact.admits_schema());
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let payload = program
            .payloads
            .iter()
            .find(|p| p.message == message)
            .unwrap();
        assert_eq!(payload.bytes.len(), 100);
        assert_eq!(&payload.bytes[68..68 + message.len()], message.as_bytes());
        assert!(payload.bytes[68 + message.len()..].iter().all(|b| *b == 0));
        let proof = program
            .reverts
            .iter()
            .find(|r| r.payload == payload.id)
            .unwrap();
        let index = program.render_index(&stage.ir);
        index.validate_guard_sources().unwrap();
        let capsule = index.construction(proof.block).unwrap();
        assert_eq!(capsule.revert, proof.id);
        assert_eq!(proof.writes.len(), 4);
        let control = program
            .controls
            .iter()
            .find(|c| {
                c.guard.as_ref().is_some_and(|g| {
                    g.payload
                        == evm_abstract_protocol::decompile_model::LiftedGuardPayload::ErrorString(
                            payload.id,
                        )
                })
            })
            .unwrap();
        assert!(
            index
                .render_guard(control, |_| true)
                .unwrap()
                .unwrap()
                .contains(message)
        );
        for entity in [payload.id, proof.id, capsule.id] {
            let origin = artifact
                .origin(evm_abstract_protocol::decompile_model::EntityRef {
                    stage: stage.id,
                    entity,
                })
                .unwrap();
            assert!(!origin.parents.is_empty() && !origin.source.is_empty());
        }
        let restored: DecompileJobResultReply = serde_json::from_slice(
            &serde_json::to_vec(&DecompileJobResultReply {
                result: Ok(result.clone()),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(restored.result.unwrap(), result);
        for version in [1, 2, 3] {
            let mut old = artifact.clone();
            old.schema_version = version;
            assert!(!old.admits_schema());
        }
        // Recorded Passed and a valid result envelope do not authorize forged
        // memory/source ownership when a consumer builds its shared index.
        let mut wrong = stage.clone();
        let program = wrong.ir.lifted.as_mut().unwrap();
        program.reverts[0].memory = program.reverts[0].offset;
        let program = wrong.ir.lifted.as_ref().unwrap();
        let index = program.render_index(&wrong.ir);
        assert!(index.validate_guard_sources().is_err());
        assert!(index.construction(proof.block).is_none());
    }
    pool.shutdown().unwrap();
}

#[test]
fn nonempty_proof_http_budget_refusal_retains_only_the_prior_validated_publication() {
    let assets = Assets::new();
    let pool = pool();
    let input = DecompileRequest {
        bytecode: include_str!(
            "../../../../evm-decompile/tests/fixtures/error-payloads/router-ds-math-guard.hex"
        )
        .trim()
        .into(),
        ..DecompileRequest::default()
    };
    let complete = fetch(&input, &assets, &pool).artifact;
    assert!(
        !complete
            .current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .constructions
            .is_empty()
    );
    for history in [false, true] {
        let mut limited = input.clone();
        limited.limits = if history {
            DecompileLimits {
                max_history_entities: complete.statistics.history_entities - 1,
                ..input.limits.clone()
            }
        } else {
            DecompileLimits {
                max_work: complete.statistics.work - 1,
                ..input.limits.clone()
            }
        };
        let artifact = fetch(&limited, &assets, &pool).artifact;
        assert_eq!(artifact.completion, Completion::Limited);
        assert_eq!(
            artifact.current().unwrap().kind,
            evm_abstract_protocol::decompile_model::StageKind::Structured
        );
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.ir.lifted.is_none())
        );
        assert!(artifact.statistics.work <= limited.limits.max_work);
        assert!(artifact.statistics.history_entities <= limited.limits.max_history_entities);
    }
    pool.shutdown().unwrap();
}
