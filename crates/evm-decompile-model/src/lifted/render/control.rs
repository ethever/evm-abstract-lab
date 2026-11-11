//! Select one complete projection for each source If and rendering boundary.
//! Suffix suppression follows that selected projection alone. Compact require
//! retains the original lazy failed route as its executable source witness;
//! explicit failure-first plans move that route without absorbing primitives.
use super::{FailureFirstIf, LiftedRenderError as Error, LiftedRenderIndex};
use crate::{EntityId, Ir, Terminator};
use std::collections::BTreeMap;

/// What the selected If does with its immediate following source suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftedIfSuffix {
    /// Retain the suffix, including one still reached by the successful arm.
    Keep,
    /// The require's certified lazy failed route owns this entire suffix.
    Absorb {
        /// Original containing source Sequence.
        sequence: EntityId,
        /// Inclusive child position.
        start: usize,
        /// Exclusive child position.
        end: usize,
    },
    /// Move the complete suffix into the explicit failed If arm.
    Move {
        /// Original containing source Sequence.
        sequence: EntityId,
        /// Inclusive child position.
        start: usize,
        /// Exclusive child position.
        end: usize,
    },
}

/// An admitted compact require over one exact, unchanged source occurrence.
/// Its private scope includes any required inherited failed continuation, so a
/// canonical procedure body cannot borrow failure code outside its boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequireIf {
    source: EntityId,
    success_node: EntityId,
    failure_node: EntityId,
    pass_nonzero: bool,
    scope: EntityId,
    suffix: LiftedIfSuffix,
}
impl RequireIf {
    /// Original source If occurrence.
    pub fn source(&self) -> EntityId {
        self.source
    }
    /// Original successful arm, including its simultaneous edge assignments.
    pub fn success_node(&self) -> EntityId {
        self.success_node
    }
    /// Original lazy failed fragment, retained for execution and inspection.
    pub fn failure_node(&self) -> EntityId {
        self.failure_node
    }
    /// Whether the source nonzero condition selects success.
    pub fn pass_nonzero(&self) -> bool {
        self.pass_nonzero
    }
}

/// One selected projection, never an independent collection of omission flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftedIfPlan<'a> {
    /// Exact empty REVERT or Error(string), with a certified lazy failed route.
    Require(&'a RequireIf),
    /// Complete explicit failed arm followed by the original successful arm.
    FailureFirst(&'a FailureFirstIf),
    /// Preserve the original If and both of its arms.
    Original,
}
impl LiftedIfPlan<'_> {
    /// Suffix ownership of this selected projection.
    pub fn suffix(self) -> LiftedIfSuffix {
        match self {
            Self::Require(plan) => plan.suffix,
            Self::FailureFirst(plan) => {
                plan.suffix
                    .map_or(LiftedIfSuffix::Keep, |(sequence, start, end)| {
                        LiftedIfSuffix::Move {
                            sequence,
                            start,
                            end,
                        }
                    })
            }
            Self::Original => LiftedIfSuffix::Keep,
        }
    }
}

#[derive(Default)]
pub(super) struct ControlPlanIndex {
    requires: BTreeMap<EntityId, RequireIf>,
    cuts: BTreeMap<(EntityId, usize), EntityId>,
}
impl ControlPlanIndex {
    pub(super) fn new(
        index: &LiftedRenderIndex<'_>,
        ir: &Ir,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Self {
        let mut result = Self::default();
        let Ok(source) = &index.failure_first else {
            return result;
        };
        if !index.program.has_guards() {
            return result;
        }
        let units = ir.blocks.iter().fold(
            ir.edges.len().saturating_add(index.program.record_count()),
            |n, b| {
                let successors = match &b.terminator {
                    Terminator::Fallthrough { .. } => 1,
                    Terminator::Jump { edges, .. } => edges.len(),
                    Terminator::Branch {
                        then_edges,
                        else_edge,
                        ..
                    } => then_edges
                        .len()
                        .saturating_add(usize::from(else_edge.is_some())),
                    _ => 0,
                };
                n.saturating_add(b.operations.len())
                    .saturating_add(successors)
                    .saturating_add(4)
            },
        );
        if !pay(units.saturating_mul(8)) {
            return result;
        }
        // Byte equality alone cannot absorb observations, calls or writes.
        // Admit the existing ranked failure graph even for legacy empty-revert
        // guards, for which there may be no payload records to trigger it.
        let raw = ir.blocks.iter().map(|b| (b.id, b)).collect();
        if super::super::payload::check_failure_payloads(index.program, ir, &raw, &index.blocks)
            .is_err()
        {
            return result;
        }
        for control in &index.program.controls {
            let Some(guard) = index.guard(control.node) else {
                continue;
            };
            match index.guard_data_bindable(control, pay) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(Error::Limit) => return Self::default(),
                Err(_) => continue,
            }
            let Some(&scope) = source.require_scopes.get(&control.node) else {
                continue;
            };
            let suffix = guard
                .continuation
                .as_ref()
                .filter(|c| !c.pass_falls_through)
                .map_or(LiftedIfSuffix::Keep, |c| LiftedIfSuffix::Absorb {
                    sequence: c.sequence,
                    start: c.start,
                    end: c.end,
                });
            // The old guard shape's false fallthrough bit is insufficient to
            // cut a suffix: an earlier unresolved successor can escape before
            // a later lexical STOP. Reuse the exact explicit-plan admission,
            // including OPEN and terminal-body checks, for every absorption.
            if let LiftedIfSuffix::Absorb {
                sequence,
                start,
                end,
            } = suffix
                && !index
                    .failure_first_direction(control.node, scope, !guard.pass_nonzero)
                    .is_some_and(|plan| {
                        plan.failure_node() == guard.failure_node
                            && plan.success_node() == guard.pass_node
                            && plan.suffix == Some((sequence, start, end))
                    })
            {
                continue;
            }
            if let LiftedIfSuffix::Absorb {
                sequence, start, ..
            } = suffix
            {
                result.cuts.insert((sequence, start), control.node);
            }
            result.requires.insert(
                control.node,
                RequireIf {
                    source: control.node,
                    success_node: guard.pass_node,
                    failure_node: guard.failure_node,
                    pass_nonzero: guard.pass_nonzero,
                    scope,
                    suffix,
                },
            );
        }
        result
    }
}

impl<'a> LiftedRenderIndex<'a> {
    /// Prefer a certified require, then complete explicit failure-first If,
    /// otherwise preserve the original source If within this rendering root.
    pub fn if_plan(&self, source: EntityId, boundary: EntityId) -> LiftedIfPlan<'_> {
        if self.shared_calls.contains_key(&source) {
            return LiftedIfPlan::Original;
        }
        self.expanded_if_plan(source, boundary)
    }
    /// The selected projection inside an admitted canonical definition, whose
    /// body expands its own source invocation rather than substituting a call.
    pub fn procedure_if_plan(
        &self,
        source: EntityId,
        procedure: &crate::SharedProcedure,
    ) -> LiftedIfPlan<'_> {
        if self.procedure(procedure.id) != Some(procedure) {
            return LiftedIfPlan::Original;
        }
        self.expanded_if_plan(source, procedure.body)
    }
    /// Definitions expand canonical bodies, including their invocation roots.
    pub(crate) fn expanded_if_plan(
        &self,
        source: EntityId,
        boundary: EntityId,
    ) -> LiftedIfPlan<'_> {
        let Ok(index) = &self.failure_first else {
            return LiftedIfPlan::Original;
        };
        if let Some(plan) = self.control_plans.requires.get(&source)
            && index.contains(boundary, source)
            && index.contains(boundary, plan.scope)
        {
            return LiftedIfPlan::Require(plan);
        }
        self.failure_first_expanded(source, boundary)
            .map_or(LiftedIfPlan::Original, LiftedIfPlan::FailureFirst)
    }
    /// Skip only a suffix absorbed or moved by the selected preceding If.
    pub fn selected_sequence_end(
        &self,
        sequence: EntityId,
        position: usize,
        boundary: EntityId,
    ) -> Option<usize> {
        self.selected_cut(sequence, position, boundary, false)
    }
    pub(crate) fn expanded_selected_sequence_end(
        &self,
        sequence: EntityId,
        position: usize,
        boundary: EntityId,
    ) -> Option<usize> {
        self.selected_cut(sequence, position, boundary, true)
    }
    fn selected_cut(
        &self,
        sequence: EntityId,
        position: usize,
        boundary: EntityId,
        expanded: bool,
    ) -> Option<usize> {
        let source = *self
            .control_plans
            .cuts
            .get(&(sequence, position))
            .or_else(|| {
                self.failure_first
                    .as_ref()
                    .ok()?
                    .cuts
                    .get(&(sequence, position))
            })?;
        let plan = if expanded {
            self.expanded_if_plan(source, boundary)
        } else {
            self.if_plan(source, boundary)
        };
        match plan.suffix() {
            LiftedIfSuffix::Absorb {
                sequence: owner,
                start,
                end,
            }
            | LiftedIfSuffix::Move {
                sequence: owner,
                start,
                end,
            } if owner == sequence && start == position => Some(end),
            _ => None,
        }
    }
    /// Format an admitted require without changing any source construction.
    pub fn render_require(
        &self,
        plan: &RequireIf,
        pay: impl FnMut(usize) -> bool,
    ) -> Result<String, Error> {
        if self.control_plans.requires.get(&plan.source) != Some(plan) {
            return Err(Error::MissingWitness(plan.source));
        }
        let control = self
            .control(plan.source)
            .ok_or(Error::MissingWitness(plan.source))?;
        self.render_guard(control, pay)?
            .ok_or(Error::MissingWitness(plan.source))
    }
}
