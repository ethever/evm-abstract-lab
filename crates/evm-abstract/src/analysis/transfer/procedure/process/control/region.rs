//! Compose effect-free control, retaining the unchanged world at macro boundaries.
//!
//! Only immutable code-derived Steps are cached. Enabled foreign compositions
//! use the uncompressed schedule, so no dynamic premise or owner epoch is skipped.

use super::Step;
use crate::{
    analysis::{
        MachineError, ProcessControlBlock, ProcessControlRegion, ProcessControlRelation,
        ProcessFrameKey, ProcessLocation,
    },
    bytecode::Program,
    resource::{WorkBudget, clone_payment::pay},
};
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(in crate::analysis::transfer::procedure) struct Cache(Vec<Captured>);

struct Captured {
    program: Program,
    relation: Arc<ProcessControlRelation>,
    steps: Vec<Step>,
    regions: Vec<Option<Arc<ProcessControlRegion>>>,
}

pub(in crate::analysis::transfer::procedure::process) enum Plan {
    Boundary(Step),
    Region(Arc<ProcessControlRegion>),
}

impl Cache {
    pub(in crate::analysis::transfer::procedure::process) fn plan(
        &mut self,
        program: &Program,
        frame: &ProcessFrameKey,
        budget: &mut WorkBudget,
    ) -> Result<Plan, MachineError> {
        pay(budget, self.0.len().saturating_mul(32).saturating_add(16))?;
        let mut found = None;
        for (index, cached) in self.0.iter().enumerate() {
            if cached.relation.epoch == frame.epoch
                && cached.relation.source == frame.source
                && cached.program.eq_with_payment(program, budget)?
            {
                found = Some(index);
                break;
            }
        }
        let index = match found {
            Some(index) => index,
            None => {
                let cached = Captured::new(program, frame, budget)?;
                self.0.push(cached);
                self.0.len() - 1
            }
        };
        let cached = &mut self.0[index];
        if let ProcessLocation::Block(index) = frame.location {
            let step = *cached.steps.get(index).ok_or(MachineError::Shape)?;
            if step.boundary() {
                return Ok(Plan::Boundary(step));
            }
        }
        let index = coordinate(frame.location, cached.steps.len())?;
        if let Some(region) = &cached.regions[index] {
            pay(budget, 8)?;
            return Ok(Plan::Region(Arc::clone(region)));
        }
        let region = Arc::new(cached.compose(frame.location, budget)?);
        pay(budget, 8)?;
        cached.regions[index] = Some(Arc::clone(&region));
        Ok(Plan::Region(region))
    }
}

impl Captured {
    fn new(
        program: &Program,
        frame: &ProcessFrameKey,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        pay(
            budget,
            program.blocks().len().saturating_mul(64).saturating_add(32),
        )?;
        let mut steps = Vec::with_capacity(program.blocks().len());
        for index in 0..program.blocks().len() {
            steps.push(Step::at(program, index, budget)?);
        }
        let blocks = steps
            .iter()
            .map(|step| ProcessControlBlock {
                successors: step.successors,
                continuation: step.continuation,
                boundary: step.boundary(),
            })
            .collect();
        let jump_targets = program.jumpdest_blocks().values().copied().collect();
        let regions = vec![None; steps.len().saturating_add(2)];
        let relation = Arc::new(ProcessControlRelation {
            epoch: frame.epoch,
            source: frame.source,
            blocks,
            jump_targets,
        });
        Ok(Self {
            program: program.clone(),
            relation,
            steps,
            regions,
        })
    }

    fn compose(
        &self,
        entry: ProcessLocation,
        budget: &mut WorkBudget,
    ) -> Result<ProcessControlRegion, MachineError> {
        let count = self.steps.len();
        // Dense visitation and pending normal edges are linear in the immutable
        // relation. The shared unknown-target union is expanded only at its own
        // entry, so ordinary origins retain a single hub edge, not B targets.
        pay(budget, count.saturating_add(2).saturating_mul(96))?;
        let mut visited = vec![false; count + 2];
        let mut through = vec![false; count + 2];
        let mut boundaries = vec![false; count + 2];
        let mut pending = vec![entry];
        while let Some(location) = pending.pop() {
            MachineError::check_cancelled()?;
            let index = coordinate(location, count)?;
            if visited[index] {
                continue;
            }
            visited[index] = true;
            match location {
                ProcessLocation::Block(index) => {
                    let step = self.steps[index];
                    if step.boundary() {
                        boundaries[index] = true;
                    } else {
                        through[index] = true;
                        pending.extend(
                            step.successors
                                .into_iter()
                                .flatten()
                                .map(|(location, _)| location),
                        );
                    }
                }
                ProcessLocation::JumpTargets if location == entry => {
                    through[index] = true;
                    pending.extend(
                        self.relation
                            .jump_targets
                            .iter()
                            .map(|index| ProcessLocation::Block(*index)),
                    );
                }
                ProcessLocation::JumpTargets | ProcessLocation::End => boundaries[index] = true,
                ProcessLocation::Any => return Err(MachineError::Shape),
            }
        }
        let locations = |flags: Vec<bool>| {
            flags
                .into_iter()
                .enumerate()
                .filter_map(|(index, keep)| {
                    keep.then_some(if index < count {
                        ProcessLocation::Block(index)
                    } else if index == count {
                        ProcessLocation::JumpTargets
                    } else {
                        ProcessLocation::End
                    })
                })
                .collect()
        };
        Ok(ProcessControlRegion {
            relation: Arc::clone(&self.relation),
            entry,
            through: locations(through),
            boundaries: locations(boundaries),
        })
    }
}

fn coordinate(location: ProcessLocation, blocks: usize) -> Result<usize, MachineError> {
    match location {
        ProcessLocation::Block(index) if index < blocks => Ok(index),
        ProcessLocation::JumpTargets => Ok(blocks),
        ProcessLocation::End => Ok(blocks + 1),
        _ => Err(MachineError::Shape),
    }
}
