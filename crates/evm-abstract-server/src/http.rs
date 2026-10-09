//! Small HTTP/1.1 host for a local, same-origin workbench.
//!
//! Each connection carries one bounded request and is closed after its response.
//! Content-Length framing is mandatory for JSON; chunked requests, duplicate
//! framing headers and ambiguous paths are rejected. Static paths are checked
//! after canonicalization, including symlink resolution. No ambient RPC is used.

mod connections;
mod request;
mod tasks;

use evm_abstract_protocol::{
    API_PATH, ApiError, ApiErrorCode, ErrorDetails, JobReply, TransportErrorKind, TransportFailure,
};
use std::{
    fs,
    io::{self, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path},
    time::Duration,
};
use thiserror::Error;

/// Failure of the local host itself, distinct from a typed client error reply.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Socket or file I/O failed.
    #[error("web host I/O: {0}")]
    Io(io::Error),
    /// A typed response could not be encoded.
    #[error("response encoding: {0}")]
    Json(serde_json::Error),
    /// Analysis worker pool could not be constructed.
    #[error("analysis pool: {0}")]
    Pool(crate::jobs::PoolError),
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for ServerError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Serve bounded HTTP connections independently of the CPU analysis pool.
///
/// Individual client disconnects do not stop the listener. The application owns
/// binding and prints the selected address before entering this loop.
pub fn serve(listener: &TcpListener, assets: &Path) -> Result<(), ServerError> {
    serve_with_config(listener, assets, crate::jobs::Config::default())
}

/// Serve with explicit CPU worker, queue and retained-result limits.
pub fn serve_with_config(
    listener: &TcpListener,
    assets: &Path,
    config: crate::jobs::Config,
) -> Result<(), ServerError> {
    let jobs = crate::jobs::Pool::new(config).map_err(ServerError::Pool)?;
    let mut connections = connections::Pool::new(assets, jobs)?;
    for stream in listener.incoming() {
        let stream = stream?;
        connections.accept(stream)?;
    }
    Ok(())
}

/// Serve one accepted connection; exposed for real transport integration tests.
pub fn serve_connection(
    mut stream: TcpStream,
    assets: &Path,
    jobs: &crate::jobs::Pool,
) -> Result<(), ServerError> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request = match request::read(&mut stream) {
        Ok(request) => request,
        Err(error) => return error_reply(&mut stream, error),
    };
    if request.path == API_PATH || request.path.starts_with(&format!("{API_PATH}/")) {
        return tasks::serve(&mut stream, request, jobs);
    }
    if request.method != "GET" {
        return error_reply(
            &mut stream,
            api_error(ApiErrorCode::MethodNotAllowed, "static assets accept GET"),
        );
    }
    let path = request.path.split('?').next().unwrap_or(&request.path);
    let relative = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if relative.is_empty()
        || relative.contains(['%', '\\', ':', '\0'])
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return error_reply(
            &mut stream,
            api_error(ApiErrorCode::NotFound, "asset not found"),
        );
    }
    let root = match assets.canonicalize() {
        Ok(root) => root,
        Err(_) => {
            return error_reply(
                &mut stream,
                api_error(
                    ApiErrorCode::NotFound,
                    "web assets are unavailable; build the web frontend first",
                ),
            );
        }
    };
    let asset = match root.join(relative).canonicalize() {
        Ok(asset) if asset.starts_with(&root) && asset.is_file() => asset,
        _ => {
            return error_reply(
                &mut stream,
                api_error(ApiErrorCode::NotFound, "asset not found"),
            );
        }
    };
    let metadata = asset.metadata()?;
    if metadata.len() > 64 * 1024 * 1024 {
        return error_reply(
            &mut stream,
            api_error(ApiErrorCode::NotFound, "asset exceeds host size bound"),
        );
    }
    let body = fs::read(&asset)?;
    let mime = match asset.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };
    respond(&mut stream, 200, mime, &body)
}

fn api_error(code: ApiErrorCode, message: impl Into<String>) -> ApiError {
    let kind = match code {
        ApiErrorCode::MethodNotAllowed => TransportErrorKind::Method,
        ApiErrorCode::NotFound => TransportErrorKind::Route,
        ApiErrorCode::RequestTooLarge => TransportErrorKind::Size,
        ApiErrorCode::Internal | ApiErrorCode::Transport => TransportErrorKind::Network,
        _ => TransportErrorKind::HttpRequest,
    };
    ApiError {
        code,
        message: message.into(),
        details: ErrorDetails::Transport(TransportFailure {
            kind,
            status: None,
            bytes: None,
        }),
    }
}

fn error_status(code: ApiErrorCode) -> u16 {
    match code {
        ApiErrorCode::InvalidRequest
        | ApiErrorCode::InvalidBytecode
        | ApiErrorCode::InvalidLimits => 400,
        ApiErrorCode::RequestTooLarge => 413,
        ApiErrorCode::Internal => 500,
        ApiErrorCode::NotFound => 404,
        ApiErrorCode::MethodNotAllowed => 405,
        ApiErrorCode::Rpc => 502,
        ApiErrorCode::TaskNotFound => 404,
        ApiErrorCode::TaskNotReady => 409,
        ApiErrorCode::QueueFull => 429,
        ApiErrorCode::Cancelled => 410,
        ApiErrorCode::WorkerFailed => 503,
        ApiErrorCode::Transport => 502,
    }
}

fn error_reply(stream: &mut TcpStream, error: ApiError) -> Result<(), ServerError> {
    let status = error_status(error.code);
    let body = serde_json::to_vec(&JobReply { result: Err(error) })?;
    respond(stream, status, "application/json; charset=utf-8", &body)
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    body: &[u8],
) -> Result<(), ServerError> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}
