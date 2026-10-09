//! Typed task routes; analysis is always delegated to the native worker pool.

use super::{ServerError, api_error, error_reply, error_status, request::Request, respond};
use crate::jobs::Pool;
use evm_abstract_protocol::{
    API_PATH, AnalysisReport, AnalyzeRequest, ApiError, ApiErrorCode, ErrorDetails, JobId,
    JobReply, ValidationErrorKind, ValidationFailure,
};
use std::net::TcpStream;

enum Route {
    Submit,
    Status(JobId),
    Result(JobId),
}

fn route(path: &str) -> Result<Route, ApiError> {
    if path == API_PATH {
        return Ok(Route::Submit);
    }
    let Some(suffix) = path.strip_prefix(&format!("{API_PATH}/")) else {
        return Err(api_error(ApiErrorCode::NotFound, "task route not found"));
    };
    let mut parts = suffix.split('/');
    let id = parts.next().unwrap_or_default();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(api_error(
            ApiErrorCode::InvalidRequest,
            "task ID must be an unsigned integer",
        ));
    }
    let id = JobId(
        id.parse()
            .map_err(|_| api_error(ApiErrorCode::InvalidRequest, "task ID exceeds its range"))?,
    );
    match (parts.next(), parts.next()) {
        (None, None) => Ok(Route::Status(id)),
        (Some("result"), None) => Ok(Route::Result(id)),
        _ => Err(api_error(ApiErrorCode::NotFound, "task route not found")),
    }
}

pub(super) fn serve(
    stream: &mut TcpStream,
    request: Request,
    jobs: &Pool,
) -> Result<(), ServerError> {
    let route = match route(&request.path) {
        Ok(route) => route,
        Err(error) => return error_reply(stream, error),
    };
    match (request.method.as_str(), route) {
        ("POST", Route::Submit) => {
            let input: AnalyzeRequest = match serde_json::from_slice(&request.body) {
                Ok(input) => input,
                Err(error) => {
                    return error_reply(
                        stream,
                        ApiError {
                            code: ApiErrorCode::InvalidRequest,
                            message: format!("invalid analysis JSON: {error}"),
                            details: ErrorDetails::Validation(ValidationFailure {
                                kind: ValidationErrorKind::Json,
                                field: None,
                                value: None,
                            }),
                        },
                    );
                }
            };
            let result = jobs.submit(input);
            let status = result
                .as_ref()
                .err()
                .map_or(202, |error| error_status(error.code));
            json(stream, status, &JobReply { result })
        }
        ("GET", Route::Status(id)) => {
            let result = jobs.status(id);
            let status = result
                .as_ref()
                .err()
                .map_or(200, |error| error_status(error.code));
            json(stream, status, &JobReply { result })
        }
        ("DELETE", Route::Status(id)) => {
            let result = jobs.cancel(id);
            let status = result
                .as_ref()
                .err()
                .map_or(200, |error| error_status(error.code));
            json(stream, status, &JobReply { result })
        }
        ("GET", Route::Result(id)) => {
            let report = match jobs.result(id) {
                Ok(report) => report,
                Err(error) => return error_reply(stream, error),
            };
            json(
                stream,
                200,
                &ReportReply {
                    result: Ok(&report),
                },
            )
        }
        _ => error_reply(
            stream,
            api_error(
                ApiErrorCode::MethodNotAllowed,
                "unsupported method for this task route",
            ),
        ),
    }
}

#[derive(serde::Serialize)]
struct ReportReply<'a> {
    result: Result<&'a AnalysisReport, &'a ApiError>,
}

fn json<T: serde::Serialize>(
    stream: &mut TcpStream,
    status: u16,
    reply: &T,
) -> Result<(), ServerError> {
    let body = serde_json::to_vec(reply)?;
    respond(stream, status, "application/json; charset=utf-8", &body)
}
