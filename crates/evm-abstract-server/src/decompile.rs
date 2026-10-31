//! Transport adapter for independent local recovery and traceable structuring.

use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    ApiError, ApiErrorCode, DecompileArtifact, DecompileConfig, DecompileRequest, ErrorDetails,
    ValidationErrorKind, ValidationFailure, decompile_model::Progress,
};
use std::sync::mpsc::SyncSender;

/// Bounded best-effort progress observations; they never block the worker.
#[derive(Clone)]
pub struct Observer {
    sender: SyncSender<Progress>,
}

impl Observer {
    pub(crate) fn new(sender: SyncSender<Progress>) -> Self {
        Self { sender }
    }

    /// Publish an actual core observation without waiting for a UI consumer.
    pub fn observe(&self, progress: Progress) {
        let _ = self.sender.try_send(progress);
    }
}

/// Validate transport shape before consuming a common scheduler slot.
pub(crate) fn validate(request: &DecompileRequest) -> Result<(), ApiError> {
    if request.bytecode.len() > evm_abstract_protocol::MAX_REQUEST_BYTES {
        return Err(ApiError {
            code: ApiErrorCode::RequestTooLarge,
            message: "runtime bytecode exceeds the transport request limit".into(),
            details: ErrorDetails::Validation(ValidationFailure {
                kind: ValidationErrorKind::Limits,
                field: Some("bytecode".into()),
                value: None,
            }),
        });
    }
    evm_bytecode::parse_hex(&request.bytecode).map_err(invalid_hex)?;
    // Selected recovery limits belong to core, which publishes an inspectable
    // refusal. Admission enforces only transport shape and its hard byte bound.
    Ok(())
}

/// Run local recovery directly from code; no world or Hybrid analysis is built.
pub(crate) fn run(
    request: DecompileRequest,
    control: &Control,
    observer: &Observer,
) -> Result<DecompileArtifact, ApiError> {
    validate(&request)?;
    let fork = match request.fork {
        evm_abstract_protocol::Fork::Cancun => evm_decompile::Fork::Cancun,
        evm_abstract_protocol::Fork::Prague => evm_decompile::Fork::Prague,
        evm_abstract_protocol::Fork::Osaka => evm_decompile::Fork::Osaka,
    };
    let bytes = evm_bytecode::parse_hex(&request.bytecode).map_err(invalid_hex)?;
    let input = evm_decompile::Input::new(bytes, fork);
    let config = DecompileConfig {
        strategy: request.strategy,
        limits: request.limits,
    };
    Ok(control.scope(|| {
        evm_decompile::decompile_with_progress(
            &input,
            &config,
            control.cancellation(),
            |progress| observer.observe(progress),
        )
    }))
}

fn invalid_hex(error: evm_bytecode::DecodeError) -> ApiError {
    ApiError {
        code: ApiErrorCode::InvalidBytecode,
        message: error.to_string(),
        details: ErrorDetails::Validation(ValidationFailure {
            kind: ValidationErrorKind::Bytecode,
            field: Some("bytecode".into()),
            value: None,
        }),
    }
}
