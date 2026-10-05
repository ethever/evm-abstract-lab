//! Exact-precondition call summaries with an immutable instruction-graph certificate.
//!
//! A summary describes a complete callee computation, including nested calls and
//! rollback. Suspended callers and the outer output-copy continuation are not
//! semantic callee inputs; all other frame, environment and store facts are.
//! Certification uses the ordinary shared worklist rather than running a second
//! executor. Reuse materializes the certified graph for the new caller so the
//! instruction graph and world SSA retain every call and return boundary.

use super::{
    CallStack, Diagnostic, MachineEdge, MachinePayload, OutcomeKind, RootFrame, WorldAnalysis,
    transfer::{CompletedCall, WorkBudget},
};
use crate::{
    Fork,
    world::{ByteArray, SnapshotIdentity, Store},
};
use alloy_primitives::{Address, B256, keccak256};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

mod replay;
pub(super) use replay::Replay;

/// Complete exact preconditions for a reusable callee relation.
///
/// Equality, rather than a hash-only lookup or an inferred read set, is the
/// reuse guard. The complete store includes code overlays, nonce, lifecycle,
/// transient state, balances and logs. Consequently changes to any of these
/// observations produce a miss. Cache ownership is local to one fixed world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SummaryInput {
    /// Fork under which all runtime and initcode instructions were decoded.
    pub fork: Fork,
    /// Typed offline or block-hash-pinned snapshot identity.
    pub snapshot: SnapshotIdentity,
    /// Binds all initial account/code observations, including offline snapshots.
    pub world_fingerprint: B256,
    /// Hash of the current executable account bytes, after code overlay changes.
    pub code_hash: Option<B256>,
    /// Callee state represented as the root of its relative certificate stack.
    pub frame: RootFrame,
    /// Complete transaction store on entry, including rollback preconditions.
    pub store: Store,
    /// Transaction ORIGIN, shared by all nested frames.
    pub origin: Address,
    /// Available additional call depth; recursion cannot gain a fresh budget.
    pub remaining_call_depth: usize,
    /// Transaction-wide external-call depth policy used during certification.
    pub max_call_depth: usize,
    /// Intraprocedural jump-history precision policy.
    pub context_depth: usize,
    /// Finite-domain capacity used to compute the relation.
    pub max_constants: usize,
    /// Memory/range modeling policy.
    pub max_memory_bytes: usize,
    /// Whether the root transaction environment is intentionally symbolic.
    pub symbolic_entry_environment: bool,
}

impl SummaryInput {
    fn work_size(&self) -> usize {
        let frame = &self.frame.state;
        frame.stack.iter().fold(
            self.store
                .work_size()
                .saturating_add(frame.memory.work_size())
                .saturating_add(frame.calldata.work_size())
                .saturating_add(frame.returndata.work_size())
                .saturating_add(frame.saved_store.state().work_size())
                .saturating_add(
                    frame
                        .program
                        .as_ref()
                        .map_or(0, crate::bytecode::Program::byte_len),
                )
                .saturating_add(
                    frame
                        .call_value
                        .constants()
                        .map_or(1, |values| values.len()),
                )
                .saturating_add(1),
            |cost, value| cost.saturating_add(value.constants().map_or(1, |values| values.len())),
        )
    }
}

/// One terminal possibility in a complete abstract-input-to-effects relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SummaryOutput {
    /// Child completion category; revert and failure restore its saved store.
    pub kind: OutcomeKind,
    /// Full child returndata, before the caller's requested-prefix copy.
    pub data: ByteArray,
    /// Committed or rolled-back transaction state before caller resumption.
    pub store: Store,
}

/// Public evidence for an immutable, fully closed call summary.
#[derive(Clone, Debug, Serialize)]
pub struct SummaryRecord {
    /// Entry state from which this complete certificate was obtained.
    pub source_state: usize,
    /// Exact input and immutable world/code binding.
    pub input: SummaryInput,
    /// Return, revert and failure possibilities, including store effects.
    pub outputs: Vec<SummaryOutput>,
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
    /// Candidates whose input changed or whose graph never fully closed.
    pub rejected_incomplete: usize,
    /// Child graph states materialized from completed certificates.
    pub imported_states: usize,
}

#[derive(Clone)]
pub(super) struct Node {
    pub entry: MachinePayload,
    pub exit: MachinePayload,
    pub exit_stack: Vec<crate::domain::Value>,
    pub executed_pcs: Vec<usize>,
    pub diagnostics: Vec<Diagnostic>,
    pub completed_calls: Vec<CompletedCall>,
}

#[derive(Clone)]
pub(super) struct Terminal {
    pub from: usize,
    pub output: SummaryOutput,
    pub payload: MachinePayload,
    pub raw_kind: OutcomeKind,
    pub raw_data: ByteArray,
}

#[derive(Clone)]
pub(super) struct Certificate {
    pub record: usize,
    pub nodes: Vec<Node>,
    pub edges: Vec<MachineEdge>,
    pub terminals: Vec<Terminal>,
}

struct Candidate {
    state: usize,
    input: SummaryInput,
}

pub(super) struct Cache {
    fingerprint: Option<B256>,
    pub call_entries: BTreeSet<usize>,
    candidates: Vec<Candidate>,
    certificates: Vec<Certificate>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            fingerprint: None,
            call_entries: BTreeSet::new(),
            candidates: Vec::new(),
            certificates: Vec::new(),
        }
    }

    pub fn input(
        &mut self,
        analysis: &WorldAnalysis,
        state: usize,
        budget: &mut WorkBudget,
    ) -> Option<SummaryInput> {
        let payload = &analysis.states[state].entry;
        let bytes = payload
            .store
            .code(payload.active().key.code_address)
            .map_or(0, |code| match code {
                crate::world::Code::Runtime(program) => program.byte_len(),
                crate::world::Code::Delegation(_) => 23,
                _ => 0,
            });
        if !budget.charge(payload.work_size().saturating_add(bytes).saturating_add(1)) {
            return None;
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
            if !budget.charge(namespace_cost) {
                return None;
            }
            let fingerprint = analysis.world.fingerprint();
            self.fingerprint = Some(fingerprint);
            fingerprint
        };
        let (frame, _) = payload
            .call_stack
            .active_child()
            .expect("summary inputs start at child frames")
            .clone()
            .into_root();
        Some(SummaryInput {
            fork: analysis.world.fork(),
            snapshot: analysis.world.identity().clone(),
            world_fingerprint: fingerprint,
            code_hash: payload
                .store
                .raw_account_code(frame.state.key.code_address)
                .map(keccak256),
            frame,
            store: payload.store.clone(),
            origin: analysis.entry.caller,
            remaining_call_depth: analysis
                .config
                .max_call_depth
                .saturating_sub(payload.call_stack.depth()),
            max_call_depth: analysis.config.max_call_depth,
            context_depth: analysis.config.analysis.context_depth,
            max_constants: analysis.config.analysis.max_constants,
            max_memory_bytes: analysis.config.max_memory_bytes,
            symbolic_entry_environment: analysis.config.symbolic_entry_environment,
        })
    }

    pub fn lookup(
        &self,
        input: &SummaryInput,
        analysis: &WorldAnalysis,
        budget: &mut WorkBudget,
    ) -> Result<Option<usize>, ()> {
        for (index, certificate) in self.certificates.iter().enumerate() {
            let record = &analysis.summaries[certificate.record];
            // Charge equality comparisons too: scanning an exact-input cache
            // must not hide unbounded domain/store work outside the ledger.
            if !budget.charge(input.work_size().saturating_add(record.input.work_size())) {
                return Err(());
            }
            if input == &record.input {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub fn begin(&mut self, state: usize, input: SummaryInput, budget: &mut WorkBudget) -> bool {
        if !budget.charge(self.candidates.len().saturating_add(input.work_size())) {
            return false;
        }
        if !self
            .candidates
            .iter()
            .any(|candidate| candidate.state == state && candidate.input == input)
        {
            self.candidates.push(Candidate { state, input });
        }
        true
    }

    pub fn certificate(&self, index: usize) -> &Certificate {
        &self.certificates[index]
    }

    pub fn pending_target(&self, analysis: &WorldAnalysis) -> Option<super::MachineKey> {
        self.candidates
            .first()
            .map(|candidate| analysis.states[candidate.state].key.clone())
    }

    /// Check closure against the same queue and frontiers as normal execution.
    /// A changed source input is never certified under its older precondition.
    pub fn publish_closed(
        &mut self,
        analysis: &mut WorldAnalysis,
        edges: &BTreeSet<MachineEdge>,
        queued: &BTreeSet<usize>,
        diagnostics: &BTreeSet<Diagnostic>,
        completed_calls: &[Vec<CompletedCall>],
        budget: &mut WorkBudget,
    ) -> bool {
        let candidates = std::mem::take(&mut self.candidates);
        let mut iter = candidates.into_iter();
        while let Some(candidate) = iter.next() {
            let Some(current) = self.input(analysis, candidate.state, budget) else {
                self.candidates.push(candidate);
                self.candidates.extend(iter);
                return false;
            };
            if current != candidate.input {
                analysis.summary_stats.rejected_incomplete += 1;
                continue;
            }
            match capture(
                analysis,
                edges,
                queued,
                diagnostics,
                completed_calls,
                &candidate,
                budget,
            ) {
                Capture::Pending => self.candidates.push(candidate),
                Capture::Work => {
                    self.candidates.push(candidate);
                    self.candidates.extend(iter);
                    return false;
                }
                Capture::Complete(mut certificate) => {
                    // Another call may have closed the same exact relation.
                    // Keep one immutable certificate and avoid widening outputs.
                    let comparison_cost = analysis.summaries.iter().fold(1usize, |cost, record| {
                        cost.saturating_add(record.input.work_size())
                            .saturating_add(candidate.input.work_size())
                    });
                    if !budget.charge(comparison_cost) {
                        self.candidates.push(candidate);
                        self.candidates.extend(iter);
                        return false;
                    }
                    if analysis
                        .summaries
                        .iter()
                        .any(|record| record.input == candidate.input)
                    {
                        continue;
                    }
                    let record = analysis.summaries.len();
                    analysis.summaries.push(SummaryRecord {
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

    pub fn finish(self, analysis: &mut WorldAnalysis) {
        analysis.summary_stats.rejected_incomplete += self.candidates.len();
    }
}

enum Capture {
    Pending,
    Work,
    Complete(Certificate),
}

fn relative(payload: &MachinePayload, prefix: usize) -> MachinePayload {
    if prefix == 0 {
        return payload.clone();
    }
    let children = payload.call_stack.children();
    let (root, _) = children[prefix - 1].clone().into_root();
    MachinePayload {
        call_stack: CallStack::from_parts(root, children[prefix..].to_vec()),
        store: payload.store.clone(),
    }
}

fn capture(
    analysis: &WorldAnalysis,
    edges: &BTreeSet<MachineEdge>,
    queued: &BTreeSet<usize>,
    diagnostics: &BTreeSet<Diagnostic>,
    completed_calls: &[Vec<CompletedCall>],
    candidate: &Candidate,
    budget: &mut WorkBudget,
) -> Capture {
    let depth = analysis.states[candidate.state].entry.call_stack.depth();
    let mut queue = VecDeque::from([candidate.state]);
    let mut reached = BTreeSet::new();
    let mut internal = Vec::new();
    let mut terminal_states = BTreeSet::new();
    while let Some(id) = queue.pop_front() {
        if !reached.insert(id) {
            continue;
        }
        if !budget.charge(
            edges
                .len()
                .saturating_add(analysis.frontiers.len())
                .saturating_add(1),
        ) {
            return Capture::Work;
        }
        if queued.contains(&id)
            || analysis.states[id].exit.is_none()
            || analysis
                .frontiers
                .iter()
                .any(|frontier| frontier.from == Some(id))
        {
            return Capture::Pending;
        }
        for edge in edges.iter().filter(|edge| edge.from == id) {
            if analysis.states[edge.to].entry.call_stack.depth() >= depth {
                internal.push(edge.clone());
                queue.push_back(edge.to);
            } else {
                terminal_states.insert(id);
            }
        }
    }
    // Entry first makes the certificate portable without an extra root map.
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
                .saturating_add(state.exit.as_ref().map_or(0, MachinePayload::work_size))
                .saturating_add(state.executed_pcs.len());
            completed_calls[*id].iter().fold(cost, |cost, call| {
                cost.saturating_add(call.payload.work_size())
                    .saturating_add(call.data.work_size().saturating_mul(3))
                    .saturating_add(call.output_store.work_size().saturating_mul(3))
            })
        },
    );
    if !budget.charge(cost) {
        return Capture::Work;
    }
    let nodes = ordered
        .iter()
        .map(|id| {
            let state = &analysis.states[*id];
            Node {
                entry: relative(&state.entry, depth - 1),
                exit: relative(
                    state.exit.as_ref().expect("closure checked exit"),
                    depth - 1,
                ),
                exit_stack: state.exit_stack.clone(),
                executed_pcs: state.executed_pcs.clone(),
                diagnostics: diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.state == *id)
                    .cloned()
                    .collect(),
                completed_calls: completed_calls[*id]
                    .iter()
                    .cloned()
                    .map(|mut call| {
                        call.payload = relative(&call.payload, depth - 1);
                        call
                    })
                    .collect(),
            }
        })
        .collect();
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
            terminals.push(Terminal {
                from: ids[&state],
                payload: relative(&call.payload, depth - 1),
                raw_kind: call.kind,
                raw_data: call.data.clone(),
                output: SummaryOutput {
                    kind: call.output_kind,
                    data: call.output_data.clone(),
                    store: call.output_store.clone(),
                },
            });
        }
    }
    Capture::Complete(Certificate {
        record: 0,
        nodes,
        edges,
        terminals,
    })
}
