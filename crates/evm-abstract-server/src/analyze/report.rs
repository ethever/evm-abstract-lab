//! Project the complete world machine while preserving native graph namespaces.
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
    control.observer().phase(Phase::BuildingSsa);
    let ssa = ssa::report(native, control).map_err(ssa::errors::convert)?;
    checkpoint(control)?;
    control.observer().phase(Phase::Projecting);
    let mut pools = Pools::new(control);
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
        .map(|id| pools.programs[id].blocks.clone())
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
        call_value: value::info(&environment.value),
        calldata: pools.buffer(&environment.calldata)?,
        is_static: environment.is_static,
        gas_upper_bound: match environment.gas {
            GasInput::Unknown => None,
            GasInput::UpperBound(bound) => Some(value::word(bound)),
        },
        gas_price: value::info(&environment.gas_price),
        coinbase: value::address(environment.coinbase),
        timestamp: value::info(&environment.timestamp),
        number: value::info(&environment.number),
        prevrandao: value::info(&environment.prevrandao),
        gas_limit: value::info(&environment.gas_limit),
        chain_id: value::info(environment.chain_id.as_ref().unwrap_or(&unknown)),
        base_fee: value::info(&environment.base_fee),
        blob_base_fee: value::info(&environment.blob_base_fee),
        block_hashes: hashes(&environment.block_hashes),
        blob_hashes: hashes(&environment.blob_hashes.hashes),
        blob_count: value::info(&environment.blob_hashes.length),
    };
    checkpoint(control)?;
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
                detail: format!("{:?}", item.kind),
            })
            .collect(),
        frontiers: native
            .frontiers()
            .iter()
            .map(|item| api::Frontier {
                from: item.from,
                pc: item.pc,
                kind: mapping::frontier(&item.reason),
                reason: mapping::reason(&item.reason),
                detail: format!("{:?}", item.reason),
            })
            .collect(),
        metadata: api::ReportMetadata {
            entry_address: native.entry().address.to_string(),
            root_program,
            snapshot,
            fingerprint: world.fingerprint().to_string(),
            environment,
            limits: request.limits.clone(),
            work: native.work() as u64,
        },
        programs: pools.programs,
        accounts,
        states,
        byte_arrays: pools.buffers_dto,
        stores: pools.stores,
        outcomes,
        acquisition: native.rpc_acquisition().map(rpc::acquisition),
    })
}
