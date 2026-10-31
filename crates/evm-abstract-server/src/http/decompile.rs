//! Typed endpoints for local decompiler tasks in the shared bounded scheduler.

use super::{Application, api_error, error_response, error_status, json_response, response};
use axum::{
    Json,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    response::Response,
};
use evm_abstract_protocol::{
    ApiError, ApiErrorCode, DecompileArtifact, DecompileJobReply, DecompileJobSnapshot,
    DecompileRequest, ErrorDetails, JobId, PublicationVersion, ValidationErrorKind,
    ValidationFailure,
};

pub(super) async fn submit(
    State(state): State<Application>,
    request: Result<Json<DecompileRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(_) => {
            return error_response(ApiError {
                code: ApiErrorCode::InvalidRequest,
                message: "invalid decompiler JSON or Content-Type".into(),
                details: ErrorDetails::Validation(ValidationFailure {
                    kind: ValidationErrorKind::Json,
                    field: None,
                    value: None,
                }),
            });
        }
    };
    reply(
        tokio::task::spawn_blocking(move || state.jobs.submit_decompile(request)).await,
        202,
    )
}

pub(super) async fn status(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match super::tasks::id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    reply(
        tokio::task::spawn_blocking(move || state.jobs.decompile_status(id)).await,
        200,
    )
}

pub(super) async fn cancel(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match super::tasks::id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    reply(
        tokio::task::spawn_blocking(move || state.jobs.cancel_decompile(id)).await,
        200,
    )
}

pub(super) async fn result(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match super::tasks::id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    match tokio::task::spawn_blocking(move || {
        let result = state.jobs.decompile_result(id)?;
        #[derive(serde::Serialize)]
        struct BorrowedResult<'a> {
            id: JobId,
            publication: PublicationVersion,
            artifact: &'a DecompileArtifact,
        }
        #[derive(serde::Serialize)]
        struct Envelope<'a> {
            result: Result<BorrowedResult<'a>, ApiError>,
        }
        serde_json::to_vec(&Envelope {
            result: Ok(BorrowedResult {
                id: result.id,
                publication: result.publication,
                artifact: &result.artifact,
            }),
        })
        .map_err(|_| api_error(ApiErrorCode::Internal, "decompiler result encoding failed"))
    })
    .await
    {
        Ok(Ok(body)) => response(200, "application/json; charset=utf-8", body),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(api_error(
            ApiErrorCode::Internal,
            "decompiler result worker failed",
        )),
    }
}

fn reply(
    result: Result<Result<DecompileJobSnapshot, ApiError>, tokio::task::JoinError>,
    success: u16,
) -> Response {
    match result {
        Ok(result) => {
            let status = result
                .as_ref()
                .err()
                .map_or(success, |error| error_status(error.code));
            json_response(status, &DecompileJobReply { result })
        }
        Err(_) => error_response(api_error(
            ApiErrorCode::Internal,
            "decompiler task transport worker failed",
        )),
    }
}
