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

/// Resolve source identity without cloning the Program or reconstructing a Store.
fn root_source<'a, N: WordElement>(
    execution: &'a EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(super::FrameCode, Option<&'a crate::bytecode::Program>), ProjectionError> {
    pay(budget, 8)?;
    if let Some(state) = execution.states().first() {
        let frame = &state.entry.call_stack.root().state;
        pay(budget, 0)?;
        return Ok((frame.code, frame.program.as_ref()));
    }
    if execution.status() != super::Status::Incomplete {
        return Err(ProjectionError::MissingProgram);
    }
    let levels = execution
        .world()
        .accounts()
        .len()
        .checked_ilog2()
        .unwrap_or(0) as usize
        + 2;
    pay(budget, levels.saturating_mul(128))?;
    let (_, mode, program) = super::transfer::resolve_entry_code(
        execution.world().fork(),
        execution.entry().address,
        execution.entry().environment.to,
        |address| {
            execution
                .world()
                .account(address)
                .map(|account| &account.code)
        },
    );
    pay(budget, 0)?;
    Ok((mode, program))
}
impl<N: WordElement> EvmWorldAnalysis<N> {
    /// Original root binding, available before any successful frame admission.
    pub fn root_binding(&self) -> crate::world::RootInputBinding {
        self.entry().binding()
    }

    /// Paid provenance of the declared input, not evidence of execution.
    /// Namespace bytes never establish an observed physical code account.
    pub fn declared_root_source(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<super::ProcessSource, ProjectionError> {
        use super::{FrameCode, ProcessSource};
        use crate::world::Code;
        use alloy_primitives::keccak256;
        let levels = self.world().accounts().len().checked_ilog2().unwrap_or(0) as usize + 2;
        pay(budget, levels.saturating_mul(128))?;
        let coordinate = self.entry().address;
        let namespace = self.entry().environment.to.as_concrete().is_none();
        let fork = self.world().fork();
        let (source, mode, program) = super::transfer::resolve_entry_code(
            fork,
            coordinate,
            self.entry().environment.to,
            |address| self.world().account(address).map(|a| &a.code),
        );
        let provenance = match mode {
            FrameCode::Runtime => {
                let program = program.ok_or(ProjectionError::MissingProgram)?;
                pay(
                    budget,
                    program.byte_len().saturating_mul(4).saturating_add(64),
                )?;
                let hash = program.bytes_digest();
                if namespace && source == coordinate {
                    ProcessSource::InputProgram {
                        hash,
                        fork: program.fork(),
                    }
                } else {
                    ProcessSource::Runtime {
                        source,
                        hash,
                        fork: program.fork(),
                    }
                }
            }
            FrameCode::Unknown => {
                if namespace
                    && let Some(Code::Delegation(target)) =
                        self.world().account(coordinate).map(|a| &a.code)
                {
                    pay(budget, 128)?;
                    ProcessSource::InputDelegation {
                        target: *target,
                        hash: keccak256(
                            self.world()
                                .raw_account_code(coordinate)
                                .ok_or(ProjectionError::MissingProgram)?,
                        ),
                        fork,
                    }
                } else {
                    ProcessSource::AnyRuntime { fork }
                }
            }
            FrameCode::Empty => ProcessSource::Empty { fork },
            FrameCode::Precompile(address) => ProcessSource::Precompile { address, fork },
            FrameCode::InvalidDelegation => {
                pay(budget, 128)?;
                ProcessSource::InvalidDelegation {
                    source,
                    hash: keccak256(
                        self.world()
                            .raw_account_code(source)
                            .ok_or(ProjectionError::MissingProgram)?,
                    ),
                    fork,
                }
            }
            FrameCode::InitCode => return Err(ProjectionError::MissingProgram),
        };
        pay(budget, 0)?;
        Ok(provenance)
    }

    /// Paid source classification. Declared code in an empty incomplete result
    /// names only an unexpanded input, never an executed frame or completion.
    pub fn root_code(&self, budget: &mut WorkBudget) -> Result<super::FrameCode, ProjectionError> {
        root_source(self, budget).map(|(mode, _)| mode)
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
        let root = execution
            .states()
            .first()
            .map(|state| &state.entry.call_stack.root().state);
        let (mode, program) = root_source(&execution, budget)?;
        if mode != super::FrameCode::Runtime {
            return Err(ProjectionError::NonRuntime(mode));
        }
        let program = program.ok_or(ProjectionError::MissingProgram)?;
        let selected = |state: &super::EvmMachineState<N>| {
            root.is_some_and(|root| {
                state.key.frames.len() == 1
                    && state.active().mode == super::FrameCode::Runtime
                    && state.active().code_address == root.key.code_address
                    && state.active().code_hash == root.key.code_hash
                    && state.entry.active().program.as_ref() == Some(program)
                    && state.active().basic_block_index < program.blocks().len()
            })
        };
        let mut copies = program.byte_len().saturating_add(32);
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
        let program = program.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Address, Fork,
        analysis::{
            AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, FrameCode, Status,
            control::Control, cooperation::NoExtension,
        },
        bytecode::Program,
        domain::{NumericValue, ProductAlgebra, ValueAlgebra},
        world::{EvmAccount, EvmEntry, EvmWorld},
    };

    #[test]
    fn unexpanded_runtime_projects_its_declared_code_and_actual_frontier() {
        let values = ValueAlgebra::new(ProductAlgebra::default());
        let native = AnalysisEngine::new(&values, &EnginePolicies::default(), &mut NoExtension)
            .run(
                AnalysisInput::<NumericValue>::from_program(
                    Program::from_hex("600160020100").unwrap(),
                    Default::default(),
                )
                .unwrap(),
                ExecutionConfig {
                    max_work: 1,
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
                &Control::default(),
            )
            .unwrap();
        assert_eq!(native.status(), Status::Incomplete);
        assert!(native.states().is_empty());
        let frontier_count = native.frontiers().len();
        let mut postprocessing = WorkBudget::new(2_000_000_000);
        assert_eq!(
            native.root_code(&mut postprocessing).unwrap(),
            FrameCode::Runtime
        );
        let view = EvmAnalysis::from_execution(native, &mut postprocessing).unwrap();
        assert_eq!(view.program().bytes(), &[0x60, 1, 0x60, 2, 1, 0]);
        assert!(view.states().is_empty() && view.edges().is_empty());
        assert_eq!(view.status(), Status::Incomplete);
        assert_eq!(view.frontiers().len(), frontier_count);
        assert!(view.execution().states().is_empty());
    }

    #[test]
    fn declared_bytes_at_precompile_never_become_runtime_projection() {
        let values = ValueAlgebra::new(ProductAlgebra::default());
        let owner = Address::from_word(crate::U256::ONE.into());
        for max_work in [1, ExecutionConfig::default().max_work] {
            let mut world =
                EvmWorld::<NumericValue>::new(Fork::Osaka, "precompile shadows declared bytes");
            world
                .insert(owner, EvmAccount::from_hex("600100", Fork::Osaka).unwrap())
                .unwrap();
            let native = AnalysisEngine::new(&values, &EnginePolicies::default(), &mut NoExtension)
                .run(
                    AnalysisInput::new(world, EvmEntry::new(owner)).unwrap(),
                    ExecutionConfig {
                        max_work,
                        root_terminal_coverage: false,
                        ..ExecutionConfig::default()
                    },
                    &Control::default(),
                )
                .unwrap();
            if max_work == 1 {
                assert!(native.states().is_empty());
            } else {
                assert!(!native.states().is_empty());
            }
            let mut postprocessing = WorkBudget::new(2_000_000_000);
            assert_eq!(
                native.root_code(&mut postprocessing).unwrap(),
                FrameCode::Precompile(owner)
            );
            assert!(
                matches!(EvmAnalysis::from_execution(native, &mut postprocessing),
                Err(ProjectionError::NonRuntime(FrameCode::Precompile(address))) if address == owner)
            );
        }
    }
    #[test]
    fn serialized_boundaries_publish_native_ids_and_the_local_mapping() {
        let values = ValueAlgebra::new(ProductAlgebra::default());
        let owner = Address::repeat_byte(0x31);
        let callee = Address::from_word(crate::U256::from(0x20).into());
        let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "JSON root child root IDs");
        world
            .insert(
                owner,
                EvmAccount::from_hex("5f5f5f5f5f602061fffff15000", Fork::Osaka).unwrap(),
            )
            .unwrap();
        world
            .insert(callee, EvmAccount::from_hex("00", Fork::Osaka).unwrap())
            .unwrap();
        let native = AnalysisEngine::new(&values, &EnginePolicies::default(), &mut NoExtension)
            .run(
                AnalysisInput::new(world, EvmEntry::new(owner)).unwrap(),
                ExecutionConfig::default(),
                &Control::default(),
            )
            .unwrap();
        assert_eq!(native.status(), Status::Converged);
        let view =
            EvmAnalysis::from_execution(native, &mut WorkBudget::new(2_000_000_000)).unwrap();
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["edge_endpoint_namespace"], "local");
        assert_eq!(json["boundary_endpoint_namespace"], "native");
        let mapping = json["execution_state_ids"].as_array().unwrap();
        assert!(
            mapping
                .iter()
                .enumerate()
                .any(|(local, native)| native.as_u64().unwrap() != local as u64)
        );
        let boundaries = json["boundary_edges"].as_array().unwrap();
        assert!(boundaries.len() >= 2);
        for edge in boundaries {
            let selected: Vec<_> = [edge["from"].as_u64().unwrap(), edge["to"].as_u64().unwrap()]
                .into_iter()
                .filter_map(|native| mapping.iter().position(|id| id.as_u64() == Some(native)))
                .collect();
            assert_eq!(
                selected.len(),
                1,
                "exactly one endpoint is in the selected root view"
            );
            assert_eq!(
                json["states"][selected[0]]["id"].as_u64(),
                Some(selected[0] as u64)
            );
        }
    }
}
