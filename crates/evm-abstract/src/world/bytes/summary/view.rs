//! Borrowed numerical content, independent of executable byte-state roots.

use super::{Content, EvmByteArray, EvmByteArraySummary, EvmValue, WordElement};
use crate::domain::{AlgebraRef, EvmNumericAlgebra};
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

impl<N: WordElement> EvmByteArray<N> {
    /// Existing complete projection quote, before querying an original input.
    /// This grants no credit until the caller reserves it on its persistent
    /// ledger and checks cancellation before and after summary_projection.
    pub fn summary_projection_work<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        self.projection_work(domain)
    }
}
