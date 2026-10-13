//! Project the complete world machine while preserving native graph namespaces.
mod summaries;
use super::{checkpoint, mapping, pools::Pools, rpc, ssa, value};
use evm_abstract::{
    analysis::{self, control::Control, progress::Phase},
    domain::AbstractValue,
    world::{GasInput, SnapshotIdentity, Store},
};
use evm_abstract_protocol as api;

pub(super) fn report(
    native: &analysis::WorldAnalysis,
    request: &api::AnalyzeRequest,
    scope: api::AnalysisScope,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    checkpoint(control)?;
    let mut pools = Pools::new(control);
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
    let ssa = ssa::report(native, control, &mut pools.symbolic)?;
    checkpoint(control)?;
    control.observer().phase(Phase::Projecting);
    let initial = Store::new(native.world());
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
        let block = state
            .program()
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
            id: state.id,
            basic_block: state.active().basic_block_index,
            start_pc: block.map(|block| block.start_pc),
            context: state.active().jump_history.clone(),
            frame_depth: state.key.frames.len(),
            code_address: state.active().code_address.to_string(),
            storage_address: state.active().address.to_string(),
            program,
            instructions: block
                .into_iter()
                .flat_map(|block| &block.instructions)
                .map(mapping::instruction)
                .collect(),
            entry_stack: state
                .entry
                .active()
                .stack
                .iter()
                .map(ToString::to_string)
                .collect(),
            exit_stack: state.exit_stack.iter().map(ToString::to_string).collect(),
            executed_pcs: state.executed_pcs.clone(),
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
    let environment = &native.entry().environment;
    pools.symbolic.charge(
        environment
            .block_hashes
            .len()
            .saturating_add(environment.blob_hashes.hashes.len()),
    )?;
    let unknown = AbstractValue::top();
    let hashes =
        |table: &std::collections::BTreeMap<evm_abstract::U256, alloy_primitives::B256>| {
            table
                .iter()
                .map(|(index, hash)| api::IndexedHash {
                    index: value::word(*index),
                    hash: hash.to_string(),
                })
                .collect()
        };
    let environment = api::EnvironmentSnapshot {
        to: value::address(environment.to),
        caller: value::address(environment.caller),
        origin: value::address(environment.resolved_origin()),
        call_value: pools.symbolic.value(&environment.value)?,
        calldata: pools.buffer(&environment.calldata)?,
        is_static: environment.is_static,
        gas_upper_bound: match environment.gas {
            GasInput::Unknown => None,
            GasInput::UpperBound(bound) => Some(value::word(bound)),
        },
        gas_price: pools.symbolic.value(&environment.gas_price)?,
        coinbase: value::address(environment.coinbase),
        timestamp: pools.symbolic.value(&environment.timestamp)?,
        number: pools.symbolic.value(&environment.number)?,
        prevrandao: pools.symbolic.value(&environment.prevrandao)?,
        gas_limit: pools.symbolic.value(&environment.gas_limit)?,
        chain_id: pools
            .symbolic
            .value(environment.chain_id.as_ref().unwrap_or(&unknown))?,
        base_fee: pools.symbolic.value(&environment.base_fee)?,
        blob_base_fee: pools.symbolic.value(&environment.blob_base_fee)?,
        block_hashes: hashes(&environment.block_hashes),
        blob_hashes: hashes(&environment.blob_hashes.hashes),
        blob_count: pools.symbolic.value(&environment.blob_hashes.length)?,
    };
    checkpoint(control)?;
    let abstract_summaries = summaries::project(native, control, &mut pools.symbolic)?;
    Ok(api::AnalysisReport {
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
        abstract_summaries,
        acquisition: native.rpc_acquisition().map(rpc::acquisition),
    })
}
