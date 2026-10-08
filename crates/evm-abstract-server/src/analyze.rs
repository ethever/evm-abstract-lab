//! Convert immutable native analysis into the engine-independent browser protocol.
//!
//! Every graph and SSA reference uses the native machine namespace. Incomplete
//! reports retain execution phases, deferred edges and all native frontiers.

mod mapping;
mod ssa;

use evm_abstract::{analysis, bytecode::Program};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, AnalyzeRequest, ApiError, ApiErrorCode,
    CfgBlock, CfgEdge, Diagnostic, DisasmBlock, Fork, Frontier, MAX_REQUEST_BYTES, SCHEMA_VERSION,
};

/// Decode and analyze one bounded request without networking or ambient state.
pub fn analyze(request: AnalyzeRequest) -> Result<AnalysisReport, ApiError> {
    let limits = &request.limits;
    if !(1..=4096).contains(&limits.max_states)
        || !(1..=100_000).contains(&limits.max_transfers)
        || limits.context_depth > 16
        || !(1..=32).contains(&limits.max_constants)
    {
        return Err(ApiError {
            code: ApiErrorCode::InvalidLimits,
            message: "limits require states 1..4096, transfers 1..100000, context depth 0..16 and constants 1..32".into(),
        });
    }
    if request.bytecode.len() > MAX_REQUEST_BYTES {
        return Err(ApiError {
            code: ApiErrorCode::RequestTooLarge,
            message: "bytecode text exceeds request bound".into(),
        });
    }
    let fork = match request.fork {
        Fork::Cancun => evm_abstract::Fork::Cancun,
        Fork::Prague => evm_abstract::Fork::Prague,
        Fork::Osaka => evm_abstract::Fork::Osaka,
    };
    let program =
        Program::from_hex_with_fork(&request.bytecode, fork).map_err(|error| ApiError {
            code: ApiErrorCode::InvalidBytecode,
            message: error.to_string(),
        })?;
    if program.byte_len() > 65_536 {
        return Err(ApiError {
            code: ApiErrorCode::RequestTooLarge,
            message: "runtime bytecode exceeds 65536 bytes".into(),
        });
    }
    let config = analysis::Config {
        max_states: limits.max_states,
        max_transfers: limits.max_transfers,
        max_constants: limits.max_constants,
        context_depth: limits.context_depth,
        ..analysis::Config::default()
    };
    let graph = analysis::analyze(program, config).map_err(|error| ApiError {
        code: ApiErrorCode::InvalidLimits,
        message: error.to_string(),
    })?;
    let native = graph.execution();
    let ssa = ssa::report(native).map_err(|error| ApiError {
        code: ApiErrorCode::Internal,
        message: error.to_string(),
    })?;
    Ok(AnalysisReport {
        schema_version: SCHEMA_VERSION,
        scope: AnalysisScope::SingleProgram,
        fork: request.fork,
        byte_len: graph.program().byte_len(),
        status: match native.status() {
            analysis::Status::Converged => AnalysisStatus::Converged,
            analysis::Status::Incomplete => AnalysisStatus::Incomplete,
        },
        transfers: native.transfers(),
        disassembly: graph
            .program()
            .blocks()
            .iter()
            .map(|block| DisasmBlock {
                id: block.id,
                start_pc: block.start_pc,
                instructions: block
                    .instructions
                    .iter()
                    .map(mapping::instruction)
                    .collect(),
            })
            .collect(),
        cfg: native
            .states()
            .iter()
            .map(|state| {
                let block = state
                    .program()
                    .and_then(|program| program.blocks().get(state.active().basic_block_index));
                CfgBlock {
                    id: state.id,
                    basic_block: state.active().basic_block_index,
                    start_pc: block.map(|block| block.start_pc),
                    context: state.active().jump_history.clone(),
                    frame_depth: state.key.frames.len(),
                    code_address: state.active().code_address.to_string(),
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
                }
            })
            .collect(),
        edges: native
            .edges()
            .iter()
            .enumerate()
            .map(|(id, edge)| CfgEdge {
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
            .map(|item| Diagnostic {
                state: item.state,
                pc: item.pc,
                kind: mapping::diagnostic(&item.kind),
                detail: format!("{:?}", item.kind),
            })
            .collect(),
        frontiers: native
            .frontiers()
            .iter()
            .map(|item| Frontier {
                from: item.from,
                pc: item.pc,
                kind: mapping::frontier(&item.reason),
                detail: format!("{:?}", item.reason),
            })
            .collect(),
    })
}
