//! Axum application and bounded Hyper transport for the same-origin workbench.
//! Parsing, framing, keepalive and response transport belong to the framework.
//! Task admission, origin policy and canonical asset paths are application rules.

mod connection;
mod export;
mod tasks;

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, FromRequest, Request, State},
    http::{Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use evm_abstract_protocol::{
    API_PATH, ApiError, ApiErrorCode, ErrorDetails, JobReply, MAX_REQUEST_BYTES,
    RPC_PROVIDERS_PATH, RpcProvidersReply, TransportErrorKind, TransportFailure,
};
use std::{
    future::Future,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::JoinSet,
};
use tower_http::compression::CompressionLayer;

const MAX_CONNECTIONS: usize = 32;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Failure to start or run the host, distinct from a typed client error reply.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Socket or runtime I/O failed.
    #[error("web host I/O: {0}")]
    Io(io::Error),
    /// Analysis worker pool could not be constructed.
    #[error("analysis pool: {0}")]
    Pool(crate::jobs::PoolError),
    /// Backend RPC configuration could not be read or validated.
    #[error("RPC configuration: {0}")]
    RpcConfig(crate::rpc_providers::ConfigError),
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
struct Application {
    assets: PathBuf,
    jobs: crate::jobs::Pool,
}

/// Build the framework router around an existing analysis pool.
pub fn router(assets: PathBuf, jobs: crate::jobs::Pool) -> Router {
    Router::new()
        .route(RPC_PROVIDERS_PATH, get(providers))
        .route(API_PATH, post(tasks::submit))
        .route("/api/tasks/{id}", get(tasks::status).delete(tasks::cancel))
        .route("/api/tasks/{id}/result", get(tasks::result))
        .fallback(asset)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn(request_policy))
        // Negotiate and stream response compression, including policy errors and assets.
        // Equal client weights prefer zstd, then Brotli, then gzip.
        .layer(CompressionLayer::new())
        .with_state(Application { assets, jobs })
}

/// Serve the application with default analysis scheduling bounds.
pub async fn serve(listener: TcpListener, assets: &Path) -> Result<(), ServerError> {
    serve_with_config(listener, assets, crate::jobs::Config::default()).await
}

/// Serve with explicit CPU worker, queue and retained-result limits.
pub async fn serve_with_config(
    listener: TcpListener,
    assets: &Path,
    config: crate::jobs::Config,
) -> Result<(), ServerError> {
    serve_with_providers(
        listener,
        assets,
        config,
        crate::rpc_providers::Registry::default(),
    )
    .await
}

/// Serve with a backend-owned RPC registry shared by admission and workers.
pub async fn serve_with_providers(
    listener: TcpListener,
    assets: &Path,
    config: crate::jobs::Config,
    providers: crate::rpc_providers::Registry,
) -> Result<(), ServerError> {
    serve_with_postprocess_work(
        listener,
        assets,
        config,
        providers,
        crate::analyze::DEFAULT_POSTPROCESS_WORK,
    )
    .await
}

/// Serve with one declared postprocessing allowance per job, shared by all
/// publications. Required minimum is funded before optional rich presentation.
pub async fn serve_with_postprocess_work(
    listener: TcpListener,
    assets: &Path,
    config: crate::jobs::Config,
    providers: crate::rpc_providers::Registry,
    postprocess_work: usize,
) -> Result<(), ServerError> {
    let jobs = crate::jobs::Pool::with_postprocess_work(config, providers, postprocess_work)
        .map_err(ServerError::Pool)?;
    serve_with_shutdown(
        listener,
        router(assets.to_owned(), jobs),
        std::future::pending(),
    )
    .await
}

/// Run the real Hyper server until shutdown, then drain accepted connections.
/// At most 32 connections are active, including clients still sending headers.
pub async fn serve_with_shutdown(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
) -> Result<(), ServerError> {
    let capacity = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let (stopping, _) = watch::channel(());
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        // Reap completed tasks while accepting so their handles stay bounded.
        while let Some(result) = connections.try_join_next() {
            report_connection_task(result);
        }
        let permit = tokio::select! {
            _ = &mut shutdown => break Ok(()),
            permit = Arc::clone(&capacity).acquire_owned() => permit.expect("host owns the semaphore"),
        };
        let (stream, peer) = tokio::select! {
            _ = &mut shutdown => break Ok(()),
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => break Err(ServerError::Io(error)),
            },
        };
        // Subscribe before spawning: shutdown can arrive before the task's first poll.
        let stopping = stopping.subscribe();
        let app = app.clone();
        connections.spawn(async move {
            let _permit = permit;
            if let Err(error) = connection::serve(stream, app, stopping).await {
                eprintln!("HTTP connection from {peer}: {error}");
            }
        });
    };
    let _ = stopping.send(());
    while let Some(result) = connections.join_next().await {
        report_connection_task(result);
    }
    result
}

fn report_connection_task(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        eprintln!("HTTP connection task: {error}");
    }
}

async fn request_policy(mut request: Request, next: Next) -> Response {
    // This is a local origin server, rather than an HTTP forward proxy. Keep
    // routing and origin checks on one unambiguous target authority.
    if request.uri().scheme().is_some()
        || request.uri().authority().is_some()
        || !request.uri().path().starts_with('/')
        || request.uri().path().starts_with("//")
    {
        return error_response(api_error(
            ApiErrorCode::InvalidRequest,
            "request target must be an origin-form path",
        ));
    }
    for name in [header::HOST, header::ORIGIN, header::CONTENT_TYPE] {
        if request.headers().get_all(&name).iter().count() > 1 {
            return error_response(api_error(
                ApiErrorCode::InvalidRequest,
                "duplicate request policy header",
            ));
        }
    }
    if request.version() == axum::http::Version::HTTP_11
        && request
            .headers()
            .get(header::HOST)
            .is_none_or(|host| host.as_bytes().is_empty())
    {
        return error_response(api_error(
            ApiErrorCode::InvalidRequest,
            "HTTP/1.1 requires Host",
        ));
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let same_origin = request
            .headers()
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            .is_some_and(|host| origin.as_bytes() == format!("http://{host}").as_bytes());
        if !same_origin {
            return error_response(api_error(
                ApiErrorCode::InvalidRequest,
                "analysis host only accepts same-origin requests",
            ));
        }
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_REQUEST_BYTES as u64)
    {
        return error_response(api_error(
            ApiErrorCode::RequestTooLarge,
            "request body exceeds 262144 bytes",
        ));
    }
    DefaultBodyLimit::max(MAX_REQUEST_BYTES).apply(&mut request);
    let (parts, body) = request.into_parts();
    let body = tokio::time::timeout(
        REQUEST_TIMEOUT,
        Bytes::from_request(Request::from_parts(parts.clone(), body), &()),
    )
    .await;
    match body {
        Ok(Ok(body)) => next.run(Request::from_parts(parts, Body::from(body))).await,
        Ok(Err(error)) => error_response(api_error(
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                ApiErrorCode::RequestTooLarge
            } else {
                ApiErrorCode::InvalidRequest
            },
            "could not read bounded request body",
        )),
        Err(_) => error_response(api_error(
            ApiErrorCode::InvalidRequest,
            "request body read timed out",
        )),
    }
}

async fn providers(State(state): State<Application>) -> Response {
    json_response(
        200,
        &RpcProvidersReply {
            result: Ok(state.jobs.rpc_providers()),
        },
    )
}

async fn method_not_allowed() -> Response {
    error_response(api_error(
        ApiErrorCode::MethodNotAllowed,
        "unsupported method for this route",
    ))
}

async fn asset(State(state): State<Application>, method: Method, uri: Uri) -> Response {
    if !matches!(method, Method::GET | Method::HEAD) {
        return method_not_allowed().await;
    }
    if uri.path().starts_with("/api/") {
        return error_response(api_error(ApiErrorCode::NotFound, "API route not found"));
    }
    let path = uri.path().to_owned();
    match tokio::task::spawn_blocking(move || load_asset(&state.assets, &path)).await {
        Ok(Ok((mime, body))) => response(200, mime, body),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(api_error(ApiErrorCode::Internal, "asset worker failed")),
    }
}

fn load_asset(assets: &Path, path: &str) -> Result<(&'static str, Vec<u8>), ApiError> {
    let relative = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if path.starts_with("//")
        || relative.is_empty()
        || relative.contains(['%', '\\', ':', '\0'])
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(api_error(ApiErrorCode::NotFound, "asset not found"));
    }
    let root = assets.canonicalize().map_err(|_| {
        api_error(
            ApiErrorCode::NotFound,
            "web assets are unavailable; build the web frontend first",
        )
    })?;
    let asset = root
        .join(relative)
        .canonicalize()
        .map_err(|_| api_error(ApiErrorCode::NotFound, "asset not found"))?;
    if !asset.starts_with(&root) || !asset.is_file() {
        return Err(api_error(ApiErrorCode::NotFound, "asset not found"));
    }
    let file = std::fs::File::open(&asset)
        .map_err(|_| api_error(ApiErrorCode::NotFound, "asset not found"))?;
    if file
        .metadata()
        .map_err(|_| api_error(ApiErrorCode::NotFound, "asset not found"))?
        .len()
        > MAX_ASSET_BYTES
    {
        return Err(api_error(
            ApiErrorCode::NotFound,
            "asset exceeds host size bound",
        ));
    }
    let mut body = Vec::new();
    file.take(MAX_ASSET_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| api_error(ApiErrorCode::Internal, "asset read failed"))?;
    if body.len() as u64 > MAX_ASSET_BYTES {
        return Err(api_error(
            ApiErrorCode::NotFound,
            "asset exceeds host size bound",
        ));
    }
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
    Ok((mime, body))
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
        ApiErrorCode::NotFound | ApiErrorCode::TaskNotFound => 404,
        ApiErrorCode::MethodNotAllowed => 405,
        ApiErrorCode::Rpc | ApiErrorCode::Transport => 502,
        ApiErrorCode::TaskNotReady => 409,
        ApiErrorCode::QueueFull => 429,
        ApiErrorCode::Cancelled => 410,
        ApiErrorCode::WorkerFailed => 503,
        ApiErrorCode::Export => 422,
    }
}

fn error_response(error: ApiError) -> Response {
    let status =
        StatusCode::from_u16(error_status(error.code)).expect("constant status codes are valid");
    let mut response = (status, axum::Json(JobReply { result: Err(error) })).into_response();
    policy_headers(&mut response);
    response
}

fn json_response<T: serde::Serialize>(status: u16, value: &T) -> Response {
    match evm_abstract::render::json::compact(value) {
        Ok(body) => response(status, "application/json; charset=utf-8", body.into_bytes()),
        Err(_) => error_response(api_error(
            ApiErrorCode::Internal,
            "typed response encoding failed",
        )),
    }
}

fn response(status: u16, mime: &'static str, body: Vec<u8>) -> Response {
    let mut response = (
        StatusCode::from_u16(status).expect("constant status codes are valid"),
        body,
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(mime),
    );
    policy_headers(&mut response);
    response
}

fn policy_headers(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
}
