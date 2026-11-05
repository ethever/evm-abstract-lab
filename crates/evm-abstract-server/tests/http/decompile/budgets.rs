//! Exact numeric budgets and synthetic wide counters through real TCP.

use super::{Assets, decode, exchange_with_pool, pool, submit_request};
use evm_abstract_protocol::{
    ApiErrorCode, DecompileJobReply, DecompileJobResultReply, DecompileRequest, ErrorDetails,
    JobState, ValidationErrorKind, decompile_model::Completion,
};
use std::{
    thread,
    time::{Duration, Instant},
};

fn wide_request_body(count: u64) -> String {
    // Write numeric tokens directly; this is the literal request accepted by
    // HTTP, without an intermediate floating-point or decimal-string adapter.
    format!(
        r#"{{"bytecode":"00","fork":"Osaka","strategy":"dispatcher","limits":{{"max_bytecode_bytes":{count},"max_blocks":{count},"max_operations":{count},"max_values":{count},"max_work":{count},"max_iterations":{count},"max_constant_set":8,"max_cloned_blocks":{count},"max_structuring_depth":512,"max_history_entities":{count}}}}}"#
    )
}

fn poll_decompile(
    pool: &evm_abstract_server::jobs::Pool,
    assets: &Assets,
    id: evm_abstract_protocol::JobId,
) -> evm_abstract_protocol::DecompileJobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let request = format!("GET /api/decompile/tasks/{id} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let (headers, reply): (_, DecompileJobReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, pool));
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        let snapshot = reply.result.unwrap();
        if snapshot.state == JobState::Completed {
            return snapshot;
        }
        assert!(
            !matches!(snapshot.state, JobState::Cancelled | JobState::Failed(_)),
            "{snapshot:?}"
        );
        assert!(Instant::now() < deadline, "{snapshot:?}");
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(target_pointer_width = "64")]
#[test]
fn raw_numeric_wide_and_default_max_budgets_survive_http_submit_poll_and_result() {
    let assets = Assets::new();
    let pool = pool();
    for count in [
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let body = wide_request_body(count);
        let expected: DecompileRequest = serde_json::from_str(&body).unwrap();
        if count == u64::MAX {
            assert_eq!(expected.limits, DecompileRequest::default().limits);
        }
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&body, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
        let id = reply.result.unwrap().id;
        let done = poll_decompile(&pool, &assets, id);
        let request = format!(
            "GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n\r\n"
        );
        let (headers, reply): (_, DecompileJobResultReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        let result = reply.result.unwrap();
        assert_eq!(result.id, id);
        assert_eq!(Some(result.publication), done.publication);
        assert_eq!(result.artifact.config.limits, expected.limits);
        assert_eq!(result.artifact.completion, Completion::Complete);
        assert_eq!(result.artifact.statistics.blocks, 1);
        assert_eq!(result.artifact.statistics.operations, 1);
        assert!(result.artifact.statistics.work > 0 && result.artifact.statistics.work < 1_000_000);
        assert_eq!(done.progress.work, result.artifact.statistics.work);
        assert_eq!(done.progress.blocks, result.artifact.statistics.blocks);
        assert_eq!(
            done.progress.operations,
            result.artifact.statistics.operations
        );
        assert_eq!(done.progress.stage, result.artifact.current_stage);
        assert_eq!(
            done.progress.phase,
            evm_abstract_protocol::decompile_model::Phase::Complete
        );
    }
    pool.shutdown().unwrap();
}

#[test]
fn http_rejects_invalid_numeric_allowances_and_keeps_small_limit_artifacts_typed() {
    let assets = Assets::new();
    let pool = pool();
    let body = wide_request_body(7);
    let mut invalid_requests: Vec<_> = ["18446744073709551616", "-1", "0.5", "\"7\""]
        .map(|invalid| body.replace("\"max_work\":7", &format!("\"max_work\":{invalid}")))
        .into();
    invalid_requests.push(body.replace("\"max_work\":7", "\"max_work\":7,\"max_work\":7"));
    invalid_requests.push(body.replace("\"max_work\":7,", ""));
    for malformed in invalid_requests {
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&malformed, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(
            headers.starts_with("HTTP/1.1 400"),
            "{malformed}: {headers}"
        );
        let error = reply.result.unwrap_err();
        assert_eq!(error.code, ApiErrorCode::InvalidRequest);
        let ErrorDetails::Validation(details) = error.details else {
            panic!("typed JSON validation missing");
        };
        assert_eq!(details.kind, ValidationErrorKind::Json);
    }
    for (field, value, completion) in [
        ("max_work", 0, Completion::Limited),
        ("max_work", 1, Completion::Limited),
        ("max_structuring_depth", 513, Completion::Failed),
        ("max_constant_set", 0, Completion::Failed),
    ] {
        let mut request = DecompileRequest {
            bytecode: "00".into(),
            ..DecompileRequest::default()
        };
        match field {
            "max_work" => request.limits.max_work = value,
            "max_structuring_depth" => request.limits.max_structuring_depth = value,
            "max_constant_set" => request.limits.max_constant_set = value,
            _ => unreachable!(),
        }
        let expected = request.limits.clone();
        let body = serde_json::to_string(&request).unwrap();
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&body, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
        let id = reply.result.unwrap().id;
        poll_decompile(&pool, &assets, id);
        let request =
            format!("GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let (_, reply): (_, DecompileJobResultReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        let artifact = reply.result.unwrap().artifact;
        assert_eq!(artifact.config.limits, expected);
        assert_eq!(artifact.completion, completion, "{field}={value}");
        assert!(
            artifact
                .diagnostics
                .iter()
                .any(|d| d.code == evm_abstract_protocol::decompile_model::DiagnosticCode::Limit)
        );
    }
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct WideCounters {
    count: u64,
    observed: std::sync::mpsc::SyncSender<()>,
    release: std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
}

impl evm_abstract_server::jobs::Executor for WideCounters {
    fn execute(
        &self,
        _: evm_abstract_protocol::AnalyzeRequest,
        _: &evm_abstract::analysis::control::Control,
    ) -> Result<evm_abstract_protocol::AnalysisReport, evm_abstract_protocol::ApiError> {
        unreachable!("this transport fixture only receives decompiler jobs")
    }

    fn decompile(
        &self,
        request: DecompileRequest,
        _: &evm_abstract::analysis::control::Control,
        observer: &evm_abstract_server::decompile::Observer,
    ) -> Result<evm_abstract_protocol::DecompileArtifact, evm_abstract_protocol::ApiError> {
        let mut artifact: evm_abstract_protocol::DecompileArtifact = serde_json::from_str(
            include_str!("../../../../evm-decompile-model/tests/fixtures/partial-artifact.json"),
        )
        .unwrap();
        artifact.config.strategy = request.strategy;
        artifact.config.limits = request.limits;
        let count = self.count;
        // Synthetic counts exercise scheduler/HTTP storage and codecs. They do
        // not claim that the small retained IR executed billions of operations.
        artifact.statistics = evm_abstract_protocol::decompile_model::Statistics {
            work: count,
            evaluations: count,
            blocks: count,
            operations: count,
            values: count,
            edges: count,
            cloned_blocks: count,
            history_entities: count,
        };
        observer.observe(evm_abstract_protocol::decompile_model::Progress {
            phase: evm_abstract_protocol::decompile_model::Phase::Recovery,
            blocks: count,
            operations: count,
            work: count,
            stage: None,
        });
        self.observed.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        Ok(artifact)
    }
}

#[test]
fn wide_observations_and_every_final_statistic_cross_actual_http_without_numeric_loss() {
    let assets = Assets::new();
    for count in [
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let (observed, waiting) = std::sync::mpsc::sync_channel(1);
        let (release, receiver) = std::sync::mpsc::sync_channel(1);
        let pool = evm_abstract_server::jobs::Pool::with_executor(
            evm_abstract_server::jobs::Config {
                workers: 1,
                queue_capacity: 1,
                retained_jobs: 2,
            },
            WideCounters {
                count,
                observed,
                release: std::sync::Arc::new(std::sync::Mutex::new(receiver)),
            },
        )
        .unwrap();
        let body = wide_request_body(count);
        let expected: DecompileRequest = serde_json::from_str(&body).unwrap();
        let (headers, reply): (_, DecompileJobReply) = decode(&exchange_with_pool(
            &submit_request(&body, "http://localhost"),
            &assets.0,
            &pool,
        ));
        assert!(headers.starts_with("HTTP/1.1 202"), "{headers}");
        let id = reply.result.unwrap().id;
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        let request = format!("GET /api/decompile/tasks/{id} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (headers, reply): (_, DecompileJobReply) =
                decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
            assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
            let snapshot = reply.result.unwrap();
            assert_eq!(snapshot.state, JobState::Running);
            if snapshot.progress.work == count {
                assert_eq!(snapshot.progress.blocks, count);
                assert_eq!(snapshot.progress.operations, count);
                assert_eq!(
                    snapshot.progress.phase,
                    evm_abstract_protocol::decompile_model::Phase::Recovery
                );
                assert_eq!(snapshot.progress.stage, None);
                assert_eq!(snapshot.publication, None);
                break;
            }
            assert!(Instant::now() < deadline, "{snapshot:?}");
            thread::sleep(Duration::from_millis(2));
        }
        release.send(()).unwrap();
        let done = poll_decompile(&pool, &assets, id);
        assert_eq!(done.progress.work, count);
        assert_eq!(done.progress.blocks, count);
        assert_eq!(done.progress.operations, count);
        assert_eq!(
            done.progress.phase,
            evm_abstract_protocol::decompile_model::Phase::Complete
        );
        let request = format!(
            "GET /api/decompile/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n\r\n"
        );
        let (headers, reply): (_, DecompileJobResultReply) =
            decode(&exchange_with_pool(request.as_bytes(), &assets.0, &pool));
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        let result = reply.result.unwrap();
        assert_eq!(result.artifact.config.limits, expected.limits);
        assert_eq!(Some(result.publication), done.publication);
        assert_eq!(result.artifact.current_stage, done.progress.stage);
        assert_eq!(
            result.artifact.statistics,
            evm_abstract_protocol::decompile_model::Statistics {
                work: count,
                evaluations: count,
                blocks: count,
                operations: count,
                values: count,
                edges: count,
                cloned_blocks: count,
                history_entities: count,
            }
        );
        pool.shutdown().unwrap();
    }
}
