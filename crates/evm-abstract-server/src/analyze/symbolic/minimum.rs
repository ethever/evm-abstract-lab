//! Original calldata uses the library's staged payer on the application ledger.

use super::Pool;
use evm_abstract::{
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement},
    world::{ByteSummaryProjectionError, EvmByteArray, EvmByteArraySummary},
};
use evm_abstract_protocol as api;

impl Pool<'_> {
    pub(in crate::analyze) fn minimum_byte_summary<
        N: WordElement,
        D: EvmNumericAlgebra<Element = N>,
    >(
        &mut self,
        source: &EvmByteArray<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmByteArraySummary<N>, api::ApiError> {
        self.charge(0)?;
        let budget = self
            .minimum_budget
            .as_mut()
            .expect("minimum byte projection owns the application ledger");
        let before = budget.used();
        // The paid implementation checks both the captured ledger cancellation
        // and this explicit Control before each native operation and afterward.
        let result = self
            .control
            .scope(|| source.summary_projection_with_budget(domain, budget));
        self.usage.work = self
            .usage
            .work
            .saturating_add(budget.used().saturating_sub(before) as u64);
        let summary = result.map_err(|error| {
            let kind = match &error {
                ByteSummaryProjectionError::Work => api::ExportFailureKind::Work,
                ByteSummaryProjectionError::Cancelled => api::ExportFailureKind::Cancelled,
                ByteSummaryProjectionError::Source(_) => api::ExportFailureKind::StateRoot,
            };
            let mut failure = self.failure(kind, 0);
            failure.message = format!("minimum input data projection failed: {error}");
            failure
        })?;
        self.charge(0)?;
        Ok(summary)
    }
}
