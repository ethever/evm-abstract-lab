//! Encoding stays cancellable after graph projection and never edits cached facts.
use evm_abstract::{
    analysis::control::Control,
    render::json::{self, JsonError},
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
    json::compact_with_control(&Reply { result: Ok(report) }, control)
        .map(String::into_bytes)
        .map_err(|error| failure(report, error))
}
fn failure(report: &api::AnalysisReport, error: JsonError) -> api::ApiError {
    let Some(metadata) = report.selected_graph().map(|graph| &graph.metadata) else {
        return super::api_error(api::ApiErrorCode::Internal, "report has no selected graph");
    };
    let message = error.to_string();
    let written = match error {
        JsonError::Cancelled { written } => written,
        JsonError::Serialization(_) => {
            return super::api_error(api::ApiErrorCode::Internal, "typed report encoding failed");
        }
        JsonError::Fingerprint(error) => return crate::analyze::fingerprint_error(error),
    };
    api::ApiError {
        code: api::ApiErrorCode::Cancelled,
        message,
        details: api::ErrorDetails::Export(Box::new(api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            required: written as u64,
            limits: metadata.export_limits,
            usage: metadata.export_usage,
            written: Some(written),
        })),
    }
}

#[cfg(test)]
mod tests;
