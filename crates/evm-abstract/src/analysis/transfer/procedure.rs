//! Caller-independent saturation for known recursion and unknown interactions.
//!
//! A table entry retains numerical bounds and finite stack equalities; recursion uses
//! worklist saturation rather than stack-depth unrolling. Every live block and separate
//! summary invocation gets independent symbolic state roots. A caller subscription keeps
//! its own continuation and rollback checkpoint. Known bytecode, repeated environment
//! interactions, and their mutually recursive calls therefore use the same worklist.

use super::{
    Execution, ExecutionFrontier, StorageReadPolicy, Successor, TransferContext, WorkBudget,
    boundary_on, calls, execute_inner, relations, resume_summary_inner, zero,
};
use crate::{
    analysis::{
        CallStack, ChildFrame, Continuation, DiagnosticKind, FrameCode, FrameKey, FrameState,
        FrontierReason, Limit, MachineEdgeKind, MachineError, MachineKey, MachinePayload,
        OutcomeKind, RootFrame,
        engine::Counters,
        machine::{
            ProcedureSummary, ProcedureSummaryKind, ProcedureSummaryOutput, ProcedureSummaryRecord,
        },
    },
    domain::{AbstractValue, Domain, NumericValue, relational::RelationState},
    world::{
        AddressInput, ByteArray, ByteArraySummary, Code, Entry, Snapshot, Store, StoreSummary,
        World,
    },
};
use alloy_primitives::{Address, keccak256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

mod coarse_retry;
mod context;
mod cycles;
mod environment;
mod loop_bound;
mod payload;
mod projection;
mod recursion;
mod solver;
mod stack_alias;
mod storage_observations;
#[cfg(test)]
mod tests;

pub(in crate::analysis) use loop_bound::LoopBound;
use payload::{form, instantiate, join, semantic_eq};
pub(in crate::analysis) use recursion::is_recursive_entry;

/// This is an internal owner namespace, never an observed account address.
/// All unknown-owner effects are abstracted together, including possible aliases.
pub(super) fn owner_namespace(world: &World, store: &Store) -> Address {
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameSummary {
    key: FrameKey,
    code: FrameCode,
    program: Option<crate::bytecode::Program>,
    stack: Vec<AbstractValue>,
    stack_aliases: stack_alias::StackAliases,
    memory: ByteArraySummary,
    calldata: ByteArraySummary,
    environment_calldata: bool,
    returndata: ByteArraySummary,
    call_value: AbstractValue,
    address_operand: AbstractValue,
    caller_operand: AbstractValue,
    saved_store: StoreSummary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PayloadSummary {
    frames: Vec<FrameSummary>,
    continuations: Vec<Continuation>,
    store: StoreSummary,
    storage_observations: storage_observations::StorageObservations,
    /// Canonical retained-object and independent-instantiation charge.
    work: usize,
    /// Procedure location only; this is never a witness for the joined invariant.
    location: MachineKey,
}

struct Node {
    procedure: usize,
    payload: PayloadSummary,
    updates: usize,
    /// Local control-flow edges, including return to a CALL continuation.
    /// Callee entries stay in the separate procedure dependency graph.
    successors: BTreeSet<usize>,
    loop_head: bool,
    /// Promotion belongs to the node, across incoming joins and fresh roots.
    runtime_retry: coarse_retry::RuntimeRetry,
}

struct Procedure {
    frame: FrameKey,
    entry: usize,
    // A real CALL continuation keeps success/failure separate for one block.
    // No arbitrary scalar or physical depth becomes part of this finite key.
    nodes: BTreeMap<(FrameKey, Option<bool>), usize>,
    outputs: BTreeMap<OutcomeKind, Output>,
    subscribers: Vec<usize>,
    dependencies: BTreeSet<usize>,
    recursive: bool,
    local_cycle: bool,
    output_updates: usize,
    covered_blocks: BTreeSet<usize>,
}

#[derive(Clone)]
struct Output {
    data: ByteArraySummary,
    store: StoreSummary,
    work: usize,
    generation: usize,
}

struct Subscriber {
    from: usize,
    callee: usize,
    /// A relative parent plus one child; each child retains its pre-entry snapshot.
    payload: PayloadSummary,
    updates: usize,
    delivered: BTreeMap<OutcomeKind, usize>,
}

/// One shared relation table can grow across multiple relative entries of a
/// transaction. The optional graph-certificate cache is independent of this
/// semantic fixed point and cannot replace it for recursive calls.
#[derive(Default)]
pub(in crate::analysis) struct Table {
    procedures: Vec<Procedure>,
    procedures_by_key: BTreeMap<FrameKey, usize>,
    nodes: Vec<Node>,
    subscribers: Vec<Subscriber>,
    subscribers_by_key: BTreeMap<(usize, usize), usize>,
    queue: VecDeque<usize>,
    queued: BTreeSet<usize>,
    frontiers: Vec<ExecutionFrontier>,
    /// Query precision losses in this relation, reported at the outer summary
    /// site instead of assigning callback PCs to an invented Unknown opcode.
    precision_losses: BTreeSet<DiagnosticKind>,
    propagations: usize,
    reused: usize,
}

struct Solver<'a> {
    table: &'a mut Table,
    world: &'a World,
    entry: &'a Entry,
    config: &'a crate::analysis::ExecutionConfig,
    domain: Domain,
    budget: &'a mut WorkBudget,
    counters: &'a mut Counters,
    storage_reads: StorageReadPolicy,
}

impl Table {
    #[expect(
        clippy::too_many_arguments,
        reason = "one transaction ledger and fixed execution inputs"
    )]
    pub(in crate::analysis) fn execute(
        &mut self,
        source_state: usize,
        world: &World,
        entry: &Entry,
        payload: MachinePayload,
        config: &crate::analysis::ExecutionConfig,
        domain: Domain,
        budget: &mut WorkBudget,
        counters: &mut Counters,
        storage_reads: StorageReadPolicy,
    ) -> (Execution, ProcedureSummaryRecord) {
        // Recursive closure is mandatory. This option only controls reuse
        // between distinct entries from the outer transaction worklist.
        if !config.use_summaries {
            *self = Self::default();
        }
        let start_transfers = counters.transfers;
        let start_propagations = self.propagations;
        let start_reused = self.reused;
        let kind = if payload.active().code == FrameCode::Unknown {
            ProcedureSummaryKind::UnknownEnvironment
        } else {
            ProcedureSummaryKind::KnownRecursion
        };
        let mut result = super::empty_execution(payload.clone());
        let mut solver = Solver {
            table: self,
            world,
            entry,
            config,
            domain,
            budget,
            counters,
            storage_reads,
        };
        let detached = MachinePayload {
            call_stack: CallStack::new(RootFrame {
                state: payload.active().clone(),
            }),
            store: payload.store.clone(),
            relations: payload.relations.clone(),
        };
        let root = match solver.procedure(detached.clone()) {
            Ok(root) => Some(root),
            Err(error) => {
                solver.frontier(detached, error.frontier_reason(), 0);
                None
            }
        };
        solver.run();
        if kind == ProcedureSummaryKind::KnownRecursion
            && let Some(root) = root
            && solver.table.procedures[root].outputs.is_empty()
            && solver.table.queue.is_empty()
            && solver.table.frontiers.is_empty()
        {
            // Folding physical depth cannot certify empty finite behavior.
            // CALL rejection normally supplies real continuation-derived exits;
            // when none reaches a terminal, retain the unmodeled termination.
            let current = solver.table.procedures[root].entry;
            solver.frontier_summary(
                solver.table.nodes[current].payload.clone(),
                FrontierReason::RecursiveNoTerminal,
                0,
            );
        }
        if let Some(root) = root {
            let outputs = solver.table.procedures[root].outputs.clone();
            let mut data_sources: Vec<(ByteArraySummary, ByteArray)> = Vec::new();
            for (kind, output) in outputs {
                if let Err(error) = solver.reserve(
                    payload
                        .work_size()
                        .saturating_add(output.work)
                        .saturating_mul(4),
                ) {
                    solver.frontier(payload.clone(), error.frontier_reason(), 0);
                    break;
                }
                let mut returned = payload.clone();
                // Static execution preserves data and historical read roots,
                // while nested zero-value calls can add EIP-161 touches.
                let output_store = if payload.active().key.is_static {
                    Ok(output.store.apply_static_touches(&payload.store))
                } else {
                    output.store.instantiate(domain)
                };
                // Equal numerical byte bounds on mutually exclusive exits of
                // this invocation describe the same unconstrained observable
                // buffer. Give them one fresh source so a return-status join
                // does not manufacture separate length identities. This cache
                // is local to one invocation; later calls remain independent.
                let output_data = if let Some((_, data)) =
                    data_sources.iter().find(|(bound, _)| *bound == output.data)
                {
                    Ok(data.clone())
                } else {
                    output.data.instantiate(domain).inspect(|data| {
                        data_sources.push((output.data.clone(), data.clone()));
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
                returned.store = store;
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
        result.frontiers.extend(
            solver
                .table
                .frontiers
                .iter()
                .map(|frontier| ExecutionFrontier {
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
                    work.saturating_add(
                        solver.table.nodes[procedure.entry]
                            .payload
                            .store
                            .work_size(),
                    )
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
                    .map(|procedure| {
                        Ok(ProcedureSummary {
                            frame: procedure.frame.clone(),
                            input: solver.table.nodes[procedure.entry]
                                .payload
                                .store
                                .instantiate(domain)?,
                            outputs: procedure
                                .outputs
                                .iter()
                                .map(|(kind, output)| {
                                    Ok(ProcedureSummaryOutput {
                                        kind: *kind,
                                        data: output.data.instantiate(domain)?,
                                        store: output.store.instantiate(domain)?,
                                    })
                                })
                                .collect::<Result<Vec<_>, MachineError>>()?,
                            dependencies: procedure.dependencies.iter().copied().collect(),
                            input_updates: solver.table.nodes[procedure.entry].updates,
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
        let closed = solver.table.queue.is_empty()
            && solver.table.queued.is_empty()
            && result.frontiers.is_empty();
        let record = ProcedureSummaryRecord {
            source_state,
            kind,
            root_procedure: root.filter(|root| *root < procedures.len()),
            closed,
            procedures,
            state_count: solver.table.nodes.len(),
            transfers: solver.counters.transfers.saturating_sub(start_transfers),
            propagations: solver.table.propagations.saturating_sub(start_propagations),
            reused: solver.table.reused.saturating_sub(start_reused),
        };
        (result, record)
    }
}
