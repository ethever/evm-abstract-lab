//! A single-program learning view projected from the native world machine.

use super::{
    Diagnostic, Edge, EvmAnalysis, EvmState, EvmWorldAnalysis, Frontier, FrontierReason, Limit,
    StateKey,
};
use crate::{
    domain::WordElement,
    resource::{WorkBudget, clone_payment::pay},
};

/// The root request does not name a decoded runtime program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    /// A local CFG requires a known decoded root program.
    #[error("root request has no decoded runtime program")]
    MissingProgram,
    /// The actual captured root executes native or unresolved semantics.
    #[error("root frame does not execute a runtime program: {0:?}")]
    NonRuntime(super::FrameCode),
    /// The complete native projection could not be paid.
    #[error("local projection work exhausted or unknown")]
    Work,
    /// Current or retained ledger owner cancelled projection.
    #[error("local projection cancelled")]
    Cancelled,
}
impl From<crate::resource::CloneError> for ProjectionError {
    fn from(error: crate::resource::CloneError) -> Self {
        match error {
            crate::resource::CloneError::Work => Self::Work,
            crate::resource::CloneError::Cancelled => Self::Cancelled,
        }
    }
}

impl<N: WordElement> EvmAnalysis<N> {
    /// Project the declared root's native graph, retaining all non-local execution.
    pub fn from_execution(
        execution: EvmWorldAnalysis<N>,
        budget: &mut WorkBudget,
    ) -> Result<Self, ProjectionError> {
        use std::collections::BTreeMap;
        pay(
            budget,
            execution
                .states()
                .len()
                .saturating_mul(16)
                .saturating_add(execution.edges().len().saturating_mul(8))
                .saturating_add(execution.frontiers().len().saturating_mul(8))
                .saturating_add(execution.diagnostics().len().saturating_mul(8))
                .saturating_add(32),
        )?;
        let root = &execution
            .states()
            .first()
            .ok_or(ProjectionError::MissingProgram)?
            .entry
            .call_stack
            .root()
            .state;
        if root.code != super::FrameCode::Runtime {
            return Err(ProjectionError::NonRuntime(root.code));
        }
        let captured_program = root
            .program
            .as_ref()
            .ok_or(ProjectionError::MissingProgram)?;
        let code_address = root.key.code_address;
        let code_hash = root.key.code_hash;
        let selected = |state: &super::EvmMachineState<N>| {
            state.key.frames.len() == 1
                && state.active().mode == super::FrameCode::Runtime
                && state.active().code_address == code_address
                && state.active().code_hash == code_hash
                && state.entry.active().program.as_ref() == Some(captured_program)
                && state.active().basic_block_index < captured_program.blocks().len()
        };
        let mut copies = captured_program.byte_len().saturating_add(32);
        for state in execution.states().iter().filter(|state| selected(state)) {
            pay(
                budget,
                state
                    .entry
                    .active()
                    .stack
                    .len()
                    .saturating_add(state.exit_stack.len())
                    .saturating_add(8),
            )?;
            copies = copies
                .saturating_add(state.entry.relations.work_size().saturating_mul(4))
                .saturating_add(
                    state
                        .exit
                        .as_ref()
                        .map_or(0, |exit| exit.relations.work_size().saturating_mul(4)),
                )
                .saturating_add(state.executed_pcs.len())
                .saturating_add(state.active().jump_history.len());
            for value in state.entry.active().stack.iter().chain(&state.exit_stack) {
                copies = copies.saturating_add(value.clone_work());
            }
        }
        pay(budget, copies)?;
        let program = captured_program.clone();
        let analysis_config = execution.config().analysis.clone();
        let mut states = Vec::new();
        let mut execution_state_ids = Vec::new();
        let mut ids = BTreeMap::new();
        for source in execution.states() {
            if !selected(source) {
                continue;
            }
            let id = states.len();
            ids.insert(source.id, id);
            execution_state_ids.push(source.id);
            states.push(EvmState {
                entry_relations: source.entry.relations.clone(),
                exit_relations: source
                    .exit
                    .as_ref()
                    .map(|payload| payload.relations.clone()),
                id,
                key: StateKey {
                    basic_block_index: source.active().basic_block_index,
                    stack_height: source.active().stack_height,
                    context: source.active().jump_history.clone(),
                },
                entry_stack: source.entry.active().stack.clone(),
                exit_stack: source.exit_stack.clone(),
                executed_pcs: source.executed_pcs.clone(),
            });
        }
        let edges = execution
            .edges()
            .iter()
            .filter_map(|edge| {
                Some(Edge {
                    from: *ids.get(&edge.from)?,
                    to: *ids.get(&edge.to)?,
                    kind: edge.kind,
                })
            })
            .collect();
        let boundary_edges = execution
            .edges()
            .iter()
            .filter(|edge| ids.contains_key(&edge.from) != ids.contains_key(&edge.to))
            .cloned()
            .collect();
        let diagnostics = execution
            .diagnostics()
            .iter()
            .filter_map(|d| {
                Some(Diagnostic {
                    state: *ids.get(&d.state)?,
                    pc: d.pc,
                    kind: d.kind.clone(),
                })
            })
            .collect();
        let initial_key = StateKey {
            basic_block_index: 0,
            stack_height: 0,
            context: Vec::new(),
        };
        let frontiers = execution
            .frontiers()
            .iter()
            .map(|f| {
                let frame = f.target.as_ref().and_then(|k| k.frames.first());
                let target = frame.map_or_else(
                    || {
                        f.from
                            .and_then(|id| ids.get(&id))
                            .map_or_else(|| initial_key.clone(), |id| states[*id].key.clone())
                    },
                    |frame| StateKey {
                        basic_block_index: frame.basic_block_index,
                        stack_height: frame.stack_height,
                        context: frame.jump_history.clone(),
                    },
                );
                let limit = match f.reason {
                    FrontierReason::Budget(limit) => limit,
                    FrontierReason::Work | FrontierReason::SummaryWork => Limit::Work,
                    FrontierReason::CallDepth => Limit::CallDepth,
                    FrontierReason::Memory => Limit::Memory,
                    _ => Limit::Model,
                };
                Frontier {
                    reason: f.reason.clone(),
                    from: f.from.and_then(|id| ids.get(&id).copied()),
                    target,
                    limit,
                }
            })
            .collect();
        pay(budget, 0)?;
        Ok(EvmAnalysis {
            program,
            config: analysis_config,
            schema_version: super::SCHEMA_VERSION,
            domain_spec: execution.domain_spec(),
            states,
            edges,
            boundary_edges,
            diagnostics,
            frontiers,
            status: execution.status(),
            transfers: execution.transfers(),
            execution,
            execution_state_ids,
        })
    }
}
