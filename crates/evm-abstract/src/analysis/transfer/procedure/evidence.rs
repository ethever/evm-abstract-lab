//! Compact, revision-qualified evidence from the native relative worklist.
//!
//! The native worklist owns admission; the global kernel owns closure. Receipts retain
//! only identities and actual delivered edges; they contain no numerical state,
//! instruction inventory or SSA definitions. Accumulated native topology remains
//! separate from the latest transfer's evidence.

use analysis_kernel::Version;
use serde::Serialize;

use super::{
    Composition, ContextPolicy, EvmExecution, EvmSolver, FrameKey, MachineEdgeKind, MachineError,
    OutcomeKind, PartitionPolicy, StatePolicy,
};
use crate::analysis::{cooperation::InputTasks, worklist::runtime::TaskKind};
use crate::domain::EvmNumericAlgebra;

#[cfg(test)]
pub(super) use tests::{cancel_after_export_census, refuse_body_instantiation};

/// Coarse regional control flow, independent of the detailed machine CFG and SSA.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EvmRegionalCfg {
    /// Real admitted body nodes, indexed by their native table IDs.
    pub nodes: Vec<EvmRegionalNode>,
    /// Real retained caller continuations, indexed by native subscriber IDs.
    pub calls: Vec<EvmRegionalCall>,
}

/// An admitted body identity; admission alone does not imply execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRegionalNode {
    /// Index in this record's regional nodes, separate from machine-state IDs.
    pub id: usize,
    /// Index in this record's procedure summaries.
    pub procedure: usize,
    /// This body's own block, code, owner, caller, static mode and context.
    pub frame: FrameKey,
    /// Current native input-slot version, including scheduling publications.
    pub input_revision: u64,
    /// Cooperative input currently excludes this admitted body.
    pub excluded: bool,
    /// Accumulated admitted local topology, including delivered continuations.
    /// These IDs alone do not certify a transfer of the current input.
    pub successors: Vec<usize>,
    /// Latest actual interpreter/environment transfer, absent for pending nodes.
    pub transfer: Option<EvmRegionalTransfer>,
}

/// Receipt of one actual transfer, without per-instruction evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRegionalTransfer {
    /// Actual interpretation; code kind, Program and source hash stay unchanged.
    pub semantics: crate::analysis::TransferSemantics,
    /// Conservative process rules, never instruction/SSA execution evidence.
    pub process: Option<crate::analysis::EvmProcessTransfer>,
    /// Native input version actually interpreted, after any admitted retry.
    pub input_revision: u64,
    /// The native owner finished and this still describes its current input.
    /// A current partial receipt does not establish closure or full coverage.
    /// Always false for cooperative compositions until foreign-source validity
    /// can be checked independently of the native slot revision.
    pub current: bool,
    /// No transfer or successor-admission obligation was refused or deferred.
    /// When current is false, this describes only the historical transfer.
    pub complete: bool,
    /// At least one captured-program instruction was actually visited.
    /// Unknown environment processing and synthetic end blocks do not set it.
    pub visited_block: bool,
    /// Typed local successors actually admitted by this transfer, in order.
    /// Returned callee continuations belong to their separate delivery receipts.
    pub successors: Vec<EvmRegionalEdge>,
    /// Subscriber IDs actually admitted by this transfer.
    pub calls: Vec<usize>,
    /// Terminal alternatives emitted by the interpreter. A publication refusal
    /// leaves complete false; existence alone does not establish output closure.
    pub outcomes: Vec<OutcomeKind>,
}

/// One real native local successor; no code-derived edge is inferred here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EvmRegionalEdge {
    /// Destination regional body ID, never a machine-state ID or bytecode PC.
    pub to: usize,
    /// Control reason supplied by the actual native successor/delivery.
    pub kind: MachineEdgeKind,
}

/// A separate caller template subscribed to a callee's growing outputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRegionalCall {
    /// Whether the relation is a primitive action or a collapsed chain.
    pub scope: Option<crate::analysis::ProcessControlScope>,
    /// Primitive process action, independent of callee code kind. Native calls
    /// retain their captured-instruction evidence instead.
    pub action: Option<crate::analysis::ProcessAction>,
    /// Account named by the action before code/delegation resolution; this is
    /// separate from both the callee's code account and its state owner.
    pub target: Option<crate::analysis::ProcessOwner>,
    /// Native subscriber ID, scoped to this table snapshot.
    pub id: usize,
    /// Caller regional body ID.
    pub from: usize,
    /// Callee index in this record's procedure summaries.
    pub callee: usize,
    /// Native callee entry-body IDs; these can still be pending.
    pub entries: Vec<usize>,
    /// Current subscriber-template input version.
    pub input_revision: u64,
    /// Latest successful delivery per outcome, possibly stale after growth.
    pub deliveries: Vec<EvmRegionalReturn>,
}

/// A callee output actually delivered to this subscriber's continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmRegionalReturn {
    /// CREATE-specific delivered cases; empty for ordinary CALL completion.
    pub creation: Vec<crate::analysis::process_creation::CompletionCase>,
    /// Delivered relative termination alternative.
    pub kind: OutcomeKind,
    /// Subscriber-template version actually instantiated.
    pub input_revision: u64,
    /// Callee outcome generation actually delivered, including feedback changes.
    pub output_generation: usize,
    /// The delivery owner finished and both native delivery stamps still match.
    /// This describes the subscriber/output pair, not closure of the callee.
    /// Cooperative compositions conservatively withhold this certification.
    pub current: bool,
    /// Caller-body continuations actually admitted by this delivery.
    pub successors: Vec<EvmRegionalEdge>,
}

impl EvmRegionalTransfer {
    fn work_size(&self) -> usize {
        10usize
            .saturating_add(self.process.as_ref().map_or(0, |process| {
                process
                    .actions
                    .len()
                    .saturating_add(process.unresolved.len())
                    .saturating_add(process.creations.iter().fold(0usize, |work, creation| {
                        work.saturating_add(creation.exceptions.len())
                            .saturating_add(creation.rejections.len().saturating_mul(4))
                            .saturating_add(16)
                    }))
                    .saturating_add(4)
                    // A control receipt shares immutable paid source/region
                    // allocations; snapshot cloning copies only their handle.
                    .saturating_add(usize::from(process.control.is_some()).saturating_mul(8))
            }))
            .saturating_add(self.successors.len().saturating_mul(2))
            .saturating_add(self.calls.len())
            .saturating_add(self.outcomes.len())
    }

    fn snapshot(&self, input: Version, excluded: bool, source_valid: bool) -> Self {
        let mut receipt = self.clone();
        receipt.current &= source_valid && self.input_revision == input.get() && !excluded;
        receipt
    }
}

/// Pending delivery edges are private until the native delivered stamp commits.
/// One refused attempt may discard old display evidence, never semantic outputs.
pub(super) struct ReturnReceipt {
    report: EvmRegionalReturn,
    delivered: bool,
}

impl ReturnReceipt {
    fn work_size(&self) -> usize {
        6usize
            .saturating_add(self.report.successors.len().saturating_mul(2))
            .saturating_add(self.report.creation.len().saturating_mul(32))
    }

    fn snapshot(
        &self,
        input: Version,
        output_generation: Option<usize>,
        delivered: Option<(Version, usize)>,
        source_valid: bool,
    ) -> Option<EvmRegionalReturn> {
        if !self.delivered {
            return None;
        }
        let mut receipt = self.report.clone();
        receipt.current &= source_valid
            && receipt.input_revision == input.get()
            && output_generation == Some(receipt.output_generation)
            && delivered.is_some_and(|(version, generation)| {
                version.get() == receipt.input_revision && generation == receipt.output_generation
            });
        Some(receipt)
    }
}

#[derive(Clone, Copy)]
enum EdgeReceipt {
    Body(usize),
    Return(usize, OutcomeKind),
}

fn outcome(case: u8) -> Option<OutcomeKind> {
    match case {
        0 => Some(OutcomeKind::Return),
        1 => Some(OutcomeKind::Revert),
        2 => Some(OutcomeKind::Failure),
        _ => None,
    }
}

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    /// A failed preparation or cooperative reevaluation cannot leave an older
    /// successful receipt certified merely because its numeric slot is equal.
    pub(super) fn regional_begin(&mut self, task: &TaskKind<<X as InputTasks>::Task>) {
        self.regional_current(task, false);
    }

    /// Called only after the native evaluation has successfully finished.
    /// Snapshotting additionally checks every retained native semantic version.
    pub(super) fn regional_current(
        &mut self,
        task: &TaskKind<<X as InputTasks>::Task>,
        current: bool,
    ) {
        self.table.regional_snapshot = None;
        match task {
            TaskKind::Body(id) => {
                if let Some(receipt) = &mut self.table.nodes[*id].transfer {
                    receipt.current = current;
                }
            }
            TaskKind::Resume(id) => {
                for receipt in self.table.subscribers[*id].returns.values_mut() {
                    receipt.report.current = current && receipt.delivered;
                }
            }
            _ => {}
        }
    }

    pub(super) fn regional_transfer(
        &mut self,
        id: usize,
        input: Version,
        execution: &EvmExecution<D::Element>,
    ) -> Result<(), MachineError> {
        // Reserve creation and eventual release, including replacement of the
        // prior receipt. No EVM value, payload or Store is visited or cloned.
        let old_work = self.table.nodes[id]
            .transfer
            .as_ref()
            .map_or(0, EvmRegionalTransfer::work_size);
        self.reserve(
            old_work.saturating_add(
                8usize
                    .saturating_add(execution.outcomes.len())
                    .saturating_mul(2),
            ),
        )?;
        // Only three termination categories exist. A fixed presence scan keeps
        // the quoted work linear even when many paths emit the same outcome.
        let mut present = [false; 3];
        for output in &execution.outcomes {
            present[match output.kind {
                OutcomeKind::Return => 0,
                OutcomeKind::Revert => 1,
                OutcomeKind::Failure => 2,
            }] = true;
        }
        let outcomes = [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ]
        .into_iter()
        .zip(present)
        .filter_map(|(kind, present)| present.then_some(kind))
        .collect();
        if let Some(process) = &execution.process {
            self.reserve(
                process
                    .actions
                    .len()
                    .saturating_add(process.unresolved.len())
                    .saturating_add(4),
            )?;
        }
        self.table.nodes[id].transfer = Some(EvmRegionalTransfer {
            semantics: self.table.nodes[id].payload.active().semantics,
            process: execution.process.clone(),
            input_revision: input.get(),
            current: false,
            complete: execution.frontiers.is_empty(),
            visited_block: !execution.executed_pcs.is_empty(),
            successors: Vec::new(),
            calls: Vec::new(),
            outcomes,
        });
        Ok(())
    }

    pub(super) fn regional_transfer_complete(&mut self, id: usize, complete: bool) {
        if let Some(receipt) = &mut self.table.nodes[id].transfer {
            receipt.complete &= complete;
        }
    }

    /// observe_body_edge is reached after native node admission, for both
    /// interpreter successors and separately executed Resume deliveries.
    pub(super) fn regional_edge(
        &mut self,
        from: usize,
        to: usize,
        kind: MachineEdgeKind,
    ) -> Result<(), MachineError> {
        if self.active.is_none() {
            return Ok(());
        }
        self.table.regional_snapshot = None;
        let Some(producer) = self.producer else {
            return Ok(());
        };
        let target = match producer.kind {
            0 if producer.id == from
                && self.table.nodes[from]
                    .transfer
                    .as_ref()
                    .is_some_and(|receipt| receipt.input_revision == producer.input.get()) =>
            {
                Some(EdgeReceipt::Body(from))
            }
            1 if self.table.subscribers[producer.id].from == from => outcome(producer.case)
                .and_then(|kind| {
                    self.table.subscribers[producer.id]
                        .returns
                        .get(&kind)
                        .filter(|receipt| receipt.report.input_revision == producer.input.get())
                        .map(|_| EdgeReceipt::Return(producer.id, kind))
                }),
            _ => None,
        };
        let Some(target) = target else {
            return Ok(());
        };
        // Append rather than rescanning accumulated fanout on every edge. The
        // receipt records actual admission order; duplicates carry no new proof.
        self.reserve(4)?;
        let edge = EvmRegionalEdge { to, kind };
        match target {
            EdgeReceipt::Body(id) => self.table.nodes[id]
                .transfer
                .as_mut()
                .expect("matched transfer")
                .successors
                .push(edge),
            EdgeReceipt::Return(id, kind) => self.table.subscribers[id]
                .returns
                .get_mut(&kind)
                .expect("matched delivery")
                .report
                .successors
                .push(edge),
        }
        Ok(())
    }

    /// Called with the actual admitted subscriber, before optional cooperation.
    pub(super) fn regional_call(&mut self, subscription: usize) -> Result<(), MachineError> {
        if self.active.is_none() {
            return Ok(());
        }
        self.table.regional_snapshot = None;
        let Some(producer) = self.producer else {
            return Ok(());
        };
        let from = self.table.subscribers[subscription].from;
        if producer.kind != 0
            || producer.id != from
            || !self.table.nodes[from]
                .transfer
                .as_ref()
                .is_some_and(|receipt| receipt.input_revision == producer.input.get())
        {
            return Ok(());
        }
        self.reserve(2)?;
        self.table.nodes[from]
            .transfer
            .as_mut()
            .expect("matched transfer")
            .calls
            .push(subscription);
        Ok(())
    }

    pub(super) fn regional_delivery(
        &mut self,
        subscription: usize,
        kind: OutcomeKind,
        input: Version,
        generation: usize,
    ) -> Result<(), MachineError> {
        let old_work = self.table.subscribers[subscription]
            .returns
            .get(&kind)
            .map_or(0, ReturnReceipt::work_size);
        self.reserve(old_work.saturating_add(12))?;
        self.table.subscribers[subscription].returns.insert(
            kind,
            ReturnReceipt {
                report: EvmRegionalReturn {
                    creation: Vec::new(),
                    kind,
                    input_revision: input.get(),
                    output_generation: generation,
                    current: false,
                    successors: Vec::new(),
                },
                delivered: false,
            },
        );
        Ok(())
    }

    /// Called adjacent to installation of the existing native delivery stamp.
    pub(super) fn regional_delivered(&mut self, subscription: usize, kind: OutcomeKind) {
        if let Some(receipt) = self.table.subscribers[subscription].returns.get_mut(&kind) {
            receipt.delivered = true;
        }
    }

    pub(super) fn regional_creation_cases(
        &mut self,
        subscription: usize,
        kind: OutcomeKind,
        cases: [Option<crate::analysis::process_creation::CompletionCase>; 2],
    ) -> Result<(), MachineError> {
        self.reserve(128)?;
        if let Some(receipt) = self.table.subscribers[subscription].returns.get_mut(&kind) {
            receipt.report.creation = cases.into_iter().flatten().collect();
        }
        Ok(())
    }

    /// Export must observe both cancellation identities. A sticky spent ledger
    /// cannot be revived by a successful zero-sized charge after a prior refusal.
    pub(super) fn regional_checkpoint(&mut self) -> Result<(), MachineError> {
        MachineError::check_cancelled()?;
        if self.budget.cancellation().is_cancelled() {
            return Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled,
            ));
        }
        if self.budget.exhausted() {
            return Err(MachineError::work_failure());
        }
        self.reserve(0)
    }

    pub(super) fn regional_cfg(&mut self) -> Result<EvmRegionalCfg, MachineError> {
        self.regional_checkpoint()?;
        let node_work = self.table.nodes.iter().fold(1usize, |work, node| {
            work.saturating_add(32)
                .saturating_add(node.payload.active().key.jump_history.len())
                .saturating_add(node.successors.len())
                .saturating_add(
                    node.transfer
                        .as_ref()
                        .map_or(0, EvmRegionalTransfer::work_size),
                )
        });
        let work = self
            .table
            .subscribers
            .iter()
            .fold(node_work, |work, subscriber| {
                subscriber.returns.values().fold(
                    work.saturating_add(8)
                        .saturating_add(self.table.procedures[subscriber.callee].entries.len()),
                    |work, receipt| work.saturating_add(receipt.work_size()),
                )
            });
        #[cfg(test)]
        cancel_after_export_census(self.budget);
        self.reserve(work.saturating_mul(4))?;
        let table = &self.table;
        let cfg = self.runtime.with_run(
            self.active
                .as_mut()
                .or(self.caller.as_deref_mut())
                .ok_or(MachineError::Shape)?,
            self.budget,
            |run| {
                // Untracked version reads do not subscribe the outer World job
                // to every body. Existing semantic slots own dependency wakes.
                let nodes = table
                    .nodes
                    .iter()
                    .enumerate()
                    .map(|(id, node)| {
                        let input = run
                            .slot_version(&node.input)
                            .map_err(super::kernel_failure)?;
                        Ok(EvmRegionalNode {
                            id,
                            procedure: node.procedure,
                            frame: node.payload.active().key.clone(),
                            input_revision: input.get(),
                            excluded: node.excluded,
                            successors: node.successors.iter().copied().collect(),
                            transfer: node.transfer.as_ref().map(|receipt| {
                                // Foreign premise retirement need not change
                                // native versions. Completion is insufficient
                                // without an explicit foreign-source guard.
                                receipt.snapshot(input, node.excluded, !X::ENABLED)
                            }),
                        })
                    })
                    .collect::<Result<Vec<_>, MachineError>>()?;
                let calls = table
                    .subscribers
                    .iter()
                    .enumerate()
                    .map(|(id, subscriber)| {
                        let input = run
                            .slot_version(&subscriber.input)
                            .map_err(super::kernel_failure)?;
                        let callee = &table.procedures[subscriber.callee];
                        let deliveries = subscriber
                            .returns
                            .iter()
                            .filter_map(|(kind, receipt)| {
                                receipt.snapshot(
                                    input,
                                    callee.outputs.get(kind).map(|output| output.generation),
                                    subscriber.delivered.get(kind).copied(),
                                    !X::ENABLED,
                                )
                            })
                            .collect();
                        Ok(EvmRegionalCall {
                            scope: subscriber
                                .action
                                .map(|_| crate::analysis::ProcessControlScope::Primitive),
                            action: subscriber.action.map(|(action, _)| action),
                            target: subscriber.action.map(|(_, target)| target),
                            id,
                            from: subscriber.from,
                            callee: subscriber.callee,
                            entries: callee.entries.iter().copied().collect(),
                            input_revision: input.get(),
                            deliveries,
                        })
                    })
                    .collect::<Result<Vec<_>, MachineError>>()?;
                Ok(EvmRegionalCfg { nodes, calls })
            },
        )?;
        // The result owns only prepaid keys/IDs/receipts. A cancellation during
        // construction discards it rather than publishing current evidence.
        self.regional_checkpoint()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests;
