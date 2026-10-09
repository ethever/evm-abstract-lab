//! Actual TCP tests cover framing, typed API responses and the static asset host.

use evm_abstract_protocol::{
    AnalysisInput, AnalyzeReply, AnalyzeRequest, ApiErrorCode, BlockSelector, ErrorDetails,
    JobReply, JobState, RpcInput, RpcProvidersReply,
};
use evm_abstract_server::http::serve_connection;
use evm_abstract_server::jobs::{Config, Pool};
use evm_abstract_server::rpc_providers::Registry;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct Assets(PathBuf);
impl Assets {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "evm-web-http-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("index.html"), "<canvas id=workbench></canvas>").unwrap();
        fs::write(path.join("app.wasm"), b"\0asm").unwrap();
        Self(path)
    }
}
impl Drop for Assets {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn exchange(bytes: &[u8], assets: &Path) -> Vec<u8> {
    let pool = pool();
    let response = exchange_with_pool(bytes, assets, &pool);
    pool.shutdown().unwrap();
    response
}

fn pool() -> Pool {
    Pool::new(Config {
        workers: 1,
        queue_capacity: 2,
        retained_jobs: 4,
    })
    .unwrap()
}

fn exchange_with_pool(bytes: &[u8], assets: &Path, pool: &Pool) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let assets = assets.to_owned();
    thread::scope(|scope| {
        // Scope joins and propagates server panics without exposing std's
        // dynamically typed panic payload at this crate's typed boundary.
        scope.spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &assets, pool).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client.write_all(bytes).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        response
    })
}

fn decode(bytes: &[u8]) -> (String, AnalyzeReply) {
    let boundary = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let header = std::str::from_utf8(&bytes[..boundary]).unwrap().to_owned();
    let reply = serde_json::from_slice(&bytes[boundary + 4..]).unwrap();
    (header, reply)
}

#[test]
fn real_transport_roundtrip_returns_structured_analysis() {
    let assets = Assets::new();
    let pool = pool();
    let input = AnalyzeRequest::default();
    let body = serde_json::to_vec(&input).unwrap();
    let mut request = format!("POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    request.extend(body);
    let response = exchange_with_pool(&request, &assets.0, &pool);
    let (header, reply) = decode_job(&response);
    assert!(header.starts_with("HTTP/1.1 202"));
    let id = reply.result.unwrap().id;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = exchange_with_pool(
            format!("GET /api/tasks/{id} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            &assets.0,
            &pool,
        );
        let status = decode_job(&response).1.result.unwrap();
        if status.state == JobState::Completed {
            break;
        }
        assert!(
            !matches!(status.state, JobState::Failed(_)),
            "{:?}",
            status.state
        );
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let (header, reply) = decode(&exchange_with_pool(
        format!("GET /api/tasks/{id}/result HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        &assets.0,
        &pool,
    ));
    assert!(header.starts_with("HTTP/1.1 200"));
    let report = reply.result.unwrap();
    assert_eq!(report.disassembly[0].instructions[2].name, "ADD");
    assert_eq!(report.ssa.blocks[0].instructions[2].operands.len(), 2);
    pool.shutdown().unwrap();
}

fn decode_job(bytes: &[u8]) -> (String, JobReply) {
    let boundary = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    (
        std::str::from_utf8(&bytes[..boundary]).unwrap().to_owned(),
        serde_json::from_slice(&bytes[boundary + 4..]).unwrap(),
    )
}

#[test]
fn http_poll_assets_and_cancel_remain_responsive_during_pending_rpc() {
    let assets = Assets::new();
    let rpc = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", rpc.local_addr().unwrap());
    let providers = Registry::from_json(
        format!(r#"{{"providers":[{{"id":"fixture","name":"Fixture","endpoint":"{endpoint}"}}]}}"#)
            .as_bytes(),
    )
    .unwrap();
    let pool = Pool::with_providers(
        Config {
            workers: 1,
            queue_capacity: 2,
            retained_jobs: 4,
        },
        providers,
    )
    .unwrap();
    let (accepted, request_started) = mpsc::sync_channel(1);
    let (closed, disconnected) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        scope.spawn(move || {
            let (stream, _) = rpc.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert!(std::str::from_utf8(&body).unwrap().contains("eth_chainId"));
            accepted.send(()).unwrap();
            // Intentionally never produce response headers. Cancellation must
            // close this real pending request before publishing Cancelled.
            let mut byte = [0];
            closed.send(reader.read(&mut byte).unwrap() == 0).unwrap();
        });
        let input = AnalyzeRequest {
            input: AnalysisInput::Rpc(RpcInput {
                provider_id: "fixture".into(),
                address: "0x1111111111111111111111111111111111111111".into(),
                block: BlockSelector::Latest,
                accounts: Vec::new(),
            }),
            ..AnalyzeRequest::default()
        };
        let body = serde_json::to_vec(&input).unwrap();
        let mut request = format!("POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        request.extend(body);
        let job = decode_job(&exchange_with_pool(&request, &assets.0, &pool))
            .1
            .result
            .unwrap();
        request_started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let poll = format!(
            "GET /api/tasks/{} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            job.id
        );
        assert_eq!(
            decode_job(&exchange_with_pool(poll.as_bytes(), &assets.0, &pool))
                .1
                .result
                .unwrap()
                .state,
            JobState::Running
        );
        let asset = exchange_with_pool(
            b"GET /app.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n",
            &assets.0,
            &pool,
        );
        assert!(asset.starts_with(b"HTTP/1.1 200"));
        let result_path = format!(
            "GET /api/tasks/{}/result HTTP/1.1\r\nHost: localhost\r\n\r\n",
            job.id
        );
        assert_eq!(
            decode(&exchange_with_pool(
                result_path.as_bytes(),
                &assets.0,
                &pool
            ))
            .1
            .result
            .unwrap_err()
            .code,
            ApiErrorCode::TaskNotReady
        );
        let cancel = format!(
            "DELETE /api/tasks/{} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            job.id
        );
        assert_eq!(
            decode_job(&exchange_with_pool(cancel.as_bytes(), &assets.0, &pool))
                .1
                .result
                .unwrap()
                .state,
            JobState::Cancelling
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = decode_job(&exchange_with_pool(poll.as_bytes(), &assets.0, &pool))
                .1
                .result
                .unwrap();
            if status.state == JobState::Cancelled {
                break;
            }
            assert_eq!(status.state, JobState::Cancelling);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(disconnected.recv_timeout(Duration::from_secs(1)).unwrap());
        let (header, reply) = decode(&exchange_with_pool(
            result_path.as_bytes(),
            &assets.0,
            &pool,
        ));
        assert!(header.starts_with("HTTP/1.1 410"));
        assert_eq!(reply.result.unwrap_err().code, ApiErrorCode::Cancelled);
    });
    pool.shutdown().unwrap();
}

#[test]
fn framing_errors_and_limits_return_typed_envelopes() {
    let assets = Assets::new();
    for (request, code, status) in [
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Length: 262145\r\nContent-Type: application/json\r\n\r\n",
            ApiErrorCode::RequestTooLarge,
            "413",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 1\r\n\r\n{",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{}",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nOrigin: https://other.example\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "GET /api/tasks HTTP/1.1\r\nHost: localhost\r\n\r\n",
            ApiErrorCode::MethodNotAllowed,
            "405",
        ),
    ] {
        let (header, reply) = decode(&exchange(request.as_bytes(), &assets.0));
        assert!(
            header.starts_with(&format!("HTTP/1.1 {status}")),
            "{header}"
        );
        assert_eq!(reply.result.unwrap_err().code, code);
    }
}

#[test]
fn static_host_has_wasm_mime_and_rejects_traversal_and_missing_paths() {
    let assets = Assets::new();
    let response = exchange(
        b"GET /app.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &assets.0,
    );
    assert!(
        std::str::from_utf8(&response)
            .unwrap()
            .contains("Content-Type: application/wasm")
    );
    let response = exchange(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", &assets.0);
    assert!(
        std::str::from_utf8(&response)
            .unwrap()
            .ends_with("<canvas id=workbench></canvas>")
    );
    for path in [
        "/../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/missing",
        "/sub/../../Cargo.toml",
    ] {
        let response = exchange(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
            &assets.0,
        );
        assert_eq!(
            decode(&response).1.result.unwrap_err().code,
            ApiErrorCode::NotFound
        );
    }
}

#[cfg(unix)]
#[test]
fn canonical_path_check_blocks_symlinks_outside_asset_root() {
    let assets = Assets::new();
    let other = Assets::new();
    std::os::unix::fs::symlink(other.0.join("index.html"), assets.0.join("escape.html")).unwrap();
    let response = exchange(
        b"GET /escape.html HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &assets.0,
    );
    assert_eq!(
        decode(&response).1.result.unwrap_err().code,
        ApiErrorCode::NotFound
    );
}

fn decode_providers(bytes: &[u8]) -> (String, RpcProvidersReply) {
    let boundary = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    (
        std::str::from_utf8(&bytes[..boundary]).unwrap().to_owned(),
        serde_json::from_slice(&bytes[boundary + 4..]).unwrap(),
    )
}

#[test]
fn provider_route_exposes_only_public_metadata_and_empty_configuration() {
    let assets = Assets::new();
    let providers = Registry::from_json(br#"{"providers":[{"id":"mainnet","name":"Ethereum mainnet","endpoint":"https://user:secret-pass@example.com/private?key=secret-key"},{"id":"local","name":"Local node","endpoint":"http://127.0.0.1:8545"}]}"#).unwrap();
    let configured = Pool::with_providers(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 1,
        },
        providers,
    )
    .unwrap();
    let response = exchange_with_pool(
        b"GET /api/rpc-providers HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &assets.0,
        &configured,
    );
    let (headers, reply) = decode_providers(&response);
    assert!(headers.starts_with("HTTP/1.1 200"));
    let catalogue = reply.result.unwrap();
    assert_eq!(catalogue.len(), 2);
    assert_eq!(catalogue[0].id, "mainnet");
    assert_eq!(catalogue[0].name, "Ethereum mainnet");
    assert_eq!(catalogue[1].id, "local");
    let text = std::str::from_utf8(&response).unwrap();
    for secret in [
        "endpoint",
        "secret",
        "user",
        "example.com",
        "127.0.0.1",
        "private",
    ] {
        assert!(!text.contains(secret));
    }
    let (headers, reply) = decode_providers(&exchange_with_pool(
        b"POST /api/rpc-providers HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        &assets.0,
        &configured,
    ));
    assert!(headers.starts_with("HTTP/1.1 405"));
    assert_eq!(
        reply.result.unwrap_err().code,
        ApiErrorCode::MethodNotAllowed
    );
    configured.shutdown().unwrap();
    let (headers, reply) = decode_providers(&exchange(
        b"GET /api/rpc-providers HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &assets.0,
    ));
    assert!(headers.starts_with("HTTP/1.1 200"));
    assert!(reply.result.unwrap().is_empty());
}

#[test]
fn rpc_submission_rejects_unknown_ids_and_legacy_endpoints_before_network_or_job_creation() {
    let assets = Assets::new();
    let rpc = TcpListener::bind("127.0.0.1:0").unwrap();
    rpc.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/secret?key=secret-key", rpc.local_addr().unwrap());
    let providers = Registry::from_json(
        format!(
            r#"{{"providers":[{{"id":"registered","name":"Fixture","endpoint":"{endpoint}"}}]}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    let pool = Pool::with_providers(
        Config {
            workers: 1,
            queue_capacity: 0,
            retained_jobs: 1,
        },
        providers,
    )
    .unwrap();
    let input = AnalyzeRequest {
        input: AnalysisInput::Rpc(RpcInput {
            provider_id: "unknown".into(),
            address: "0x1111111111111111111111111111111111111111".into(),
            block: BlockSelector::Latest,
            accounts: Vec::new(),
        }),
        ..AnalyzeRequest::default()
    };
    let valid = serde_json::to_string(&input).unwrap();
    for (body, provider_error) in [
        (valid.clone(), true),
        (valid.replace("unknown", &endpoint), true),
        (
            valid.replace(
                r#""provider_id":"unknown""#,
                &format!(r#""endpoint":"{endpoint}""#),
            ),
            false,
        ),
        (
            valid.replace(
                r#""provider_id":"unknown""#,
                &format!(r#""provider_id":"registered","endpoint":"{endpoint}""#),
            ),
            false,
        ),
    ] {
        let request = format!(
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let response = exchange_with_pool(request.as_bytes(), &assets.0, &pool);
        let (header, reply) = decode_job(&response);
        assert!(header.starts_with("HTTP/1.1 400"));
        let error = reply.result.unwrap_err();
        assert_eq!(error.code, ApiErrorCode::InvalidRequest);
        if provider_error {
            let ErrorDetails::Validation(detail) = error.details else {
                panic!("typed validation")
            };
            assert_eq!(detail.field.as_deref(), Some("provider_id"));
            assert_eq!(detail.value, None);
        }
        assert!(!std::str::from_utf8(&response).unwrap().contains("secret"));
        assert_eq!(
            rpc.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    // Invalid RPC inputs never consumed a slot; bytecode still enters the pool.
    pool.submit(AnalyzeRequest::default()).unwrap();
    pool.shutdown().unwrap();
}
