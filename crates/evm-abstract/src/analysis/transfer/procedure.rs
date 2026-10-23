//! Caller-independent saturation for known recursion and unknown interactions.
//!
//! A table entry retains numerical bounds and finite stack equalities; recursion uses
//! worklist saturation rather than stack-depth unrolling. Every live block and separate
//! summary invocation gets independent symbolic state roots. A caller subscription keeps
//! its own continuation and rollback checkpoint. Known bytecode, repeated environment
//! interactions, and their mutually recursive calls therefore use the same worklist.
//!
//! Frozen state policies own finite entry/body/subscriber identities, covering
//! relative/output admission and acceleration schedules. Report frame keys do
//! not constrain those identities. Retained feedback metadata publishes through
//! the same native slots as payload growth, including after a consumer settles.

#[cfg(test)]
use crate::analysis::testing::{ByteArray, TestValue, World};

use crate::analysis::engine::policy::{KeyIndex, PolicyKey};
use crate::analysis::engine::{
    ContextPolicy, EnginePolicies, PartitionPolicy, ProcedureOutput, StatePolicy,
};
use crate::analysis::{
    cooperation::InputTasks,
    engine::cooperation::{Composition, Cooperation},
};
use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    EvmExecution, EvmExecutionFrontier, EvmSuccessor, StorageReadPolicy, TransferContext,
    WorkBudget, boundary_on, calls, execute_inner as execute_inner_with, resume_summary_inner,
    zero,
};
use crate::{
    analysis::{
        DiagnosticKind, EvmCallStack, EvmChildFrame, EvmContinuation, EvmFrameState,
        EvmMachinePayload, EvmRootFrame, FrameCode, FrameKey, FrontierReason, Limit,
        MachineEdgeKind, MachineError, OutcomeKind,
        engine::Counters,
        machine::{
            EvmProcedureSummary, EvmProcedureSummaryOutput, EvmProcedureSummaryRecord,
            ProcedureSummaryKind,
        },
        worklist::{kernel_failure, runtime::Runtime, transfer_stop},
    },
    domain::{AlgebraRef, EvmValue},
    world::{AddressInput, Code, EvmByteArray, EvmByteArraySummary, EvmEntry, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, keccak256};
use analysis_kernel::{Availability, RunningJob, SlotMeta, Version};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod admission;
mod coarse_retry;
mod cooperation;
use cooperation::Producer;
mod cycles;
mod effects;
mod environment;
mod evidence;
pub use evidence::{
    EvmRegionalCall, EvmRegionalCfg, EvmRegionalEdge, EvmRegionalNode, EvmRegionalReturn,
    EvmRegionalTransfer,
};
mod recursion;
mod solver;
#[cfg(test)]
mod tests;

use crate::analysis::state::{
    EvmPayloadSummary,
    payload::{self, instantiate, semantic_eq},
    projection, storage_observations,
};
#[cfg(test)]
use crate::analysis::state::{context, stack_alias};
#[cfg(test)]
pub(in crate::analysis) use coarse_retry::prepare as prepare_runtime_retry;
#[cfg(test)]
use payload::join;
pub(in crate::analysis) use recursion::{is_known_call_entry, is_recursive_entry};

/// This is an internal owner namespace, never an observed account address.
/// All unknown-owner effects are abstracted together, including possible aliases.
pub(super) fn owner_namespace<N: WordElement>(world: &EvmWorld<N>, store: &EvmStore<N>) -> Address {
    let mut bytes = [0xff; 20];
    loop {
        let candidate = Address::from(bytes);
        if world.account(candidate).is_none()
            && !matches!(
                store.code(candidate),
                Some(Code::Runtime(_) | Code::Delegation(_) | Code::Empty)
            )
            && !super::precompile::contains(world.fork(), candidate)
        {
            return candidate;
        }
        for byte in bytes.iter_mut().rev() {
            if *byte != 0 {
                *byte -= 1;
                break;
            }
            *byte = 0xff;
        }
    }
}

struct EvmNode<N: WordElement> {
    procedure: usize,
    payload: EvmPayloadSummary<N>,
    input: SlotMeta<EvmPayloadSummary<N>>,
    updates: usize,
    /// Local control-flow edges, including return to a CALL continuation.
    /// Callee entries stay in the separate procedure dependency graph.
    successors: BTreeSet<usize>,
    loop_head: bool,
    /// Feedback already admitted and published to this body's input readers.
    feedback: bool,
    /// Promotion belongs to the node, across incoming joins and fresh roots.
    runtime_retry: coarse_retry::EvmRuntimeRetry<N>,
    excluded: bool,
    /// Latest actual transfer, separate from accumulated admitted topology.
    transfer: Option<EvmRegionalTransfer>,
}

struct EvmProcedure<N: WordElement, Key: PolicyKey> {
    namespace: usize,
    frame: FrameKey,
    /// Procedure preparation and body preparation may choose different keys.
    /// Retain the actual entry bound rather than borrowing the first body.
    entry_payload: EvmPayloadSummary<N>,
    /// Complete captured-program proof, independent of numerical input growth.
    store_effect: effects::StoreEffect,
    entry_updates: usize,
    entry_feedback: bool,
    entry_revision: usize,
    entry_source: SlotMeta<usize>,
    entries: BTreeSet<usize>,
    // The injected partition owns the complete body identity. The representative
    // frame above is retained solely for reporting.
    nodes: KeyIndex<Key>,
    outputs: BTreeMap<OutcomeKind, EvmOutput<N>>,
    output: SlotMeta<BTreeMap<OutcomeKind, EvmOutput<N>>>,
    dependencies: BTreeSet<usize>,
    recursive: bool,
    local_cycle: bool,
    output_updates: usize,
    covered_blocks: BTreeSet<usize>,
}

/// A procedure's actual input anchor, independently of all selected Body keys.
/// The revision slot changes atomically with the payload and semantic metadata.
pub(in crate::analysis) struct ProcedureEntrySource<'a, N: WordElement> {
    pub namespace: usize,
    pub payload: &'a EvmPayloadSummary<N>,
    pub revision: &'a usize,
    pub slot: &'a SlotMeta<usize>,
}

#[cfg(test)]
impl<N: WordElement, Key: PolicyKey> EvmProcedure<N, Key> {
    fn first_body(&self) -> usize {
        *self.entries.first().expect("admitted procedure has a body")
    }
}

#[derive(Clone)]
struct EvmOutput<N: WordElement> {
    summary: ProcedureOutput<N>,
    work: usize,
    generation: usize,
    /// Semantic content updates, independent of delivery/schedule invalidation.
    updates: usize,
}

struct EvmSubscriber<N: WordElement> {
    from: usize,
    callee: usize,
    /// A relative parent plus one child; each child retains its pre-entry snapshot.
    payload: EvmPayloadSummary<N>,
    input: SlotMeta<EvmPayloadSummary<N>>,
    updates: usize,
    feedback: bool,
    delivered: BTreeMap<OutcomeKind, (Version, usize)>,
    /// Compact continuation targets; pending attempts are not exported as returns.
    returns: BTreeMap<OutcomeKind, evidence::ReturnReceipt>,
}

/// One shared relation table can grow across multiple relative entries of a
/// transaction. The optional graph-certificate cache is independent of this
/// semantic fixed point and cannot replace it for recursive calls.
pub(in crate::analysis) struct EvmTable<
    N: WordElement,
    Key: PolicyKey = (crate::analysis::MachineKey, Option<bool>),
> {
    procedures: Vec<EvmProcedure<N, Key>>,
    procedures_by_key: KeyIndex<(usize, Key)>,
    nodes: Vec<EvmNode<N>>,
    subscribers: Vec<EvmSubscriber<N>>,
    // Each continuation/callee pair scopes the injected finite vocabulary.
    // Subscriber cardinality is bounded by bodies x procedures x policy keys;
    // every allocation consumes the same transaction work ledger.
    subscribers_by_key: KeyIndex<(usize, usize, Key)>,
    frontiers: Vec<EvmExecutionFrontier<N>>,
    // Moving deferred frontier payloads to the outer result does not erase the
    // relation's incompleteness or make its native completion observation true.
    incomplete: bool,
    /// Query precision losses in this relation, reported at the outer summary
    /// site instead of assigning callback PCs to an invented Unknown opcode.
    precision_losses: BTreeSet<DiagnosticKind>,
    propagations: usize,
    reused: usize,
    exchange_inputs: BTreeMap<
        (
            u8,
            usize,
            u8,
            crate::analysis::MachineKey,
            MachineEdgeKind,
            usize,
        ),
        usize,
    >,
}

impl<N: WordElement, Key: PolicyKey> Default for EvmTable<N, Key> {
    fn default() -> Self {
        Self {
            procedures: Vec::new(),
            procedures_by_key: KeyIndex::new(),
            nodes: Vec::new(),
            subscribers: Vec::new(),
            subscribers_by_key: KeyIndex::new(),
            frontiers: Vec::new(),
            incomplete: false,
            precision_losses: BTreeSet::new(),
            propagations: 0,
            reused: 0,
            exchange_inputs: BTreeMap::new(),
        }
    }
}

struct EvmSolver<
    'a,
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> {
    table: &'a mut EvmTable<D::Element, S::Key>,
    world: &'a EvmWorld<D::Element>,
    entry: &'a EvmEntry<D::Element>,
    config: &'a crate::analysis::ExecutionConfig,
    domain: AlgebraRef<'a, D>,
    policies: &'a EnginePolicies<C, P, S>,
    budget: &'a mut WorkBudget,
    counters: &'a mut Counters,
    storage_reads: StorageReadPolicy,
    runtime: &'a mut Runtime<<X as InputTasks>::Task>,
    cooperation: &'a mut Cooperation<D::Element, X, E>,
    active: Option<RunningJob>,
    caller: Option<&'a mut RunningJob>,
    namespace: usize,
    producer: Option<Producer>,
    deferred: bool,
    consumed: Option<crate::analysis::cooperation::Subject>,
}

impl<N: WordElement, Key: PolicyKey> EvmTable<N, Key> {
    /// Empty output is a complete bound only when no known entry remains live.
    /// Unknown procedures have their own conservative environment semantics;
    /// their known callbacks still participate in this whole-table obligation.
    fn pending_terminals(&self) -> impl Iterator<Item = usize> + '_ {
        self.procedures.iter().filter_map(|procedure| {
            if procedure.frame.mode == FrameCode::Unknown || !procedure.outputs.is_empty() {
                return None;
            }
            procedure
                .entries
                .iter()
                .copied()
                .find(|id| !self.nodes[*id].excluded)
        })
    }

    /// Give cooperation an explicit native source; callers need not infer an
    /// entry Body index or reinterpret a Body input under another payload.
    pub(in crate::analysis) fn entry_source(
        &self,
        procedure: usize,
    ) -> Option<ProcedureEntrySource<'_, N>> {
        self.procedures
            .get(procedure)
            .map(|entry| ProcedureEntrySource {
                namespace: entry.namespace,
                payload: &entry.entry_payload,
                revision: &entry.entry_revision,
                slot: &entry.entry_source,
            })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one transaction ledger and fixed execution inputs"
    )]
    pub(in crate::analysis) fn execute_with<
        D: EvmNumericAlgebra<Element = N>,
        C: ContextPolicy,
        P: PartitionPolicy,
        S: StatePolicy<D, P::Class, Key = Key>,
        X: Composition<N, Error = E>,
        E,
    >(
        &mut self,
        source_state: usize,
        world: &EvmWorld<N>,
        entry: &EvmEntry<N>,
        payload: EvmMachinePayload<N>,
        config: &crate::analysis::ExecutionConfig,
        domain: AlgebraRef<'_, D>,
        policies: &EnginePolicies<C, P, S>,
        budget: &mut WorkBudget,
        counters: &mut Counters,
        storage_reads: StorageReadPolicy,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        caller: &mut RunningJob,
        cooperation: &mut Cooperation<N, X, E>,
    ) -> (EvmExecution<N>, EvmProcedureSummaryRecord<N>) {
        // Recursive closure is mandatory. This option only controls reuse
        // between distinct entries from the outer transaction worklist.
        // Without optional reuse, each outer input owns a separate relation.
        // Keep its jobs live across revisits instead of dropping graph handles
        // and stranding the caller's output subscriptions.
        let namespace = if config.use_summaries {
            0
        } else {
            source_state.saturating_add(1)
        };
        let start_transfers = counters.transfers;
        let start_propagations = self.propagations;
        let start_reused = self.reused;
        let kind = if payload.active().code == FrameCode::Unknown {
            ProcedureSummaryKind::UnknownEnvironment
        } else if is_recursive_entry(&payload) {
            ProcedureSummaryKind::KnownRecursion
        } else {
            ProcedureSummaryKind::KnownCall
        };
        let mut result = super::empty_execution(payload.clone());
        let mut solver = EvmSolver {
            table: self,
            world,
            entry,
            config,
            domain,
            policies,
            budget,
            counters,
            storage_reads,
            runtime,
            cooperation,
            active: None,
            caller: Some(caller),
            namespace,
            producer: None,
            deferred: false,
            consumed: None,
        };
        let detached = EvmMachinePayload {
            call_stack: EvmCallStack::new(EvmRootFrame {
                state: payload.active().clone(),
            }),
            store: payload.store.clone(),
            relations: payload.relations.clone(),
        };
        let creation = payload
            .call_stack
            .active_child()
            .is_some_and(|child| child.continuation.creation.is_some());
        let root = match solver.procedure_entry(detached.clone(), creation) {
            Ok(root) => Some(root),
            Err(error) => {
                solver.frontier(detached, error.frontier_reason(), 0);
                None
            }
        };
        if let Some(root) = root
            && let Err(error) = solver.observe_origin(
                crate::analysis::cooperation::Point {
                    namespace: 0,
                    kind: crate::analysis::cooperation::PointKind::World,
                    index: source_state,
                },
                root,
            )
        {
            super::boundary(&mut result, 0, error.frontier_reason());
        }
        solver.run();
        let relative_closed = match solver.runtime.relative_closed(solver.budget) {
            Ok(closed) => closed,
            Err(error) => {
                super::boundary(&mut result, 0, error.frontier_reason());
                false
            }
        };
        if let Err(error) = solver.retain_terminal_frontiers(relative_closed) {
            super::boundary(&mut result, 0, error.frontier_reason());
        }
        if let Some(root) = root {
            let procedure = &solver.table.procedures[root];
            let entry_source = solver
                .table
                .entry_source(root)
                .expect("retained root procedure");
            debug_assert_eq!(entry_source.namespace, namespace);
            let output_work = procedure
                .outputs
                .values()
                .fold(1usize, |work, output| work.saturating_add(output.work));
            // Reading even an empty output relation retains the outer consumer.
            // A later callee publication then wakes this exact World job.
            let outputs = solver.runtime.with_run(
                solver.caller.as_deref_mut().expect("outer caller"),
                solver.budget,
                |run| {
                    run.read_slot(entry_source.slot, entry_source.revision, 1)
                        .map_err(kernel_failure)?;
                    run.read_slot(&procedure.output, &procedure.outputs, output_work)
                        .map_err(kernel_failure)
                },
            );
            let outputs = match outputs {
                Ok(answer) => match answer.state {
                    Availability::Value(outputs) => outputs,
                    _ => unreachable!("initialized output relation"),
                },
                Err(error) => {
                    solver.frontier(payload.clone(), error.frontier_reason(), 0);
                    BTreeMap::new()
                }
            };
            let mut data_sources: Vec<(EvmByteArraySummary<N>, EvmByteArray<N>)> = Vec::new();
            for (kind, output) in outputs {
                if let Err(error) = solver.reserve(
                    payload
                        .work_size()
                        .saturating_add(output.work)
                        .saturating_mul(4)
                        .saturating_add(effects::StoreEffect::MATCH_WORK),
                ) {
                    solver.frontier(payload.clone(), error.frontier_reason(), 0);
                    break;
                }
                let mut returned = payload.clone();
                // Static execution preserves data and historical read roots,
                // while nested zero-value calls can add EIP-161 touches.
                let frame = payload.active();
                let output_store = if solver.table.procedures[root].store_effect.keeps_store(
                    frame.code,
                    frame.key.mode,
                    frame.program.as_ref(),
                    kind,
                    creation,
                ) {
                    // `returned` already owns the actual post-entry store,
                    // including CALL transfers/touches and its old read roots.
                    Ok(None)
                } else if payload.active().key.is_static {
                    Ok(Some(
                        output.summary.store.apply_static_touches(&payload.store),
                    ))
                } else {
                    output.summary.store.instantiate(domain).map(Some)
                };
                // Equal numerical byte bounds on mutually exclusive exits of
                // this invocation describe the same unconstrained observable
                // buffer. Give them one fresh source so a return-status join
                // does not manufacture separate length identities. This cache
                // is local to one invocation; later calls remain independent.
                let output_data = if let Some((_, data)) = data_sources
                    .iter()
                    .find(|(bound, _)| *bound == output.summary.data)
                {
                    Ok(data.clone())
                } else {
                    output.summary.data.instantiate(domain).inspect(|data| {
                        data_sources.push((output.summary.data.clone(), data.clone()));
                    })
                };
                let (store, data) = match (output_store, output_data) {
                    (Ok(store), Ok(data)) => (store, data),
                    (Err(error), _) => {
                        solver.frontier(
                            payload.clone(),
                            MachineError::Store(error).frontier_reason(),
                            0,
                        );
                        continue;
                    }
                    (_, Err(error)) => {
                        solver.frontier(
                            payload.clone(),
                            MachineError::Bytes(error).frontier_reason(),
                            0,
                        );
                        continue;
                    }
                };
                if let Some(store) = store {
                    returned.store = store;
                }
                let mut execution = resume_summary_inner(
                    returned,
                    kind,
                    data,
                    config,
                    domain,
                    solver.budget,
                    false,
                );
                for successor in &mut execution.successors {
                    successor.kind = MachineEdgeKind::Summary(kind);
                }
                result.successors.extend(execution.successors);
                result.completed_calls.extend(execution.completed_calls);
                result.outcomes.extend(execution.outcomes);
                result.frontiers.extend(execution.frontiers);
            }
        }
        // Every frontier retains its actual numerical and relational procedure state;
        // successful discovered outputs remain useful on an interrupted solve.
        result
            .frontiers
            .extend(
                solver
                    .table
                    .frontiers
                    .iter()
                    .map(|frontier| EvmExecutionFrontier {
                        pc: frontier.pc,
                        reason: frontier.reason.clone(),
                        target: frontier.target.clone(),
                        range: frontier.range.clone(),
                        payload: frontier.payload.clone(),
                    }),
            );
        result.diagnostics.extend(
            solver
                .table
                .precision_losses
                .iter()
                .cloned()
                .map(|kind| (0, kind)),
        );
        let evidence_work = solver
            .table
            .procedures
            .iter()
            .fold(0usize, |work, procedure| {
                procedure.outputs.values().fold(
                    work.saturating_add(procedure.entry_payload.store.work_size())
                        .saturating_add(procedure.covered_blocks.len())
                        .saturating_add(procedure.dependencies.len())
                        .saturating_add(1),
                    |work, output| work.saturating_add(output.work),
                )
            });
        let procedures = solver
            .reserve(evidence_work.saturating_mul(4))
            .and_then(|()| {
                solver
                    .table
                    .procedures
                    .iter()
                    .enumerate()
                    .map(|(id, procedure)| {
                        let input = solver.table.entry_source(id).expect("retained procedure");
                        Ok(EvmProcedureSummary {
                            frame: procedure.frame.clone(),
                            input: input.payload.store.instantiate(domain)?,
                            outputs: procedure
                                .outputs
                                .iter()
                                .map(|(kind, output)| {
                                    Ok(EvmProcedureSummaryOutput {
                                        kind: *kind,
                                        data: output.summary.data.instantiate(domain)?,
                                        store: output.summary.store.instantiate(domain)?,
                                    })
                                })
                                .collect::<Result<Vec<_>, MachineError>>()?,
                            dependencies: procedure.dependencies.iter().copied().collect(),
                            input_updates: procedure.entry_updates,
                            output_updates: procedure.output_updates,
                            covered_blocks: procedure.covered_blocks.iter().copied().collect(),
                        })
                    })
                    .collect::<Result<Vec<_>, MachineError>>()
            });
        let procedures = match procedures {
            Ok(procedures) => procedures,
            Err(error) => {
                super::boundary(&mut result, 0, error.frontier_reason());
                Vec::new()
            }
        };
        // Procedure IDs in the compact graph must index an exported summary.
        // An earlier evidence refusal already retained an explicit frontier.
        let regional_cfg = if procedures.len() != solver.table.procedures.len() {
            EvmRegionalCfg::default()
        } else {
            match solver.regional_cfg() {
                Ok(cfg) => cfg,
                Err(error) => {
                    super::boundary(&mut result, 0, error.frontier_reason());
                    EvmRegionalCfg::default()
                }
            }
        };
        let closed = relative_closed && result.frontiers.is_empty() && !solver.table.incomplete;
        let mut record = EvmProcedureSummaryRecord {
            source_state,
            source_current: false,
            kind,
            root_procedure: root.filter(|root| *root < procedures.len()),
            closed,
            procedures,
            regional_cfg,
            state_count: solver.table.nodes.len(),
            transfers: solver.counters.transfers.saturating_sub(start_transfers),
            propagations: solver.table.propagations.saturating_sub(start_propagations),
            reused: solver.table.reused.saturating_sub(start_reused),
        };
        if let Err(error) = solver.regional_checkpoint() {
            super::boundary(&mut result, 0, error.frontier_reason());
            record.closed = false;
            record.regional_cfg = EvmRegionalCfg::default();
        }
        (result, record)
    }
}

#[cfg(test)]
pub(in crate::analysis) mod testing;
#[cfg(test)]
use crate::analysis::{ChildFrame, Continuation, MachinePayload};
#[cfg(test)]
use testing::{Solver, Table, execute_inner, form};

#[cfg(test)]
use super::{Execution, ExecutionFrontier};
