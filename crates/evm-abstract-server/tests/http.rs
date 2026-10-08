//! Actual TCP tests cover framing, typed API responses and the static asset host.

use evm_abstract_protocol::{AnalyzeReply, AnalyzeRequest, ApiErrorCode};
use evm_abstract_server::http::serve_connection;
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let assets = assets.to_owned();
    thread::scope(|scope| {
        // Scope joins and propagates server panics without exposing std's
        // dynamically typed panic payload at this crate's typed boundary.
        scope.spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &assets).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
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
    let input = AnalyzeRequest::default();
    let body = serde_json::to_vec(&input).unwrap();
    let mut request = format!("POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nOrigin: http://localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    request.extend(body);
    let (header, reply) = decode(&exchange(&request, &assets.0));
    assert!(header.starts_with("HTTP/1.1 200"));
    let report = reply.result.unwrap();
    assert_eq!(report.disassembly[0].instructions[2].name, "ADD");
    assert_eq!(report.ssa.blocks[0].instructions[2].operands.len(), 2);
}

#[test]
fn framing_errors_and_limits_return_typed_envelopes() {
    let assets = Assets::new();
    for (request, code, status) in [
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nContent-Length: 262145\r\nContent-Type: application/json\r\n\r\n",
            ApiErrorCode::RequestTooLarge,
            "413",
        ),
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 1\r\n\r\n{",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 10\r\n\r\n{}",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "POST /api/analyze HTTP/1.1\r\nHost: localhost\r\nOrigin: https://other.example\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            ApiErrorCode::InvalidRequest,
            "400",
        ),
        (
            "GET /api/analyze HTTP/1.1\r\nHost: localhost\r\n\r\n",
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
