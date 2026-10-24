//! Bounded owner classes and paid Store preparation for coarse EVM processes.
//!
//! Residual denotes the complement of an immutable named vocabulary. It is not
//! a representative address, a must-alias identity, or an executable native
//! frame. Code identity and logical ADDRESS operands remain separate concerns.
//! The solver must explicitly install this representation before using it.

use super::StoreError;
use crate::resource::{CloneError, SummaryError};
use alloy_primitives::Address;

mod catalogue;
mod prepared;
mod roles;
#[cfg(test)]
mod tests;

pub use catalogue::{
    CapturedProcessCode, OwnerCatalogue, OwnerCatalogueLimits, ProcessNamespace, ProcessOwnerKey,
};
pub use prepared::PreparedProcessStore;
pub use roles::{AddressRole, CallRoles, CallValueRole, ProcessCallOperands, ProcessTargetAddress};

pub use crate::analysis::{ProcessAction, ProcessOwner};

/// A preparation refusal never grants a closed process rule.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProcessOwnerError {
    /// Capacity is explicit; mandatory captured owners cannot be truncated.
    #[error("owner/code vocabulary exceeds its frozen capacity")]
    Capacity,
    /// Every captured code account must remain a separate named account.
    #[error("captured code source {0} is absent from the named vocabulary")]
    UnnamedCode(Address),
    /// A supplied exact owner does not belong to this vocabulary.
    #[error("owner {0} is not a member of the named vocabulary")]
    UnnamedOwner(Address),
    /// A newly discovered captured body requires a new frozen vocabulary.
    #[error("captured process code is outside the frozen vocabulary: {0:?}")]
    UncapturedCode(CapturedProcessCode),
    /// Epoch and physical owner class are part of the process shape.
    #[error("process owner shapes or vocabulary epochs differ")]
    Shape,
    /// Allocation must not be treated as a CALL-family action.
    #[error("allocation requires constructor and deployment semantics")]
    Allocation,
    /// The original cumulative ledger refused this operation.
    #[error("process owner work exhausted or overflowed")]
    Work,
    /// Either the original payer or the active operation was cancelled.
    #[error("process owner operation cancelled")]
    Cancelled,
    /// Native Store retention/replay could not represent the prepared state.
    #[error("{0}")]
    Store(StoreError),
}

impl From<CloneError> for ProcessOwnerError {
    fn from(error: CloneError) -> Self {
        match error {
            CloneError::Cancelled => Self::Cancelled,
            CloneError::Work => Self::Work,
        }
    }
}

impl From<SummaryError<StoreError>> for ProcessOwnerError {
    fn from(error: SummaryError<StoreError>) -> Self {
        match error {
            SummaryError::Budget(error) => error.into(),
            SummaryError::Source(error) => Self::Store(error),
        }
    }
}
