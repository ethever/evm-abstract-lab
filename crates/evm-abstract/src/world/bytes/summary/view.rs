//! Borrowed numerical content and paid projection of original byte-state roots.

use super::{Content, EvmByteArray, EvmByteArraySummary, EvmValue, RangeError, WordElement};
use crate::{
    domain::{AlgebraRef, EvmNumericAlgebra},
    resource::{CloneError, SummaryError, WorkBudget},
};
use std::collections::BTreeMap;

/// Complete content representation, borrowed without a carrier clone.
pub enum EvmByteContentView<'a, N: WordElement> {
    /// Every content byte is zero.
    Zero,
    /// Complete exact content.
    Exact(&'a [u8]),
    /// Explicit cell bounds, with unknown-byte defaults elsewhere.
    Cells(&'a BTreeMap<usize, EvmValue<N>>),
}

/// Canonical content and length; full-word constraints retain their carriers.
pub struct EvmByteArraySummaryView<'a, N: WordElement> {
    /// Independent logical length.
    pub length: &'a EvmValue<N>,
    /// EVM memory allocation mode.
    pub memory: bool,
    /// Complete byte content abstraction.
    pub content: EvmByteContentView<'a, N>,
    /// Additional complete-window predicates, not independently split bytes.
    pub words: &'a BTreeMap<usize, EvmValue<N>>,
}

impl<N: WordElement> EvmByteArraySummary<N> {
    /// Constant-work borrowed view; consumers fund their observations and copies.
    pub fn view(&self) -> EvmByteArraySummaryView<'_, N> {
        EvmByteArraySummaryView {
            length: &self.length,
            memory: self.memory,
            content: match &self.content {
                Content::Zero => EvmByteContentView::Zero,
                Content::Exact(bytes) => EvmByteContentView::Exact(bytes),
                Content::Cells(cells) => EvmByteContentView::Cells(cells),
            },
            words: &self.words,
        }
    }
}

/// Refusal of a paid numerical projection of an original byte array.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ByteSummaryProjectionError {
    /// A required operation could not be quoted or funded on the supplied ledger.
    #[error("byte summary projection work exhausted or unquotable")]
    Work,
    /// Either the ledger owner or the current controlled operation cancelled.
    #[error("byte summary projection cancelled")]
    Cancelled,
    /// The original array has a typed range or retained-state failure.
    #[error(transparent)]
    Source(#[from] RangeError),
}

impl<N: WordElement> EvmByteArray<N> {
    /// Project the authoritative root using the existing per-operation payer.
    /// Each query, copy, policy projection and canonicalization is funded on
    /// this ledger before its hook runs, including hooks on intermediate values.
    /// Refusal retains all earlier payments and leaves the borrowed input intact.
    pub fn summary_projection_with_budget<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<EvmByteArraySummary<N>, ByteSummaryProjectionError> {
        self.summary_projection_with_payment(domain, budget)
            .map_err(|error| match error {
                SummaryError::Budget(CloneError::Work) => ByteSummaryProjectionError::Work,
                SummaryError::Budget(CloneError::Cancelled) => {
                    ByteSummaryProjectionError::Cancelled
                }
                SummaryError::Source(error) => ByteSummaryProjectionError::Source(error),
            })
    }
}
