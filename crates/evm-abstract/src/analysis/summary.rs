//! Exact-precondition call summaries with an immutable instruction-graph certificate.
//!
//! A summary describes a complete callee computation, including nested calls and
//! rollback. Suspended callers and the outer output-copy continuation are not
//! semantic callee inputs; all other frame, environment and store facts are.
//! Certification uses the ordinary shared worklist rather than running a second
//! executor. Reuse materializes the certified graph for the new caller so the
//! instruction graph and world SSA retain every call and return boundary.

use crate::analysis::engine::policy::{PolicyKey, key_checkpoint};
use crate::domain::WordElement;

use super::{
    Diagnostic, EvmCallStack, EvmInstructionValue, EvmMachinePayload, EvmRootFrame,
    EvmWorldAnalysis, FrontierReason, InstructionProgress, MachineEdge, OutcomeKind,
    transfer::{EvmCompletedCall, WorkBudget},
    worklist::runtime::Runtime,
};
use crate::{
    Fork,
    world::{EvmByteArray, EvmEnvironmentValues, EvmStore, SnapshotIdentity},
};
use alloy_primitives::{B256, keccak256};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[cfg(test)]
mod cancellation;
#[cfg(test)]
mod capture_tests;
mod replay;
#[cfg(test)]
pub(super) use replay::Prepared;
pub(super) use replay::{EvmPrepared, EvmReplay};

fn reserve(budget: &mut WorkBudget, cost: usize) -> bool {
    key_checkpoint(budget).is_ok()
        && cost != usize::MAX
        && budget.charge(cost)
        && key_checkpoint(budget).is_ok()
}

pub(super) fn summary_boundary(budget: &WorkBudget) -> FrontierReason {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
    } else {
        FrontierReason::SummaryWork
    }
}

/// Complete exact preconditions for a reusable callee relation.
///
/// Equality, rather than a hash-only lookup or an inferred read set, is the
/// reuse guard. The complete store includes code overlays, nonce, lifecycle,
/// transient state, balances and logs. Consequently changes to any of these
/// observations produce a miss. `EvmCache<N, Tag>` ownership is local to one fixed world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmSummaryInput<N: WordElement> {
    /// Fork under which all runtime and initcode instructions were decoded.
    pub fork: Fork,
    /// Typed offline or block-hash-pinned snapshot identity.
    pub snapshot: SnapshotIdentity,
    /// Binds all initial account/code observations, including offline snapshots.
    pub world_fingerprint: B256,
    /// Hash of the current executable account bytes, after code overlay changes.
    pub code_hash: Option<B256>,
    /// Callee state represented as the root of its relative certificate stack.
    pub frame: EvmRootFrame<N>,
    /// Complete transaction store on entry, including rollback preconditions.
    pub store: EvmStore<N>,
    /// Complete immutable transaction and block environment.
    pub environment: EvmEnvironmentValues<N>,
    /// Complete caller-qualified constraints on input expressions.
    pub relations: crate::domain::relational::RelationState,
    /// Available additional call depth; recursion cannot gain a fresh budget.
    pub remaining_call_depth: usize,
    /// Transaction-wide external-call depth policy used during certification.
    pub max_call_depth: usize,
    /// Intraprocedural jump-history precision policy.
    pub context_depth: usize,
    /// Frozen component, exchange, provenance and cost policy.
    pub domain_spec: crate::domain::DomainSpec,
    /// Memory/range modeling policy.
    pub max_memory_bytes: usize,
}

impl<N: WordElement> EvmSummaryInput<N> {
    pub(super) fn work_size(&self) -> usize {
        self.frame
            .state
            .work_size()
            .saturating_add(self.store.work_size())
            .saturating_add(self.environment.work_size())
            .saturating_add(self.relations.work_size())
            .saturating_add(1)
    }
}

/// One terminal possibility in a complete abstract-input-to-effects relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmSummaryOutput<N: WordElement> {
    /// Child completion category; revert and failure restore its saved store.
    pub kind: OutcomeKind,
    /// Full child returndata, before the caller's requested-prefix copy.
    pub data: EvmByteArray<N>,
    /// Committed or rolled-back transaction state before caller resumption.
    pub store: EvmStore<N>,
}

/// Public evidence for an immutable, fully closed call summary.
#[derive(Clone, Debug, Serialize)]
pub struct EvmSummaryRecord<N: WordElement> {
    /// `EvmEntry<N>` state from which this complete certificate was obtained.
    pub source_state: usize,
    /// Exact input and immutable world/code binding.
    pub input: EvmSummaryInput<N>,
    /// Return, revert and failure possibilities, including store effects.
    pub outputs: Vec<EvmSummaryOutput<N>>,
    /// Number of child-relative machine states in its graph certificate.
    pub state_count: usize,
    /// Internal and terminal edges covered by its graph certificate.
    pub edge_count: usize,
    /// Other callee-entry states that instantiated this certificate.
    pub reused_at: Vec<usize>,
}

/// Observable cache behavior; a hit always means complete relation reuse.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SummaryStats {
    /// Exact-precondition matches whose graph was selected for reuse.
    pub hits: usize,
    /// Callee entry attempts without a matching complete certificate.
    pub misses: usize,
    /// Complete immutable relations published after worklist closure.
    pub published: usize,
    /// Candidates whose input changed or whose complete graph could not be certified.
    pub rejected_incomplete: usize,
    /// Child graph states materialized from completed certificates.
    pub imported_states: usize,
}

#[derive(Clone)]
pub(super) struct EvmNode<N: WordElement, Tag: PolicyKey = Option<bool>> {
    pub entry: EvmMachinePayload<N>,
    pub resume_status: Option<bool>,
    pub partition: Tag,
    pub exit: EvmMachinePayload<N>,
    pub exit_stack: Vec<crate::domain::EvmValue<N>>,
    pub executed_pcs: Vec<usize>,
    pub instruction_progress: Vec<InstructionProgress>,
    pub instruction_values: Arc<[EvmInstructionValue<N>]>,
    pub diagnostics: Vec<Diagnostic>,
    pub completed_calls: Vec<EvmCompletedCall<N>>,
}

impl<N: WordElement, Tag: PolicyKey> EvmNode<N, Tag> {
    pub fn work_size(&self) -> usize {
        let cost = self
            .entry
            .work_size()
            .saturating_add(self.exit.work_size())
            .saturating_add(self.executed_pcs.len())
            .saturating_add(self.instruction_progress.len())
            .saturating_add(self.diagnostics.len())
            .saturating_add(self.partition.key_work());
        let cost = self
            .exit_stack
            .iter()
            .fold(cost, |cost, value| cost.saturating_add(value.work_size()));
        let cost = self.instruction_values.iter().fold(cost, |cost, value| {
            cost.saturating_add(value.numeric().work_size())
                .saturating_add(value.provenance().origins().work_size())
        });
        self.completed_calls
            .iter()
            .fold(cost, |cost, call| cost.saturating_add(call_work(call)))
    }
}

pub(super) fn call_work<N: WordElement>(call: &EvmCompletedCall<N>) -> usize {
    call.payload
        .work_size()
        .saturating_add(call.data.work_size())
        .saturating_add(call.output_data.work_size())
        .saturating_add(call.output_store.work_size())
}

#[derive(Clone)]
pub(super) struct EvmTerminal<N: WordElement> {
    pub from: usize,
    pub output: EvmSummaryOutput<N>,
    pub payload: EvmMachinePayload<N>,
    pub raw_kind: OutcomeKind,
    pub raw_data: EvmByteArray<N>,
}

impl<N: WordElement> EvmTerminal<N> {
    pub fn work_size(&self) -> usize {
        self.payload
            .work_size()
            .saturating_add(self.raw_data.work_size())
            .saturating_add(self.output.data.work_size())
            .saturating_add(self.output.store.work_size())
            .saturating_add(1)
    }
}

#[derive(Clone)]
pub(super) struct EvmCertificate<N: WordElement, Tag: PolicyKey = Option<bool>> {
    pub record: usize,
    pub nodes: Vec<EvmNode<N, Tag>>,
    pub edges: Vec<MachineEdge>,
    pub terminals: Vec<EvmTerminal<N>>,
}

struct EvmCandidate<N: WordElement> {
    state: usize,
    input: EvmSummaryInput<N>,
}

pub(super) struct EvmCache<N: WordElement, Tag: PolicyKey = Option<bool>> {
    fingerprint: Option<B256>,
    fingerprint_unavailable: bool,
    pub call_entries: BTreeSet<usize>,
    candidates: Vec<EvmCandidate<N>>,
    certificates: Vec<EvmCertificate<N, Tag>>,
    stop_reason: Option<FrontierReason>,
}

impl<N: WordElement, Tag: PolicyKey> EvmCache<N, Tag> {
    pub fn new() -> Self {
        Self {
            fingerprint: None,
            fingerprint_unavailable: false,
            call_entries: BTreeSet::new(),
            candidates: Vec::new(),
            certificates: Vec::new(),
            stop_reason: None,
        }
    }

    pub fn stop_reason(&self) -> Option<FrontierReason> {
        self.stop_reason.clone()
    }

    fn stop(&mut self, reason: FrontierReason) -> bool {
        if self.stop_reason.is_none() {
            self.stop_reason = Some(reason);
        }
        false
    }

    pub fn input(
        &mut self,
        analysis: &EvmWorldAnalysis<N>,
        state: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<EvmSummaryInput<N>>, FrontierReason> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(summary_boundary(budget));
        }
        if self.fingerprint_unavailable {
            return Ok(None);
        }
        let payload = &analysis.states[state].entry;
        let bytes = payload
            .store
            .code(payload.active().key.code_address)
            .map_or(0, |code| match code {
                crate::world::Code::Runtime(program) => program.byte_len(),
                crate::world::Code::Delegation(_) => 23,
                _ => 0,
            });
        if !reserve(
            budget,
            payload
                .work_size()
                .saturating_add(bytes)
                .saturating_add(analysis.entry.environment.work_size())
                .saturating_add(1),
        ) {
            return Err(summary_boundary(budget));
        }
        let fingerprint = if let Some(fingerprint) = self.fingerprint {
            fingerprint
        } else {
            // Snapshot hashing is lazy, so analyses without external calls
            // incur no summary work. Include fixed-width account/value encoding
            // before allocating or hashing the immutable world namespace.
            let namespace_cost = analysis.states[0]
                .entry
                .store
                .work_size()
                .saturating_mul(32)
                .saturating_add(analysis.world.accounts().len().saturating_mul(64))
                .saturating_add(analysis.world.provenance().len());
            if !reserve(budget, namespace_cost) {
                return Err(summary_boundary(budget));
            }
            let fingerprint = match analysis.world.fingerprint() {
                Ok(fingerprint) => fingerprint,
                Err(crate::world::FingerprintError::Cancelled) => {
                    return Err(FrontierReason::Relations(
                        crate::domain::relational::QueryReason::Cancelled,
                    ));
                }
                Err(
                    crate::world::FingerprintError::Encoding(_)
                    | crate::world::FingerprintError::LengthOverflow,
                ) => {
                    // Ordinary execution needs no summary namespace. A failed
                    // optional encoding never supplies a guessed cache key.
                    self.fingerprint_unavailable = true;
                    return Ok(None);
                }
            };
            self.fingerprint = Some(fingerprint);
            fingerprint
        };
        let (frame, _) = payload
            .call_stack
            .active_child()
            .expect("summary inputs start at child frames")
            .clone()
            .into_root();
        Ok(Some(EvmSummaryInput {
            fork: analysis.world.fork(),
            snapshot: analysis.world.identity().clone(),
            world_fingerprint: fingerprint,
            code_hash: payload
                .store
                .raw_account_code(frame.state.key.code_address)
                .map(keccak256),
            frame,
            store: payload.store.clone(),
            environment: analysis.entry.environment.clone(),
            relations: payload.relations.clone(),
            remaining_call_depth: analysis
                .config
                .max_call_depth
                .saturating_sub(payload.call_stack.depth()),
            max_call_depth: analysis.config.max_call_depth,
            context_depth: analysis.config.analysis.context_depth,
            domain_spec: analysis.domain_spec(),
            max_memory_bytes: analysis.config.max_memory_bytes,
        }))
    }

    pub fn lookup(
        &self,
        input: &EvmSummaryInput<N>,
        analysis: &EvmWorldAnalysis<N>,
        budget: &mut WorkBudget,
    ) -> Result<Option<usize>, ()> {
        for (index, certificate) in self.certificates.iter().enumerate() {
            let record = &analysis.summaries[certificate.record];
            // Charge equality comparisons too: scanning an exact-input cache
            // must not hide unbounded domain/store work outside the ledger.
            if !reserve(
                budget,
                input.work_size().saturating_add(record.input.work_size()),
            ) {
                return Err(());
            }
            if input == &record.input {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub fn begin(
        &mut self,
        state: usize,
        input: EvmSummaryInput<N>,
        budget: &mut WorkBudget,
    ) -> bool {
        if !reserve(budget, input.work_size()) {
            return false;
        }
        for candidate in &self.candidates {
            if !reserve(budget, 1) {
                return false;
            }
            if candidate.state == state {
                if !reserve(
                    budget,
                    candidate
                        .input
                        .work_size()
                        .saturating_add(input.work_size()),
                ) {
                    return false;
                }
                if candidate.input == input {
                    return true;
                }
            }
        }
        self.candidates.push(EvmCandidate { state, input });
        true
    }

    pub fn certificate(&self, index: usize) -> &EvmCertificate<N, Tag> {
        &self.certificates[index]
    }

    pub fn pending_target(&self, analysis: &EvmWorldAnalysis<N>) -> Option<super::MachineKey> {
        self.candidates
            .first()
            .map(|candidate| analysis.states[candidate.state].key.clone())
    }

    /// Check closure against the same queue and frontiers as normal execution.
    /// A changed source input is never certified under its older precondition.
    #[expect(
        clippy::too_many_arguments,
        reason = "whole graph, precision partitions and shared ledger"
    )]
    pub fn publish_closed<T>(
        &mut self,
        analysis: &mut EvmWorldAnalysis<N>,
        edges: &BTreeSet<MachineEdge>,
        worklist: &Runtime<T>,
        diagnostics: &BTreeSet<Diagnostic>,
        completed_calls: &[Vec<EvmCompletedCall<N>>],
        partitions: &[Tag],
        budget: &mut WorkBudget,
    ) -> bool {
        let candidates = std::mem::take(&mut self.candidates);
        let mut iter = candidates.into_iter();
        while let Some(candidate) = iter.next() {
            let current = match self.input(analysis, candidate.state, budget) {
                Ok(Some(current)) => current,
                Ok(None) => {
                    analysis.summary_stats.rejected_incomplete += 1;
                    continue;
                }
                Err(reason) => {
                    self.stop_reason = Some(reason);
                    self.candidates.push(candidate);
                    self.candidates.extend(iter);
                    return self.stop(summary_boundary(budget));
                }
            };
            if !reserve(
                budget,
                current
                    .work_size()
                    .saturating_add(candidate.input.work_size()),
            ) {
                self.candidates.push(candidate);
                self.candidates.extend(iter);
                return self.stop(summary_boundary(budget));
            }
            if current != candidate.input {
                analysis.summary_stats.rejected_incomplete += 1;
                continue;
            }
            match capture(
                analysis,
                edges,
                worklist,
                diagnostics,
                completed_calls,
                partitions,
                &candidate,
                budget,
            ) {
                EvmCapture::Pending => self.candidates.push(candidate),
                EvmCapture::Uncertifiable => {
                    analysis.summary_stats.rejected_incomplete += 1;
                }
                EvmCapture::Stopped(reason) => {
                    self.stop_reason = Some(reason);
                    self.candidates.push(candidate);
                    self.candidates.extend(iter);
                    return self.stop(summary_boundary(budget));
                }
                EvmCapture::Complete(mut certificate) => {
                    // Another call may have closed the same exact relation.
                    // Keep one immutable certificate and avoid widening outputs.
                    let mut duplicate = false;
                    for record in &analysis.summaries {
                        if !reserve(
                            budget,
                            record
                                .input
                                .work_size()
                                .saturating_add(candidate.input.work_size()),
                        ) {
                            self.candidates.push(candidate);
                            self.candidates.extend(iter);
                            return self.stop(summary_boundary(budget));
                        }
                        if record.input == candidate.input {
                            duplicate = true;
                            break;
                        }
                    }
                    if duplicate {
                        continue;
                    }
                    let outputs_work =
                        certificate.terminals.iter().fold(1usize, |cost, terminal| {
                            cost.saturating_add(terminal.output.data.work_size())
                                .saturating_add(terminal.output.store.work_size())
                        });
                    if !reserve(budget, outputs_work) {
                        self.candidates.push(candidate);
                        self.candidates.extend(iter);
                        return self.stop(summary_boundary(budget));
                    }
                    let record = analysis.summaries.len();
                    analysis.summaries.push(EvmSummaryRecord {
                        source_state: candidate.state,
                        input: candidate.input,
                        outputs: certificate
                            .terminals
                            .iter()
                            .map(|terminal| terminal.output.clone())
                            .collect(),
                        state_count: certificate.nodes.len(),
                        edge_count: certificate
                            .edges
                            .len()
                            .saturating_add(certificate.terminals.len()),
                        reused_at: Vec::new(),
                    });
                    certificate.record = record;
                    self.certificates.push(certificate);
                    analysis.summary_stats.published += 1;
                }
            }
        }
        true
    }

    pub fn finish(self, analysis: &mut EvmWorldAnalysis<N>) {
        analysis.summary_stats.rejected_incomplete += self.candidates.len();
    }
}

enum EvmCapture<N: WordElement, Tag: PolicyKey = Option<bool>> {
    Pending,
    Stopped(FrontierReason),
    Uncertifiable,
    Complete(EvmCertificate<N, Tag>),
}

fn relative<N: WordElement>(payload: &EvmMachinePayload<N>, prefix: usize) -> EvmMachinePayload<N> {
    if prefix == 0 {
        return payload.clone();
    }
    let children = payload.call_stack.children();
    let (root, _) = children[prefix - 1].clone().into_root();
    EvmMachinePayload {
        relations: payload.relations.clone(),
        call_stack: EvmCallStack::from_parts(root, children[prefix..].to_vec()),
        store: payload.store.clone(),
    }
}

fn relative_completed_call<N: WordElement>(
    call: &EvmCompletedCall<N>,
    prefix: usize,
) -> EvmCompletedCall<N> {
    // Preserve the surviving copies' order. Cloning the whole call first would
    // copy its absolute payload only to discard it after relative projection.
    let data = call.data.clone();
    let output_store = call.output_store.clone();
    let output_data = call.output_data.clone();
    let payload = relative(&call.payload, prefix);
    EvmCompletedCall {
        payload,
        kind: call.kind,
        data,
        output_store,
        output_kind: call.output_kind,
        output_data,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "complete certificate evidence and precision partition"
)]
fn capture<N: WordElement, Tag: PolicyKey, T>(
    analysis: &EvmWorldAnalysis<N>,
    edges: &BTreeSet<MachineEdge>,
    worklist: &Runtime<T>,
    diagnostics: &BTreeSet<Diagnostic>,
    completed_calls: &[Vec<EvmCompletedCall<N>>],
    partitions: &[Tag],
    candidate: &EvmCandidate<N>,
    budget: &mut WorkBudget,
) -> EvmCapture<N, Tag> {
    let depth = analysis.states[candidate.state].entry.call_stack.depth();
    let mut queue = VecDeque::from([candidate.state]);
    let mut reached = BTreeSet::new();
    let mut internal = Vec::new();
    let mut terminal_states = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !reached.insert(id) {
            continue;
        }
        if !reserve(
            budget,
            edges
                .len()
                .saturating_add(analysis.frontiers.len())
                .saturating_add(1),
        ) {
            return EvmCapture::Stopped(summary_boundary(budget));
        }
        if worklist.contains_world(&id)
            || analysis.states[id].exit.is_none()
            || analysis.states[id]
                .execution_evidence()
                .is_none_or(|evidence| !evidence.is_current())
            || analysis
                .frontiers
                .iter()
                .any(|frontier| frontier.from == Some(id))
        {
            return EvmCapture::Pending;
        }
        for edge in edges.iter().filter(|edge| edge.from == id) {
            if matches!(edge.kind, super::MachineEdgeKind::Summary(_)) {
                // The exact graph replay renames node IDs, but has no binding
                // for a relative procedure's source_state, root and effects.
                // Replaying this graph would erase its abstraction provenance.
                // Keep ordinary execution and let the shared procedure table
                // supply a fresh, correctly qualified summary at the new site.
                return EvmCapture::Uncertifiable;
            }
            if analysis.states[edge.to].entry.call_stack.depth() >= depth {
                internal.push(edge.clone());
                queue.push_back(edge.to);
            } else {
                terminal_states.insert(id);
            }
        }
    }
    // EvmEntry<N> first makes the certificate portable without an extra root map.
    let ordered: Vec<_> = std::iter::once(candidate.state)
        .chain(reached.iter().copied().filter(|id| *id != candidate.state))
        .collect();
    let ids: BTreeMap<_, _> = ordered
        .iter()
        .enumerate()
        .map(|(local, id)| (*id, local))
        .collect();
    let cost = ordered.iter().fold(
        internal.len().saturating_add(terminal_states.len()),
        |cost, id| {
            let state = &analysis.states[*id];
            let cost = cost
                .saturating_add(state.entry.work_size())
                .saturating_add(state.exit.as_ref().map_or(0, EvmMachinePayload::work_size))
                .saturating_add(state.executed_pcs.len())
                .saturating_add(partitions[*id].key_work())
                .saturating_add(diagnostics.len())
                .saturating_add(
                    state
                        .exit_stack
                        .iter()
                        .fold(0usize, |cost, value| cost.saturating_add(value.work_size())),
                )
                .saturating_add(state.execution_evidence().map_or(0, |evidence| {
                    evidence.instructions().len().saturating_add(
                        evidence.instruction_values().map_or(0, |values| {
                            values.iter().fold(0usize, |cost, value| {
                                cost.saturating_add(value.numeric().work_size())
                                    .saturating_add(value.provenance().origins().work_size())
                            })
                        }),
                    )
                }));
            completed_calls[*id].iter().fold(cost, |cost, call| {
                cost.saturating_add(call_work(call).saturating_mul(4))
            })
        },
    );
    if !reserve(budget, cost) {
        return EvmCapture::Stopped(summary_boundary(budget));
    }
    let mut nodes = Vec::new();
    for id in &ordered {
        if !reserve(budget, 0) {
            return EvmCapture::Stopped(summary_boundary(budget));
        }
        // The complete capture reservation above owns this class clone.
        let partition = partitions[*id].clone();
        if !reserve(budget, 0) {
            return EvmCapture::Stopped(summary_boundary(budget));
        }
        let state = &analysis.states[*id];
        nodes.push(EvmNode {
            entry: relative(&state.entry, depth - 1),
            resume_status: state.resume_status,
            partition,
            exit: relative(
                state.exit.as_ref().expect("closure checked exit"),
                depth - 1,
            ),
            exit_stack: state.exit_stack.clone(),
            executed_pcs: state.executed_pcs.clone(),
            instruction_progress: state
                .execution_evidence()
                .expect("closed node has execution evidence")
                .instructions()
                .to_vec(),
            instruction_values: Arc::clone(
                &state
                    .execution_evidence()
                    .expect("closed node has execution evidence")
                    .instruction_values,
            ),
            diagnostics: diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.state == *id)
                .cloned()
                .collect(),
            completed_calls: completed_calls[*id]
                .iter()
                .map(|call| relative_completed_call(call, depth - 1))
                .collect(),
        });
    }
    let edges = internal
        .into_iter()
        .map(|edge| MachineEdge {
            from: ids[&edge.from],
            to: ids[&edge.to],
            kind: edge.kind,
        })
        .collect();
    let mut terminals = Vec::new();
    for state in terminal_states {
        for call in completed_calls[state]
            .iter()
            .filter(|call| call.payload.call_stack.depth() == depth)
        {
            terminals.push(EvmTerminal {
                from: ids[&state],
                payload: relative(&call.payload, depth - 1),
                raw_kind: call.kind,
                raw_data: call.data.clone(),
                output: EvmSummaryOutput {
                    kind: call.output_kind,
                    data: call.output_data.clone(),
                    store: call.output_store.clone(),
                },
            });
        }
    }
    EvmCapture::Complete(EvmCertificate {
        record: 0,
        nodes,
        edges,
        terminals,
    })
}

// Local regression fixtures use explicit built-in carriers.
#[cfg(test)]
pub(in crate::analysis) type SummaryOutput = EvmSummaryOutput<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type Node = EvmNode<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type Terminal = EvmTerminal<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type Certificate = EvmCertificate<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type Cache = EvmCache<crate::domain::NumericValue>;
