//! Resolve code and snapshot provenance without constructing an analysis world.
mod rpc;
use super::cache::{Cache, Error};
use evm_abstract_protocol::{
    self as api, ApiError, ApiErrorCode, DecompileAddressSource, DecompileOpenRequest,
    DecompileSettings, DecompileSourceInput, ErrorDetails, ValidationErrorKind, ValidationFailure,
};
use std::sync::Arc;

pub(crate) async fn resolve(
    open: DecompileOpenRequest,
    providers: &crate::rpc_providers::Registry,
    cache: &Arc<Cache>,
) -> Result<(Vec<u8>, DecompileSettings, Option<DecompileAddressSource>), ApiError> {
    match open.source {
        DecompileSourceInput::Bytecode(code) => {
            if code.len() > api::MAX_REQUEST_BYTES {
                return Err(invalid(
                    "bytecode",
                    "runtime bytecode exceeds the request limit",
                ));
            }
            let bytes = evm_bytecode::parse_hex(&code).map_err(super::invalid_hex)?;
            Ok((bytes, open.settings.unwrap_or_default(), None))
        }
        DecompileSourceInput::Sha256(source) => {
            if source.config_sha256.is_some() && open.settings.is_some() {
                return Err(invalid(
                    "settings",
                    "saved config and explicit settings cannot be combined",
                ));
            }
            let cache = Arc::clone(cache);
            let (bytes, saved) = tokio::task::spawn_blocking(move || {
                cache.source(&source.sha256, source.config_sha256.as_deref())
            })
            .await
            .map_err(|_| invalid("source", "cached source worker failed"))?
            .map_err(cache_error)?;
            Ok((bytes, saved.or(open.settings).unwrap_or_default(), None))
        }
        DecompileSourceInput::Address(source) => {
            let (bytes, origin) = rpc::resolve(source, providers).await?;
            Ok((bytes, open.settings.unwrap_or_default(), Some(origin)))
        }
    }
}
pub(crate) fn cache_error(error: Error) -> ApiError {
    let code = match error {
        Error::Missing => ApiErrorCode::NotFound,
        Error::Hash => ApiErrorCode::InvalidRequest,
        _ => ApiErrorCode::Internal,
    };
    ApiError {
        code,
        message: error.to_string(),
        details: ErrorDetails::Validation(ValidationFailure {
            kind: ValidationErrorKind::Field,
            field: Some("cache".into()),
            value: None,
        }),
    }
}
pub(super) fn invalid(field: &str, message: &str) -> ApiError {
    ApiError {
        code: ApiErrorCode::InvalidRequest,
        message: message.into(),
        details: ErrorDetails::Validation(ValidationFailure {
            kind: ValidationErrorKind::Field,
            field: Some(field.into()),
            value: None,
        }),
    }
}
