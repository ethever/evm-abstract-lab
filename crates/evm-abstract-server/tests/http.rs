//! Actual TCP tests cover framing, typed API responses and the static asset host.

use evm_abstract_protocol::{
    AnalysisInput, AnalyzeReply, AnalyzeRequest, ApiErrorCode, BlockSelector, ErrorDetails,
    JobReply, JobState, RpcInput, RpcProvidersReply,
};
use evm_abstract_server::http::{router, serve_with_shutdown};
use evm_abstract_server::jobs::{Config, Pool};
use evm_abstract_server::rpc_providers::Registry;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
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
    // This single-exchange helper explicitly requests closure; keepalive has a
    // separate test that sends a second request on the same real connection.
    let boundary = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap();
    let mut request = bytes[..boundary].to_vec();
    request.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    request.extend_from_slice(&bytes[boundary + 4..]);
    with_server(assets, pool, |address| {
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        client.write_all(&request).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        response
    })
}

struct Stop(Option<tokio::sync::oneshot::Sender<()>>);
impl Drop for Stop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn with_server<T>(assets: &Path, pool: &Pool, interact: impl FnOnce(SocketAddr) -> T) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let assets = assets.to_owned();
    let pool = pool.clone();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    thread::scope(|scope| {
        scope.spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                serve_with_shutdown(listener, router(assets, pool), async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
            });
        });
        let _stop = Stop(Some(shutdown));
        interact(address)
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
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nOrigin: http://localhost\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 1\r\n\r\n{",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
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
fn origin_server_rejects_proxy_targets_and_ambiguous_paths() {
    let assets = Assets::new();
    for target in [
        "http://other.example/api/rpc-providers",
        "http://localhost/api/rpc-providers",
        "//api/rpc-providers",
    ] {
        let request =
            format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\n\r\n");
        let (headers, reply) = decode_job(&exchange(request.as_bytes(), &assets.0));
        assert!(headers.starts_with("HTTP/1.1 400"), "{headers}");
        assert_eq!(reply.result.unwrap_err().code, ApiErrorCode::InvalidRequest);
    }
}

fn chunked(body: &[u8]) -> Vec<u8> {
    let mut request = b"POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for chunk in body.chunks(4096) {
        request.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        request.extend_from_slice(chunk);
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"0\r\n\r\n");
    request
}

#[test]
fn hyper_accepts_chunked_json_and_bounds_the_actual_streamed_body() {
    let assets = Assets::new();
    let pool = pool();
    let body = serde_json::to_vec(&AnalyzeRequest::default()).unwrap();
    let (headers, reply) = decode_job(&exchange_with_pool(&chunked(&body), &assets.0, &pool));
    assert!(headers.starts_with("HTTP/1.1 202"));
    assert!(reply.result.is_ok());
    let oversized = vec![b' '; evm_abstract_protocol::MAX_REQUEST_BYTES + 1];
    let (headers, reply) = decode_job(&exchange_with_pool(&chunked(&oversized), &assets.0, &pool));
    assert!(headers.starts_with("HTTP/1.1 413"));
    assert_eq!(
        reply.result.unwrap_err().code,
        ApiErrorCode::RequestTooLarge
    );
    pool.shutdown().unwrap();
}

fn read_response(reader: &mut BufReader<TcpStream>) -> (String, Vec<u8>) {
    let mut header = String::new();
    let mut length = None;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
        header.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0; length.expect("framework exposes a known body length")];
    reader.read_exact(&mut body).unwrap();
    (header, body)
}

#[test]
fn hyper_reuses_one_tcp_connection_for_a_second_request() {
    let assets = Assets::new();
    let pool = pool();
    with_server(&assets.0, &pool, |address| {
        let client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(client);
        reader
            .get_mut()
            .write_all(b"GET /api/rpc-providers HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let (header, body) = read_response(&mut reader);
        assert!(header.starts_with("HTTP/1.1 200"));
        assert!(!header.to_ascii_lowercase().contains("connection: close"));
        assert!(
            serde_json::from_slice::<RpcProvidersReply>(&body)
                .unwrap()
                .result
                .unwrap()
                .is_empty()
        );
        reader
            .get_mut()
            .write_all(b"GET /app.wasm HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let (header, body) = read_response(&mut reader);
        assert_eq!(body, b"\0asm");
        let header = header.to_ascii_lowercase();
        assert!(header.contains("content-type: application/wasm"));
        assert!(header.contains("cache-control: no-store"));
        assert!(header.contains("x-content-type-options: nosniff"));
    });
    pool.shutdown().unwrap();
}

#[test]
fn connection_capacity_includes_clients_still_sending_headers() {
    let assets = Assets::new();
    let pool = pool();
    with_server(&assets.0, &pool, |address| {
        let mut occupied = Vec::new();
        for _ in 0..32 {
            let mut client = TcpStream::connect(address).unwrap();
            client
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
                .unwrap();
            occupied.push(client);
        }
        let mut waiting = TcpStream::connect(address).unwrap();
        waiting
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        waiting
            .write_all(
                b"GET /api/rpc-providers HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut byte = [0];
        let blocked = waiting.read(&mut byte).unwrap_err();
        assert!(matches!(
            blocked.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));

        // The admitted connection releases its permit when its transport
        // finishes, so the next client can enter without waiting for deadlines.
        let mut finished = occupied.remove(0);
        finished
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        finished.write_all(b"Connection: close\r\n\r\n").unwrap();
        let mut first_reply = Vec::new();
        finished.read_to_end(&mut first_reply).unwrap();
        assert!(first_reply.starts_with(b"HTTP/1.1 200"));
        waiting
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reply = Vec::new();
        waiting.read_to_end(&mut reply).unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 200"));
    });
    pool.shutdown().unwrap();
}

#[test]
fn malformed_http_is_rejected_by_hyper_before_application_json() {
    let assets = Assets::new();
    for request in [
        b"POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
        b"GET / HTTP/1.1\r\nHost: localhost\r\nHost: localhost\r\n\r\n".to_vec(),
        format!("GET / HTTP/1.1\r\nHost: localhost\r\nX-Fill: {}\r\n\r\n", "a".repeat(9000)).into_bytes(),
    ] {
        let response = exchange(&request, &assets.0);
        let header = String::from_utf8_lossy(&response);
        assert!(header.starts_with("HTTP/1.1 400") || header.starts_with("HTTP/1.1 431"), "{header}");
    }
}

#[test]
fn request_body_deadline_is_total_even_when_the_declared_body_is_incomplete() {
    let assets = Assets::new();
    let started = Instant::now();
    let (header, reply) = decode_job(&exchange(b"POST /api/tasks HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{}", &assets.0));
    assert!(header.starts_with("HTTP/1.1 400"));
    assert_eq!(reply.result.unwrap_err().code, ApiErrorCode::InvalidRequest);
    assert!(started.elapsed() >= Duration::from_secs(9));
}

#[test]
fn header_deadline_releases_a_client_that_never_finishes_headers() {
    let assets = Assets::new();
    let pool = pool();
    with_server(&assets.0, &pool, |address| {
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();
        let started = Instant::now();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(started.elapsed() >= Duration::from_secs(9));
        assert!(response.is_empty() || response.starts_with(b"HTTP/1.1 408"));
    });
    pool.shutdown().unwrap();
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
            .to_ascii_lowercase()
            .contains("content-type: application/wasm")
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
fn provider_route_exposes_configured_names_and_url_values_and_empty_configuration() {
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
    assert_eq!(
        catalogue[0].endpoint,
        "https://user:secret-pass@example.com/private?key=secret-key"
    );
    assert_eq!(catalogue[1].id, "local");
    assert_eq!(catalogue[1].endpoint, "http://127.0.0.1:8545");
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
