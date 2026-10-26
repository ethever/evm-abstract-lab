//! Intern real source relations once; per-Body receipts retain only coordinates.
use super::Pool;
use crate::analyze::{mapping, process};
use alloy_primitives::B256;
use evm_abstract::{analysis, resource::WorkBudget};
use evm_abstract_protocol as api;
use std::{collections::BTreeMap, sync::Arc};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Directory {
    rows: Vec<Arc<api::ProcessControlRelation>>,
    // Keep the compared native handle alive, so pointer reuse cannot admit a
    // different relation. Different allocations at one key require equality.
    native: BTreeMap<(B256, analysis::ProcessSource), NativeRelation>,
}

struct NativeRelation {
    id: usize,
    relation: Arc<analysis::ProcessControlRelation>,
}

fn block(native: &analysis::ProcessControlBlock) -> api::ProcessControlBlock {
    api::ProcessControlBlock {
        successors: native.successors.map(|edge| {
            edge.map(|(to, kind)| api::ProcessControlEdge {
                to: process::location(to),
                kind: mapping::edge(kind),
            })
        }),
        continuation: native.continuation.map(process::location),
        boundary: native.boundary,
    }
}

fn relation_work(native: &analysis::ProcessControlRelation) -> usize {
    native
        .blocks
        .len()
        .saturating_mul(16)
        .saturating_add(native.jump_targets.len().saturating_mul(2))
        .saturating_add(128)
}

impl<'a> Pool<'a> {
    pub(in crate::analyze) fn seed_controls(
        &mut self,
        rows: Vec<Arc<api::ProcessControlRelation>>,
    ) {
        self.control_directory.rows = rows;
    }

    /// Rich graph construction attaches the existing payer after SSA/native
    /// projection. Minimum mode already borrows that payer for every charge.
    pub(in crate::analyze) fn attach_control_budget(&mut self, budget: &'a mut WorkBudget) {
        self.control_budget = Some(budget);
    }

    pub(in crate::analyze) fn release_control_budget(&mut self) -> Option<&'a mut WorkBudget> {
        self.control_budget.take()
    }

    fn control_charge(&mut self, work: usize) -> Result<(), api::ApiError> {
        self.charge(0)?;
        if let Some(budget) = &mut self.control_budget {
            let kind = if !budget.charge(0) {
                Some(api::ExportFailureKind::Cancelled)
            } else if budget.exhausted() || work == usize::MAX || !budget.charge(work) {
                Some(api::ExportFailureKind::Work)
            } else {
                None
            };
            if let Some(kind) = kind {
                return Err(self.failure(kind, work as u64));
            }
        }
        self.charge(work)
    }

    pub(in crate::analyze) fn control_region(
        &mut self,
        native: &analysis::ProcessControlRegion,
    ) -> Result<api::ProcessControlRegion, api::ApiError> {
        self.control_charge(
            native
                .through
                .len()
                .saturating_add(native.boundaries.len())
                .saturating_mul(2)
                .saturating_add(8),
        )?;
        let relation = self.control_relation(&native.relation)?;
        Ok(api::ProcessControlRegion {
            relation,
            entry: process::location(native.entry),
            through: native
                .through
                .iter()
                .copied()
                .map(process::location)
                .collect(),
            boundaries: native
                .boundaries
                .iter()
                .copied()
                .map(process::location)
                .collect(),
        })
    }

    fn control_relation(
        &mut self,
        native: &Arc<analysis::ProcessControlRelation>,
    ) -> Result<usize, api::ApiError> {
        self.control_charge(
            self.control_directory
                .native
                .len()
                .checked_ilog2()
                .unwrap_or(0) as usize
                + 16,
        )?;
        let key = (native.epoch, native.source);
        if let Some((id, previous)) = self
            .control_directory
            .native
            .get(&key)
            .map(|previous| (previous.id, Arc::clone(&previous.relation)))
        {
            if !Arc::ptr_eq(native, &previous) {
                self.control_charge(relation_work(native))?;
                if native != &previous {
                    return Err(process::invalid(api::ProcessValidationKind::Source));
                }
                self.control_charge(0)?;
                self.control_directory.native.insert(
                    key,
                    NativeRelation {
                        id,
                        relation: Arc::clone(native),
                    },
                );
            }
            return Ok(id);
        }
        self.control_charge(
            self.control_directory
                .rows
                .len()
                .saturating_mul(128)
                .saturating_add(relation_work(native)),
        )?;
        let source = process::source(native.source);
        let epoch = api::ProcessDigest(native.epoch.0);
        let id = if let Some(row) = self
            .control_directory
            .rows
            .iter()
            .find(|row| row.epoch == epoch && row.source == source)
        {
            if row.blocks.len() != native.blocks.len()
                || row.jump_targets != native.jump_targets
                || row
                    .blocks
                    .iter()
                    .zip(&native.blocks)
                    .any(|(projected, native)| *projected != block(native))
            {
                return Err(process::invalid(api::ProcessValidationKind::Source));
            }
            row.id
        } else {
            let id = self.control_directory.rows.len();
            self.control_directory
                .rows
                .push(Arc::new(api::ProcessControlRelation {
                    id,
                    epoch,
                    source,
                    program: None,
                    blocks: native.blocks.iter().map(block).collect(),
                    jump_targets: native.jump_targets.clone(),
                }));
            id
        };
        self.control_directory.native.insert(
            key,
            NativeRelation {
                id,
                relation: Arc::clone(native),
            },
        );
        Ok(id)
    }

    pub(in crate::analyze) fn finish_controls(
        &mut self,
        programs: &[Arc<api::ProgramInfo>],
    ) -> Result<Vec<Arc<api::ProcessControlRelation>>, api::ApiError> {
        for index in 0..self.control_directory.rows.len() {
            self.control_charge(programs.len().saturating_mul(128).saturating_add(4))?;
            let row = &mut self.control_directory.rows[index];
            let program = programs
                .iter()
                .find(|program| row.source.matches_program(program))
                .map(|program| program.id);
            if row.program.is_some_and(|id| Some(id) != program)
                || (program.is_none()
                    && matches!(
                        row.source,
                        api::ProcessSource::InputProgram(_) | api::ProcessSource::Runtime(_)
                    ))
            {
                return Err(process::invalid(api::ProcessValidationKind::Source));
            }
            // An existing published row stays immutable, including honest
            // absence of constructor bytes. New rows have one private handle.
            if let Some(row) = Arc::get_mut(row) {
                row.program = program;
            }
        }
        self.control_charge(0)?;
        Ok(std::mem::take(&mut self.control_directory.rows))
    }
}
