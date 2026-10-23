//! Project the complete world machine while preserving native graph namespaces.
//! The caller retains the actual execution algebra; the model label comes from
//! application dispatch, while captured numeric limits come from native results.
mod environment;
mod summaries;
mod terminal;
use super::{checkpoint, mapping, pools::Pools, rpc, ssa, value};
use evm_abstract::{
    analysis::{self, control::Control, progress::Phase},
    domain::{EvmNumericAlgebra, ValueAlgebra, WordElement},
    resource::WorkBudget,
    world::{EvmStore, SnapshotIdentity},
};
use evm_abstract_protocol as api;

pub(super) fn report<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    native: &analysis::EvmWorldAnalysis<N>,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    checkpoint(control)?;
    // The request's Control.scope covers this ledger and every native output
    // computation. There is no local execution projection on the server path.
    let mut postprocessing = WorkBudget::new(super::DEFAULT_POSTPROCESS_WORK);
    let mut pools = Pools::<N>::new(control);
    pools.symbolic.charge(
        native
            .states()
            .len()
            .saturating_add(native.edges().len())
            .saturating_add(native.diagnostics().len())
            .saturating_add(native.frontiers().len())
            .saturating_add(native.outcomes().len())
            .saturating_add(native.world().accounts().len()),
    )?;
    control.observer().phase(Phase::BuildingSsa);
    let ssa = ssa::report(native, control, &mut postprocessing, &mut pools.symbolic)?;
    checkpoint(control)?;
    control.observer().phase(Phase::Projecting);
    let initial =
        EvmStore::<N>::new_checked(native.world(), algebra.borrowed()).map_err(|error| {
            if control.cancellation().is_cancelled() {
                return pools.symbolic.failure(api::ExportFailureKind::Cancelled, 0);
            }
            let mut failure = pools.symbolic.failure(api::ExportFailureKind::StateRoot, 0);
            failure.message = format!("initial store export failed: {error}");
            failure
        })?;
    let accounts: Vec<api::AccountState> = native
        .world()
        .accounts()
        .keys()
        .map(|address| pools.account(&initial, *address))
        .collect::<Result<_, _>>()?;
    let mut states = Vec::with_capacity(native.states().len());
    let mut cfg = Vec::with_capacity(native.states().len());
    for state in native.states() {
        checkpoint(control)?;
        let entry = pools.machine(&state.entry)?;
        let program = entry.frames.last().and_then(|frame| frame.program);
        let process_key = state.active().process.as_deref().map(super::process::key);
        let is_native = process_key.is_none();
        let block = is_native
            .then(|| state.program())
            .flatten()
            .and_then(|program| program.blocks().get(state.active().basic_block_index));
        pools.symbolic.charge(
            state
                .active()
                .jump_history
                .len()
                .saturating_add(state.executed_pcs.len())
                .saturating_add(state.exit_stack.len())
                .saturating_add(block.map_or(0, |block| block.instructions.len()))
                .saturating_add(1),
        )?;
        cfg.push(api::CfgBlock {
            process: process_key,
            id: state.id,
            basic_block: is_native.then_some(state.active().basic_block_index),
            start_pc: block.map(|block| block.start_pc),
            context: if is_native {
                state.active().jump_history.clone()
            } else {
                vec![]
            },
            frame_depth: state.key.frames.len(),
            code_address: is_native.then(|| state.active().code_address.to_string()),
            storage_address: is_native.then(|| state.active().address.to_string()),
            program,
            instructions: block
                .into_iter()
                .flat_map(|block| &block.instructions)
                .map(mapping::instruction)
                .collect(),
            entry_stack: if is_native {
                state
                    .entry
                    .active()
                    .stack
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            } else {
                vec![]
            },
            exit_stack: if is_native {
                state.exit_stack.iter().map(ToString::to_string).collect()
            } else {
                vec![]
            },
            executed_pcs: if is_native {
                state.executed_pcs.clone()
            } else {
                vec![]
            },
        });
        states.push(api::StateDetails {
            state: state.id,
            entry,
            exit: state
                .exit
                .as_ref()
                .map(|payload| pools.machine(payload))
                .transpose()?,
        });
    }
    let mut outcomes = Vec::with_capacity(native.outcomes().len());
    for outcome in native.outcomes() {
        checkpoint(control)?;
        outcomes.push(api::Outcome {
            state: outcome.state,
            kind: match outcome.kind {
                analysis::OutcomeKind::Return => api::OutcomeKind::Return,
                analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
                analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
            },
            data: pools.buffer(&outcome.data)?,
            store: pools.store(&outcome.store)?,
        });
    }
    let root_program = states
        .first()
        .and_then(|state| state.entry.frames.first())
        .and_then(|frame| frame.program)
        .or_else(|| {
            if !states.is_empty() {
                return None;
            }
            // Admission may exhaust work before a first machine state exists. Source
            // observations still belong to the result; following only one marker
            // matches EIP-7702 and does not claim any instruction was executed.
            let root = accounts
                .iter()
                .find(|account| account.address == native.entry().address.to_string())?;
            root.program.or_else(|| {
                root.delegation_target
                    .as_ref()
                    .and_then(|target| accounts.iter().find(|account| &account.address == target))
                    .and_then(|account| account.program)
            })
        });
    let disassembly = root_program
        .map(|id| {
            let source = &pools.programs[id];
            pools.symbolic.charge(
                source
                    .blocks
                    .iter()
                    .map(|block| block.instructions.len() + 1)
                    .sum(),
            )?;
            Ok(source.blocks.clone())
        })
        .transpose()?
        .unwrap_or_default();
    let byte_len = root_program.map_or(0, |id| {
        pools.programs[id].bytecode.len().saturating_sub(2) / 2
    });
    let world = native.world();
    let snapshot = match world.identity() {
        SnapshotIdentity::Offline { .. } => None,
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => {
            let facts = world.snapshot_environment();
            Some(api::ChainSnapshot {
                chain_id: value::word(*chain_id),
                block_hash: block_hash.to_string(),
                number: facts.map(|facts| value::word(facts.number)),
                parent_hash: facts.map(|facts| facts.parent_hash.to_string()),
                timestamp: facts.map(|facts| value::word(facts.timestamp)),
                coinbase: facts.map(|facts| facts.coinbase.to_string()),
                prevrandao: facts.map(|facts| facts.prevrandao.to_string()),
                gas_limit: facts.map(|facts| value::word(facts.gas_limit)),
                base_fee: facts.and_then(|facts| facts.base_fee).map(value::word),
                blob_base_fee: facts.and_then(|facts| facts.blob_base_fee).map(value::word),
                excess_blob_gas: facts
                    .and_then(|facts| facts.excess_blob_gas)
                    .map(value::word),
                blob_gas_used: facts.and_then(|facts| facts.blob_gas_used).map(value::word),
            })
        }
    };
    let environment = environment::project(&native.entry().environment, &mut pools)?;
    checkpoint(control)?;
    let abstract_summaries =
        summaries::project(native.procedure_summaries(), control, &mut pools.symbolic)?;
    let root_terminal_coverage = native
        .root_terminal_coverage()
        .map(|source| terminal::project(source, numeric_model, &mut pools))
        .transpose()?;
    checkpoint(control)?;
    let report = api::AnalysisReport {
        schema_version: api::SCHEMA_VERSION,
        scope,
        fork: request.fork,
        byte_len,
        status: match native.status() {
            analysis::Status::Converged => api::AnalysisStatus::Converged,
            analysis::Status::Incomplete => api::AnalysisStatus::Incomplete,
        },
        transfers: native.transfers() as u64,
        disassembly,
        cfg,
        edges: native
            .edges()
            .iter()
            .enumerate()
            .map(|(id, edge)| api::CfgEdge {
                id,
                from: edge.from,
                to: edge.to,
                kind: mapping::edge(edge.kind),
            })
            .collect(),
        ssa,
        diagnostics: native
            .diagnostics()
            .iter()
            .map(|item| api::Diagnostic {
                state: item.state,
                pc: item.pc,
                kind: mapping::diagnostic(&item.kind),
                reduction: mapping::reduction(&item.kind),
                precision: mapping::precision(&item.kind),
                invalid_opcode: mapping::invalid_opcode(&item.kind),
                detail: match &item.kind {
                    analysis::DiagnosticKind::InvalidOpcode(reason) => reason.to_string(),
                    other => format!("{other:?}"),
                },
            })
            .collect(),
        frontiers: native
            .frontiers()
            .iter()
            .map(|item| {
                Ok(api::Frontier {
                    from: item.from,
                    pc: item.pc,
                    kind: mapping::frontier(&item.reason),
                    reason: mapping::reason(&item.reason),
                    detail: format!("{:?}", item.reason),
                    range: item
                        .range
                        .as_ref()
                        .map(|witness| pools.range(witness))
                        .transpose()?,
                    payload: item
                        .payload
                        .as_ref()
                        .map(|payload| pools.pending(payload))
                        .transpose()?,
                })
            })
            .collect::<Result<_, api::ApiError>>()?,
        metadata: api::ReportMetadata {
            entry_address: native.entry().address.to_string(),
            root_program,
            snapshot,
            fingerprint: world
                .fingerprint()
                .map_err(super::errors::fingerprint)?
                .to_string(),
            environment,
            limits: request.limits.clone(),
            work: native.work() as u64,
            postprocessing_max_work: super::DEFAULT_POSTPROCESS_WORK as u64,
            postprocessing_work: postprocessing.used() as u64,
            timings: api::AnalysisTimings::default(),
            export_limits: pools.symbolic.limits,
            export_usage: pools.symbolic.usage,
        },
        symbolic: pools.symbolic.graph,
        pending_machines: pools.pending,
        programs: pools.programs,
        accounts,
        states,
        byte_arrays: pools.buffers_dto,
        stores: pools.stores,
        outcomes,
        root_terminal_coverage,
        abstract_summaries,
        acquisition: native.rpc_acquisition().map(rpc::acquisition),
    };
    checkpoint(control)?;
    Ok(report)
}
