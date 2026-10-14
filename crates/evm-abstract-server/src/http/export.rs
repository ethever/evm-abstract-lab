//! Encoding stays bounded after graph projection and never edits cached facts.
use evm_abstract::{
    analysis::control::Control,
    render::json::{self, JsonError, JsonLimits},
};
use evm_abstract_protocol as api;

#[derive(serde::Serialize)]
struct Reply<'a> {
    result: Result<&'a api::AnalysisReport, &'a api::ApiError>,
}

pub(super) fn report(
    report: &api::AnalysisReport,
    control: &Control,
) -> Result<Vec<u8>, api::ApiError> {
    json::compact_with_control(
        &Reply { result: Ok(report) },
        JsonLimits {
            max_bytes: report.metadata.export_limits.max_output_bytes,
        },
        control,
    )
    .map(String::into_bytes)
    .map_err(|error| failure(report, error))
}
fn failure(report: &api::AnalysisReport, error: JsonError) -> api::ApiError {
    let message = error.to_string();
    let (kind, required, written) = match error {
        JsonError::Bytes {
            required, written, ..
        } => (api::ExportFailureKind::OutputBytes, required, written),
        JsonError::Cancelled { written } => (api::ExportFailureKind::Cancelled, written, written),
        JsonError::Serialization(_) => {
            return super::api_error(api::ApiErrorCode::Internal, "typed report encoding failed");
        }
        JsonError::Fingerprint(error) => return crate::analyze::fingerprint_error(error),
    };
    api::ApiError {
        code: if kind == api::ExportFailureKind::Cancelled {
            api::ApiErrorCode::Cancelled
        } else {
            api::ApiErrorCode::Export
        },
        message,
        details: api::ErrorDetails::Export(Box::new(api::ExportFailure {
            kind,
            required: required as u64,
            limits: report.metadata.export_limits,
            usage: report.metadata.export_usage,
            written: Some(written),
        })),
    }
}

#[cfg(test)]
mod tests;
