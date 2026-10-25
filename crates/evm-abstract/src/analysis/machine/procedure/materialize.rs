//! Explicit rich expansion funded by the caller's persistent postprocess ledger.

use super::{
    AlgebraRef, EvmNumericAlgebra, EvmProcedureReport, EvmProcedureSummary,
    EvmProcedureSummaryOutput, EvmProcedureSummaryRecord, EvmProcessInvocation,
    ProcedureReportOutcome, WordElement, WorkBudget,
};
use crate::{analysis::MachineError, resource::clone_payment::checkpoint};

impl<N: WordElement> EvmProcedureSummaryRecord<N> {
    /// Replays numerical bounds into independent read-only presentation roots.
    /// Supply the same application postprocess ledger on subsequent calls. This
    /// function cannot reset execution work, alter frontiers or certify a source.
    pub fn materialize<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> EvmProcedureReport<N> {
        let start = budget.used();
        let result = (|| -> Result<Vec<EvmProcedureSummary<N>>, MachineError> {
            checkpoint(budget)?;
            crate::resource::clone_payment::pay(
                budget,
                self.procedures.len().saturating_mul(8).saturating_add(8),
            )?;
            let mut procedures = Vec::with_capacity(self.procedures.len());
            for model in &self.procedures {
                checkpoint(budget)?;
                crate::resource::clone_payment::pay(
                    budget,
                    model
                        .dependencies
                        .len()
                        .saturating_add(model.covered_blocks.len())
                        .saturating_add(model.outputs.len().saturating_mul(8))
                        .saturating_add(16),
                )?;
                let process_input = model
                    .process_input
                    .as_ref()
                    .map(|input| {
                        Ok::<_, MachineError>(EvmProcessInvocation {
                            address: crate::domain::summary::clone_value(&input.address, budget)?,
                            caller: crate::domain::summary::clone_value(&input.caller, budget)?,
                            value: crate::domain::summary::clone_value(&input.value, budget)?,
                        })
                    })
                    .transpose()?;
                let frame = crate::analysis::engine::policy::clone_key(&model.frame, budget)?;
                let input = model.input.instantiate_with_payment(domain, budget)?;
                let mut outputs = Vec::with_capacity(model.outputs.len());
                for output in &model.outputs {
                    outputs.push(EvmProcedureSummaryOutput {
                        kind: output.kind,
                        data: output.data.instantiate_with_payment(domain, budget)?,
                        store: output.store.instantiate_with_payment(domain, budget)?,
                    });
                }
                procedures.push(EvmProcedureSummary {
                    process_input,
                    semantics: model.semantics,
                    frame,
                    input,
                    outputs,
                    dependencies: model.dependencies.clone(),
                    input_updates: model.input_updates,
                    output_updates: model.output_updates,
                    covered_blocks: model.covered_blocks.clone(),
                });
            }
            checkpoint(budget)?;
            Ok(procedures)
        })();
        let (outcome, procedures) = match result {
            Ok(procedures) => (ProcedureReportOutcome::Complete, procedures),
            Err(error) => (
                ProcedureReportOutcome::Refused(error.frontier_reason()),
                Vec::new(),
            ),
        };
        EvmProcedureReport {
            version: self.version,
            outcome,
            procedures,
            postprocess_work: budget.used().saturating_sub(start),
        }
    }
}
