//! Preserve native admission discriminants and their complete contextual facts.
mod session;
pub(super) use session::error as session_error;

use super::value;
use evm_abstract::{
    analysis::{ConfigError, cooperation::CooperationError},
    bytecode::DecodeError,
    domain::DomainSpecError,
    world::environment::EnvironmentError,
};
use evm_abstract_protocol as api;
fn environment_details(source: EnvironmentError) -> api::EnvironmentFailure {
    match source {
        EnvironmentError::TableLimit => api::EnvironmentFailure::TableLimit,
        EnvironmentError::BlobIndex { index, count } => {
            api::EnvironmentFailure::BlobIndex(api::BlobIndexFailure {
                index: value::word(index),
                count: value::word(count),
            })
        }
        EnvironmentError::Destination { expected, observed } => {
            api::EnvironmentFailure::Destination(api::DestinationFailure {
                expected: expected.to_string(),
                observed: observed.to_string(),
            })
        }
    }
}
pub(super) fn environment(source: EnvironmentError) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::InvalidRequest,
        message: source.to_string(),
        details: api::ErrorDetails::Environment(environment_details(source)),
    }
}
pub(super) fn configuration(source: ConfigError) -> api::ApiError {
    let message = source.to_string();
    let detail = match source {
        ConfigError::DomainSpec(DomainSpecError::InvalidRelationLimits) => {
            api::ConfigurationFailure::Relations
        }
        ConfigError::Environment(error) => return environment(error),
        ConfigError::Budget => api::ConfigurationFailure::Budget,
    };
    api::ApiError {
        code: api::ApiErrorCode::InvalidLimits,
        message,
        details: api::ErrorDetails::Configuration(detail),
    }
}
/// The server installs NoExtension, so external component errors are impossible.
/// Infrastructure categories remain typed instead of becoming transport failures.
pub(super) fn execution(source: CooperationError<std::convert::Infallible>) -> api::ApiError {
    let message = source.to_string();
    let kind = match source {
        CooperationError::Config(error) => return configuration(error),
        CooperationError::Cancelled => return super::cancelled(),
        CooperationError::Extension(error) => match error {},
        CooperationError::Protocol(_) => api::ExecutionFailureKind::Protocol,
        CooperationError::Initialization(_) => api::ExecutionFailureKind::Initialization,
        CooperationError::Refinement(_) => api::ExecutionFailureKind::Refinement,
        CooperationError::Unfinished(_) => api::ExecutionFailureKind::Unfinished,
    };
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message,
        details: api::ErrorDetails::Execution(kind),
    }
}

pub(super) fn bytecode(source: DecodeError) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::InvalidBytecode,
        message: source.to_string(),
        details: api::ErrorDetails::Bytecode(super::rpc::causes::code(
            &evm_abstract::world::rpc::CodeFailure::from(&source),
        )),
    }
}
pub(super) fn world(source: evm_abstract::world::WorldError) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::InvalidRequest,
        message: source.to_string(),
        details: api::ErrorDetails::World(super::rpc::causes::world(
            &evm_abstract::world::rpc::WorldFailure::from(&source),
        )),
    }
}

pub(crate) fn fingerprint(source: evm_abstract::world::FingerprintError) -> api::ApiError {
    use evm_abstract::world::FingerprintError as F;
    let message = source.to_string();
    let (kind, line, column) = match source {
        F::Cancelled => (api::FingerprintFailureKind::Cancelled, None, None),
        F::Encoding(error) => (
            api::FingerprintFailureKind::Encoding,
            Some(error.line()),
            Some(error.column()),
        ),
        F::LengthOverflow => (api::FingerprintFailureKind::LengthOverflow, None, None),
    };
    api::ApiError {
        code: if kind == api::FingerprintFailureKind::Cancelled {
            api::ApiErrorCode::Cancelled
        } else {
            api::ApiErrorCode::Export
        },
        message,
        details: api::ErrorDetails::Fingerprint(api::FingerprintFailure { kind, line, column }),
    }
}

#[cfg(test)]
mod tests;
