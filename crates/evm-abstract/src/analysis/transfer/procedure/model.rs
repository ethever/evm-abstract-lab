//! Paid minimal snapshots and version-qualified immutable row reuse.
//!
//! Canonical bounds are copied through native payment hooks only when their
//! producer changes. Append-only dependency/coverage sets use cardinalities as
//! guards. Regional currentness has its own task/input/foreign-source lifetime;
//! sharing numerical rows never substitutes for those receipts.

use super::{
    AlgebraRef, Composition, ContextPolicy, EvmNumericAlgebra, EvmProcedure, EvmProcedureModel,
    EvmProcedureModels, EvmRegionalEvidence, EvmSolver, MachineError, PartitionPolicy, PolicyKey,
    StatePolicy, WordElement, WorkBudget, kernel_failure,
};
use crate::analysis::{EvmProcedureModelData, EvmProcedureModelOutput, ProcedureModelVersion};
use crate::world::{EvmStoreSummary, process::OwnerCatalogue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Guard {
    version: ProcedureModelVersion,
    input_updates: usize,
    output_updates: usize,
    dependencies: usize,
    covered_blocks: usize,
}

fn public_store<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    store: &EvmStoreSummary<N>,
    catalogue: Option<&OwnerCatalogue>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmStoreSummary<N>, MachineError> {
    Ok(
        if let Some(catalogue) = catalogue.filter(|c| c.namespace().is_some()) {
            store.process_export(catalogue, domain, budget)?
        } else {
            store.clone_with_payment(budget)?
        },
    )
}

fn retain_row<N: WordElement, Key: PolicyKey, D: EvmNumericAlgebra<Element = N>>(
    procedure: &EvmProcedure<N, Key>,
    guard: Guard,
    catalogue: Option<&OwnerCatalogue>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> Result<EvmProcedureModel<N>, MachineError> {
    crate::resource::clone_payment::pay(
        budget,
        procedure
            .dependencies
            .len()
            .saturating_add(procedure.covered_blocks.len())
            .saturating_add(procedure.outputs.len().saturating_mul(12))
            .saturating_add(32),
    )?;
    let frame = procedure.entry_payload.active();
    let process_input = if frame.key.process.is_some() {
        Some(crate::analysis::EvmProcessInvocation {
            address: crate::domain::summary::clone_value(&frame.address_operand, budget)?,
            caller: crate::domain::summary::clone_value(&frame.caller_operand, budget)?,
            value: crate::domain::summary::clone_value(&frame.call_value, budget)?,
        })
    } else {
        None
    };
    let frame = crate::analysis::engine::policy::clone_key(&procedure.frame, budget)?;
    let input = public_store(&procedure.entry_payload.store, catalogue, domain, budget)?;
    let mut outputs = Vec::with_capacity(procedure.outputs.len());
    for (kind, output) in &procedure.outputs {
        outputs.push(EvmProcedureModelOutput {
            kind: *kind,
            generation: output.generation,
            data: output.summary.data.clone_with_payment(budget)?,
            store: public_store(&output.summary.store, catalogue, domain, budget)?,
        });
    }
    crate::resource::clone_payment::checkpoint(budget)?;
    Ok(EvmProcedureModel::new(EvmProcedureModelData {
        version: guard.version,
        process_input,
        semantics: procedure.entry_payload.active().semantics,
        frame,
        input,
        outputs,
        dependencies: procedure.dependencies.iter().copied().collect(),
        input_updates: procedure.entry_updates,
        output_updates: procedure.output_updates,
        covered_blocks: procedure.covered_blocks.iter().copied().collect(),
    }))
}

impl<D, C, P, S, X, E> EvmSolver<'_, D, C, P, S, X, E>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
{
    fn model_rows(&mut self) -> Result<(EvmProcedureModels<D::Element>, bool), MachineError> {
        self.regional_checkpoint()?;
        let table = &self.table;
        self.reserve(table.procedures.len().saturating_mul(16).saturating_add(8))?;
        let table = &self.table;
        let versions = self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                table
                    .procedures
                    .iter()
                    .map(|procedure| {
                        run.slot_version(&procedure.output)
                            .map(|version| version.get())
                            .map_err(kernel_failure)
                    })
                    .collect::<Result<Vec<_>, _>>()
            },
        )?;
        let mut changed = self.table.model_snapshot.is_none()
            || self.table.model_rows.len() != self.table.procedures.len();
        for (id, output_revision) in versions.into_iter().enumerate() {
            let procedure = &self.table.procedures[id];
            let guard = Guard {
                version: ProcedureModelVersion {
                    namespace: procedure.namespace,
                    entry_revision: procedure.entry_revision,
                    output_revision,
                    source_epoch: self.table.process_catalogue.as_ref().map(|c| c.epoch()),
                },
                input_updates: procedure.entry_updates,
                output_updates: procedure.output_updates,
                dependencies: procedure.dependencies.len(),
                covered_blocks: procedure.covered_blocks.len(),
            };
            if self
                .table
                .model_rows
                .get(id)
                .is_some_and(|(old, _)| *old == guard)
            {
                continue;
            }
            let row = retain_row(
                procedure,
                guard,
                self.table.process_catalogue.as_deref(),
                self.domain,
                self.budget,
            )?;
            changed = true;
            if let Some(old) = self.table.model_rows.get_mut(id) {
                *old = (guard, row);
            } else {
                self.table.model_rows.push((guard, row));
            }
        }
        // A prior partial/refused preparation can update row caches without
        // publishing a directory. Compare its actual immutable handles too.
        if let Some(snapshot) = &self.table.model_snapshot {
            changed |= snapshot.len() != self.table.model_rows.len()
                || snapshot
                    .iter()
                    .zip(&self.table.model_rows)
                    .any(|(old, (_, row))| !old.shares_storage_with(row));
        }
        if changed {
            self.reserve(
                self.table
                    .model_rows
                    .len()
                    .saturating_mul(4)
                    .saturating_add(8),
            )?;
            let rows = self
                .table
                .model_rows
                .iter()
                .map(|(_, row)| row.clone())
                .collect();
            self.table.model_snapshot = Some(EvmProcedureModels::new(rows));
            self.table.evidence_dirty = true;
        }
        self.regional_checkpoint()?;
        Ok((
            self.table
                .model_snapshot
                .as_ref()
                .expect("retained directory")
                .clone(),
            changed,
        ))
    }

    pub(super) fn semantic_snapshot(
        &mut self,
    ) -> Result<(EvmProcedureModels<D::Element>, EvmRegionalEvidence, usize), MachineError> {
        let (rows, rows_changed) = self.model_rows()?;
        // Enabled cooperation may retire a foreign premise without changing a
        // native numeric slot. Never reuse its currency without its own guard.
        let cfg_changed = X::ENABLED || self.table.regional_snapshot.is_none();
        let cfg = if cfg_changed {
            let cfg = self.regional_cfg()?;
            self.reserve(8)?;
            let cfg = EvmRegionalEvidence::new(cfg);
            self.table.evidence_dirty = true;
            if !X::ENABLED {
                self.table.regional_snapshot = Some(cfg.clone());
            }
            cfg
        } else {
            self.reserve(4)?;
            self.table
                .regional_snapshot
                .as_ref()
                .expect("cached receipts")
                .clone()
        };
        self.regional_checkpoint()?;
        if rows_changed || cfg_changed || self.table.evidence_dirty {
            self.table.evidence_generation = self
                .table
                .evidence_generation
                .checked_add(1)
                .ok_or(MachineError::IdentityExhausted)?;
            self.table.evidence_dirty = false;
        }
        Ok((rows, cfg, self.table.evidence_generation))
    }
}

#[cfg(test)]
mod tests;
