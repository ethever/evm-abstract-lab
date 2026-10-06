//! Concrete errors at the CLI's acquisition, analysis and output boundary.

use evm_abstract::{
    analysis::{ConfigError, RpcAnalysisError},
    bytecode::DecodeError,
    ssa::SsaError,
    world::rpc::RpcError,
};
use thiserror::Error;

use crate::{evm::EnvironmentInputError, world::InputError};

#[derive(Debug, Error)]
pub(crate) enum CliError {
    #[error("{0}")]
    Io(std::io::Error),
    #[error("{0}")]
    Decode(DecodeError),
    #[error("{0}")]
    Config(ConfigError),
    #[error("{0}")]
    Input(InputError),
    #[error("{0}")]
    Environment(#[source] EnvironmentInputError),
    #[error("{0}")]
    Rpc(RpcError),
    #[error("{0}")]
    Ssa(SsaError),
    #[error("{0}")]
    Json(serde_json::Error),
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DecodeError> for CliError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<InputError> for CliError {
    fn from(error: InputError) -> Self {
        Self::Input(error)
    }
}

impl From<RpcError> for CliError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

impl From<RpcAnalysisError> for CliError {
    fn from(error: RpcAnalysisError) -> Self {
        match error {
            RpcAnalysisError::Config(error) => Self::Config(error),
            RpcAnalysisError::Rpc(error) => Self::Rpc(error),
        }
    }
}

impl From<SsaError> for CliError {
    fn from(error: SsaError) -> Self {
        Self::Ssa(error)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<EnvironmentInputError> for CliError {
    fn from(error: EnvironmentInputError) -> Self {
        Self::Environment(error)
    }
}
