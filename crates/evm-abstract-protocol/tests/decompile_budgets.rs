//! Exact JSON-number contracts through complete decompiler API envelopes.

use evm_abstract_protocol::{
    DecimalU64, DecompileArtifact, DecompileJobReply, DecompileJobResult, DecompileJobResultReply,
    DecompileJobSnapshot, DecompileLimits, DecompileRequest, JobId, JobState, PublicationVersion,
    decompile_model::{Phase, Progress, Statistics},
};

fn limits(count: u64) -> DecompileLimits {
    DecompileLimits {
        max_bytecode_bytes: count,
        max_blocks: count,
        max_operations: count,
        max_values: count,
        max_work: count,
        max_iterations: count,
        max_constant_set: count,
        max_cloned_blocks: count,
        max_structuring_depth: count,
        max_history_entities: count,
    }
}

#[test]
fn explicit_legacy_request_retains_its_numeric_object_shape() {
    let raw = r#"{"bytecode":"00","fork":"Osaka","strategy":"dispatcher","limits":{"max_bytecode_bytes":100000,"max_blocks":8192,"max_operations":100000,"max_values":500000,"max_work":100000000,"max_iterations":100000,"max_constant_set":8,"max_cloned_blocks":2048,"max_structuring_depth":128,"max_history_entities":2000000}}"#;
    let request: DecompileRequest = serde_json::from_str(raw).unwrap();
    assert_eq!(request.limits.max_work, 100_000_000);
    assert_eq!(request.limits.max_structuring_depth, 128);
    assert_eq!(request.limits.max_constant_set, 8);
    let encoded = serde_json::to_value(request).unwrap();
    let expected: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(encoded, expected);
}

#[test]
fn full_request_progress_and_result_envelopes_roundtrip_all_wide_numeric_fields() {
    for count in [
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let request = DecompileRequest {
            bytecode: "00".into(),
            limits: limits(count),
            ..DecompileRequest::default()
        };
        let raw = serde_json::to_string(&request).unwrap();
        assert!(raw.contains(&format!("\"max_work\":{count}")));
        assert_eq!(
            serde_json::from_str::<DecompileRequest>(&raw).unwrap(),
            request
        );
        let progress = Progress {
            phase: Phase::Recovery,
            blocks: count,
            operations: count,
            work: count,
            stage: None,
        };
        let snapshot = DecompileJobReply {
            result: Ok(DecompileJobSnapshot {
                id: JobId(count),
                state: JobState::Running,
                progress,
                queue_ns: DecimalU64(count),
                processing_ns: DecimalU64(count),
                publication: None,
            }),
        };
        let raw = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<DecompileJobReply>(&raw).unwrap(),
            snapshot
        );
        let wire: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for field in ["blocks", "operations", "work"] {
            assert_eq!(
                wire["result"]["Ok"]["progress"][field].as_u64(),
                Some(count)
            );
        }
        let mut artifact: DecompileArtifact = serde_json::from_str(include_str!(
            "../../evm-decompile-model/tests/fixtures/partial-artifact.json"
        ))
        .unwrap();
        artifact.config.limits = request.limits;
        artifact.statistics = Statistics {
            work: count,
            evaluations: count,
            blocks: count,
            operations: count,
            values: count,
            edges: count,
            cloned_blocks: count,
            history_entities: count,
        };
        let reply = DecompileJobResultReply {
            result: Ok(DecompileJobResult {
                id: JobId(count),
                publication: PublicationVersion(DecimalU64(count)),
                artifact,
            }),
        };
        let raw = serde_json::to_string(&reply).unwrap();
        let decoded: DecompileJobResultReply = serde_json::from_str(&raw).unwrap();
        assert_eq!(decoded, reply);
        let wire: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let artifact = &wire["result"]["Ok"]["artifact"];
        for value in artifact["config"]["limits"].as_object().unwrap().values() {
            assert_eq!(value.as_u64(), Some(count));
        }
        for value in artifact["statistics"].as_object().unwrap().values() {
            assert_eq!(value.as_u64(), Some(count));
        }
    }
}

#[test]
fn nested_request_limits_reject_non_u64_duplicate_and_missing_fields() {
    let request = DecompileRequest {
        limits: limits(7),
        ..DecompileRequest::default()
    };
    let wire = serde_json::to_value(&request).unwrap();
    let raw = serde_json::to_string(&request).unwrap();
    for field in wire["limits"].as_object().unwrap().keys() {
        let token = format!("\"{field}\":7");
        for invalid in ["18446744073709551616", "-1", "0.5", "\"7\""] {
            let malformed = raw.replacen(&token, &format!("\"{field}\":{invalid}"), 1);
            assert!(
                serde_json::from_str::<DecompileRequest>(&malformed).is_err(),
                "{field}={invalid}"
            );
        }
        let duplicate = raw.replacen(&token, &format!("{token},{token}"), 1);
        assert!(
            serde_json::from_str::<DecompileRequest>(&duplicate).is_err(),
            "{field}"
        );
        let mut missing = wire.clone();
        missing["limits"].as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<DecompileRequest>(missing).is_err(),
            "{field}"
        );
    }
}
