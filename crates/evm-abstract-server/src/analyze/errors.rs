//! Preserve native admission discriminants and their complete contextual facts.
use super::value;
use evm_abstract::{
    analysis::ConfigError, bytecode::DecodeError, world::environment::EnvironmentError,
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
        ConfigError::Environment(error) => {
            api::ConfigurationFailure::Environment(environment_details(error))
        }
        ConfigError::Constants => api::ConfigurationFailure::Constants,
        ConfigError::Budget => api::ConfigurationFailure::Budget,
        ConfigError::Facts => api::ConfigurationFailure::Facts,
        ConfigError::Relations => api::ConfigurationFailure::Relations,
    };
    api::ApiError {
        code: api::ApiErrorCode::InvalidLimits,
        message,
        details: api::ErrorDetails::Configuration(detail),
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
