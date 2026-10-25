use super::{InitializationError, RefinementError};
use crate::analysis::{ConfigError, MachineError};
use analysis_kernel::KernelError;

/// A joint EVM/component run failed before publishing a native result.
/// Component errors keep their concrete type; infrastructure and invalid facts
/// never become a proof of unreachable EVM behavior.
#[derive(Debug, thiserror::Error)]
pub enum CooperationError<E> {
    /// Invalid execution configuration or environment.
    #[error("{0}")]
    Config(ConfigError),
    /// The caller did not register support for the selected semantic view.
    #[error("extension accepts {supported:?} inputs; requested {requested:?} execution")]
    UnsupportedView {
        /// Explicit immutable input view contract of the assembly.
        supported: super::InputViews,
        /// Requested execution interpretation.
        requested: crate::analysis::RuntimeExecution,
    },
    /// Failure of the independently authored analyzer/reduction bundle.
    #[error("external analyzer failed: {0}")]
    Extension(E),
    /// Shared scope, dependency, identity or resource protocol failure.
    #[error("cooperation protocol failed: {0}")]
    Protocol(KernelError<MachineError>),
    /// An immutable input universe could not be seeded completely.
    #[error("{0}")]
    Initialization(InitializationError<MachineError>),
    /// Invalid local facts or required refinement work refused by the root meter.
    #[error("{0}")]
    Refinement(RefinementError),
    /// Mandatory external work or a required feedback answer has not settled.
    /// Pending is not numerical Bottom or a completed execution relation.
    #[error("cooperation still has unfinished obligations: {0:?}")]
    Unfinished(analysis_kernel::Closure),
    /// Application cancellation; no partial report is returned.
    #[error("analysis cancelled")]
    Cancelled,
}

impl<E> From<ConfigError> for CooperationError<E> {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl<E> From<KernelError<MachineError>> for CooperationError<E> {
    fn from(error: KernelError<MachineError>) -> Self {
        Self::Protocol(error)
    }
}

impl<E> From<InitializationError<MachineError>> for CooperationError<E> {
    fn from(error: InitializationError<MachineError>) -> Self {
        Self::Initialization(error)
    }
}

impl<E> From<RefinementError> for CooperationError<E> {
    fn from(error: RefinementError) -> Self {
        Self::Refinement(error)
    }
}

impl<E> CooperationError<E> {
    /// Change only the concrete extension error at an explicit phase boundary.
    pub(crate) fn map_extension<F>(self, map: impl FnOnce(E) -> F) -> CooperationError<F> {
        match self {
            Self::Config(v) => CooperationError::Config(v),
            Self::UnsupportedView {
                supported,
                requested,
            } => CooperationError::UnsupportedView {
                supported,
                requested,
            },
            Self::Extension(v) => CooperationError::Extension(map(v)),
            Self::Protocol(v) => CooperationError::Protocol(v),
            Self::Initialization(v) => CooperationError::Initialization(v),
            Self::Refinement(v) => CooperationError::Refinement(v),
            Self::Unfinished(v) => CooperationError::Unfinished(v),
            Self::Cancelled => CooperationError::Cancelled,
        }
    }
}
