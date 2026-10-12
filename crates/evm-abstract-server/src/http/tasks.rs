//! Axum task handlers delegate analysis operations to the existing worker pool.

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
    AnalyzeRequest, ApiError, ApiErrorCode, ErrorDetails, JobId, JobReply, ValidationErrorKind,
    ValidationFailure,
};

pub(super) async fn submit(
    State(state): State<Application>,
    input: Result<Json<AnalyzeRequest>, JsonRejection>,
) -> Response {
    let input = match input {
        Ok(Json(input)) => input,
        Err(_) => {
            return error_response(ApiError {
                code: ApiErrorCode::InvalidRequest,
                message: "invalid analysis JSON or Content-Type".into(),
                details: ErrorDetails::Validation(ValidationFailure {
                    kind: ValidationErrorKind::Json,
                    field: None,
                    value: None,
                }),
            });
        }
    };
    job_reply(
        tokio::task::spawn_blocking(move || state.jobs.submit(input)).await,
        202,
    )
}

pub(super) async fn status(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    job_reply(
        tokio::task::spawn_blocking(move || state.jobs.status(id)).await,
        200,
    )
}

pub(super) async fn cancel(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    job_reply(
        tokio::task::spawn_blocking(move || state.jobs.cancel(id)).await,
        200,
    )
}

pub(super) async fn result(
    State(state): State<Application>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match id(path) {
        Ok(id) => id,
        Err(error) => return error_response(error),
    };
    match tokio::task::spawn_blocking(move || {
        state.jobs.result(id).and_then(|report| {
            super::export::report(
                &report,
                &evm_abstract::analysis::control::Control::default(),
            )
        })
    })
    .await
    {
        Ok(Ok(body)) => response(200, "application/json; charset=utf-8", body),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(api_error(ApiErrorCode::Internal, "result worker failed")),
    }
}

fn id(path: Result<Path<String>, PathRejection>) -> Result<JobId, ApiError> {
    let Path(value) =
        path.map_err(|_| api_error(ApiErrorCode::InvalidRequest, "invalid task path"))?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(api_error(
            ApiErrorCode::InvalidRequest,
            "task ID must be an unsigned integer",
        ));
    }
    value
        .parse()
        .map(JobId)
        .map_err(|_| api_error(ApiErrorCode::InvalidRequest, "task ID exceeds its range"))
}

fn job_reply(
    result: Result<Result<evm_abstract_protocol::JobSnapshot, ApiError>, tokio::task::JoinError>,
    success: u16,
) -> Response {
    match result {
        Ok(result) => {
            let status = result
                .as_ref()
                .err()
                .map_or(success, |error| error_status(error.code));
            json_response(status, &JobReply { result })
        }
        Err(_) => error_response(api_error(
            ApiErrorCode::Internal,
            "task transport worker failed",
        )),
    }
}
