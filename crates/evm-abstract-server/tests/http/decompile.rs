use super::{Assets, decoded_content, exchange_with_pool, header_value, pool, read_response};
use evm_abstract_protocol::{
    ApiErrorCode, DecompileJobReply, DecompileJobResultReply, DecompileRequest, DecompileStrategy,
    ErrorDetails, JobState, ValidationErrorKind, decompile_model::Completion,
};
use std::{
    io::BufReader,
    thread,
    time::{Duration, Instant},
};

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> (String, T) {
    let (headers, bytes) = read_response(&mut BufReader::new(bytes));
    let body = decoded_content(header_value(&headers, "content-encoding"), &bytes);
    (headers, serde_json::from_slice(&body).unwrap())
}

fn submit_request(body: &str, origin: &str) -> Vec<u8> {
    format!(
        "POST /api/decompile/tasks HTTP/1.1\r\nHost: localhost\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len(),
    ).into_bytes()
}

#[test]
fn both_policies_and_partial_results_cross_the_real_compressed_http_boundary() {
    let assets = Assets::new();
    let pool = pool();
    for (bytecode, strategy, completion) in [
        (
            "600260030100",
            DecompileStrategy::Dispatcher,
            Completion::Complete,
        ),
        (
            "600260030100",
            DecompileStrategy::CodeDuplication,
            Completion::Complete,
        ),
        ("5f3556", DecompileStrategy::Dispatcher, Completion::Partial),
    ] {
        let input = DecompileRequest {
            bytecode: bytecode.into(),
            strategy,
            ..DecompileRequest::default()
        };
        let body = serde_json::to_string(&input).unwrap();
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&body, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
        let id = reply.result.unwrap().id;
        let deadline = Instant::now() + Duration::from_secs(20);
        let terminal = loop {
            let request =
                format!("GET /api/decompile/tasks/{id} HTTP/1.1\r\nHost: localhost\r\n\r\n");
            let (_, reply): (_, DecompileJobReply) =
                decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
            let snapshot = reply.result.unwrap();
            if snapshot.state == JobState::Completed {
                break snapshot;
            }
            assert!(
                !matches!(snapshot.state, JobState::Failed(_) | JobState::Cancelled),
                "{snapshot:?}"
            );
            assert!(Instant::now() < deadline, "{snapshot:?}");
            thread::sleep(Duration::from_millis(2));
        };
        let request = format!(
            "GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n\r\n"
        );
        let (headers, reply): (_, DecompileJobResultReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        let result = reply.result.unwrap();
        assert_eq!(result.id, id);
        assert_eq!(Some(result.publication), terminal.publication);
        assert_eq!(result.artifact.config.strategy, strategy);
        assert_eq!(result.artifact.completion, completion);
        assert_eq!(result.artifact.current_stage, terminal.progress.stage);
        assert_eq!(result.artifact.statistics.work, terminal.progress.work);
        assert!(!result.artifact.stages.is_empty());

        let request = format!("GET /api/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let (_, reply): (_, evm_abstract_protocol::JobResultReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        assert_eq!(reply.result.unwrap_err().code, ApiErrorCode::TaskNotFound);
    }
    pool.shutdown().unwrap();
}

#[test]
fn strict_configuration_and_origin_policy_apply_before_decompiler_admission() {
    let assets = Assets::new();
    let pool = pool();
    let body = serde_json::to_string(&DecompileRequest::default()).unwrap();
    let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request(&body, "http://other.example"),
        &assets.0,
        &pool,
    ));
    assert!(headers.starts_with("HTTP/1.1 400"));
    assert!(reply.result.is_err());

    for field in ["max_cloned_bloks", "unknown_policy"] {
        let mut input = serde_json::to_value(DecompileRequest::default()).unwrap();
        input["limits"][field] = serde_json::json!(0);
        let body = serde_json::to_string(&input).unwrap();
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&body, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(headers.starts_with("HTTP/1.1 400"), "{headers}");
        assert_eq!(reply.result.unwrap_err().code, ApiErrorCode::InvalidRequest);
    }
    pool.shutdown().unwrap();
}

#[test]
fn json_and_hex_errors_preserve_their_authoritative_validation_categories() {
    let assets = Assets::new();
    let pool = pool();
    let (_, malformed): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request("{", "http://localhost"),
        &assets.0,
        &pool,
    ));
    let error = malformed.result.unwrap_err();
    assert_eq!(error.code, ApiErrorCode::InvalidRequest);
    let ErrorDetails::Validation(details) = error.details else {
        panic!("JSON validation details missing");
    };
    assert_eq!(details.kind, ValidationErrorKind::Json);

    let input = DecompileRequest {
        bytecode: "0xgg".into(),
        ..DecompileRequest::default()
    };
    let body = serde_json::to_string(&input).unwrap();
    let (headers, malformed): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request(&body, "http://localhost"),
        &assets.0,
        &pool,
    ));
    assert!(headers.starts_with("HTTP/1.1 400"));
    let error = malformed.result.unwrap_err();
    assert_eq!(error.code, ApiErrorCode::InvalidBytecode);
    let ErrorDetails::Validation(details) = error.details else {
        panic!("bytecode validation details missing");
    };
    assert_eq!(details.kind, ValidationErrorKind::Bytecode);
    assert_eq!(details.field.as_deref(), Some("bytecode"));

    let input = DecompileRequest {
        bytecode: "ef00".into(),
        limits: evm_abstract_protocol::DecompileLimits {
            max_bytecode_bytes: 0,
            ..evm_abstract_protocol::DecompileLimits::default()
        },
        ..DecompileRequest::default()
    };
    let body = serde_json::to_string(&input).unwrap();
    let (headers, accepted): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request(&body, "http://localhost"),
        &assets.0,
        &pool,
    ));
    assert!(headers.starts_with("HTTP/1.1 202"));
    let id = accepted.result.unwrap().id;
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.decompile_status(id).unwrap().state != JobState::Completed {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let result = pool.decompile_result(id).unwrap();
    assert_eq!(result.artifact.completion, Completion::Limited);
    assert!(
        !result
            .artifact
            .diagnostics
            .iter()
            .any(|d| d.code == evm_abstract_protocol::decompile_model::DiagnosticCode::Decode)
    );
    pool.shutdown().unwrap();
}

#[test]
fn deeply_nested_structured_artifact_roundtrips_inside_the_actual_result_envelope() {
    // Twenty-five nested loops exceed the old incidental JSON-tree limit while
    // remaining comfortably inside the declared semantic structuring depth.
    // A real memory definition also exercises the nullable producing-operation
    // field inside the populated HTTP result, alongside branches and loops.
    let mut bytes = vec![0x60, 0x2a, 0x5f, 0x52];
    let mut headers = Vec::new();
    let mut fixups = Vec::new();
    for _ in 0..25 {
        headers.push(u16::try_from(bytes.len()).unwrap());
        bytes.extend([0x5b, 0x5f, 0x35, 0x61]);
        fixups.push(bytes.len());
        bytes.extend([0, 0, 0x57]);
    }
    bytes.push(0x61);
    bytes.extend(headers.last().unwrap().to_be_bytes());
    bytes.push(0x56);
    for index in (0..headers.len()).rev() {
        let destination = u16::try_from(bytes.len()).unwrap().to_be_bytes();
        bytes[fixups[index]..fixups[index] + 2].copy_from_slice(&destination);
        bytes.push(0x5b);
        if index == 0 {
            bytes.push(0x00);
        } else {
            bytes.push(0x61);
            bytes.extend(headers[index - 1].to_be_bytes());
            bytes.push(0x56);
        }
    }
    let bytecode = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let input = DecompileRequest {
        bytecode,
        ..DecompileRequest::default()
    };
    let assets = Assets::new();
    let pool = pool();
    let body = serde_json::to_string(&input).unwrap();
    let (_, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
        &submit_request(&body, "http://localhost"),
        &assets.0,
        &pool,
    ));
    let id = reply.result.unwrap().id;
    let deadline = Instant::now() + Duration::from_secs(20);
    while pool.decompile_status(id).unwrap().state != JobState::Completed {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let request = format!(
        "GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n\r\n"
    );
    let (_, reply): (_, DecompileJobResultReply) =
        decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
    assert_named_result_payloads_are_strict(&reply);
    let artifact = reply.result.unwrap().artifact;
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    let stage = artifact.current().unwrap();
    assert_eq!(
        stage.kind,
        evm_abstract_protocol::decompile_model::StageKind::Structured
    );
    assert!(stage.ir.structured.is_some());
    assert_eq!(
        serde_json::from_slice::<evm_abstract_protocol::DecompileArtifact>(
            &serde_json::to_vec(&artifact).unwrap()
        )
        .unwrap(),
        artifact
    );
    pool.shutdown().unwrap();
}

fn assert_named_result_payloads_are_strict(reply: &DecompileJobResultReply) {
    let wire = serde_json::to_value(reply).unwrap();
    let stages = wire["result"]["Ok"]["artifact"]["stages"]
        .as_array()
        .unwrap();
    let mut paths = Vec::new();
    for (stage, snapshot) in stages.iter().enumerate() {
        for (index, value) in snapshot["ir"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if value["kind"].get("memory").is_some() {
                paths.push((
                    format!("/result/Ok/artifact/stages/{stage}/ir/values/{index}/kind/memory"),
                    "operation",
                ));
                break;
            }
        }
        for (index, block) in snapshot["ir"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if block["terminator"].get("branch").is_some() {
                paths.push((
                    format!(
                        "/result/Ok/artifact/stages/{stage}/ir/blocks/{index}/terminator/branch"
                    ),
                    "else_edge",
                ));
                break;
            }
        }
        if let Some(nodes) = snapshot["ir"]["structured"]["nodes"].as_array() {
            for (index, node) in nodes.iter().enumerate() {
                if node["kind"].get("while").is_some() {
                    paths.push((format!("/result/Ok/artifact/stages/{stage}/ir/structured/nodes/{index}/kind/while"), "condition"));
                    break;
                }
            }
        }
    }
    for field in ["operation", "else_edge", "condition"] {
        assert!(
            paths.iter().any(|(_, found)| *found == field),
            "missing populated {field} payload"
        );
    }
    for (path, required) in paths {
        let mut missing = wire.clone();
        assert!(
            missing
                .pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(required)
                .is_some()
        );
        assert!(
            serde_json::from_slice::<DecompileJobResultReply>(
                &serde_json::to_vec(&missing).unwrap()
            )
            .is_err(),
            "accepted missing {required} at {path}"
        );
        let mut extra = wire.clone();
        extra.pointer_mut(&path).unwrap()["unexpected_field"] = serde_json::json!(0);
        assert!(
            serde_json::from_slice::<DecompileJobResultReply>(&serde_json::to_vec(&extra).unwrap())
                .is_err(),
            "accepted unknown field at {path}"
        );
        let mut positional = wire.clone();
        let fields = positional
            .pointer(&path)
            .unwrap()
            .as_object()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        *positional.pointer_mut(&path).unwrap() = serde_json::json!(fields);
        assert!(
            serde_json::from_slice::<DecompileJobResultReply>(
                &serde_json::to_vec(&positional).unwrap()
            )
            .is_err(),
            "accepted positional payload at {path}"
        );
    }
}
