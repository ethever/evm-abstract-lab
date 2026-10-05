//! A single-program learning view projected from the native world machine.

use super::{
    Analysis, Config, ConfigError, Diagnostic, DiagnosticKind, Edge, EdgeKind, ExecutionConfig,
    Frontier, FrontierReason, Limit, MachineEdgeKind, State, StateKey, analyze_world,
};
use crate::{bytecode::Program, domain::Value};

pub(super) fn analyze(program: Program, config: Config) -> Result<Analysis, ConfigError> {
    use crate::world::{Account, ByteArray, Code, Entry, World};
    use alloy_primitives::Address;
    use revm_bytecode::opcode;
    use std::collections::BTreeMap;

    let address = Address::repeat_byte(0x11);
    let mut world = World::new(program.fork(), "single-bytecode symbolic environment");
    let mut account = Account::unknown();
    account.code = Code::Runtime(program.clone());
    // A program already carries this same fork, so insertion cannot reject it.
    world
        .insert(address, account)
        .expect("the world uses the program's fork");
    let execution = analyze_world(
        world,
        Entry {
            address,
            caller: Address::ZERO,
            value: Value::top(),
            calldata: ByteArray::unknown(),
            is_static: false,
        },
        ExecutionConfig {
            analysis: config.clone(),
            symbolic_entry_environment: true,
            ..ExecutionConfig::default()
        },
    )?;
    let mut states = Vec::new();
    let mut ids = BTreeMap::new();
    for source in execution.states() {
        if source.key.frames.len() != 1
            || source.active().basic_block_index >= program.blocks().len()
        {
            continue;
        }
        let id = states.len();
        ids.insert(source.id, id);
        let mut exit_stack = source.exit_stack.clone();
        if let Some(last) = program.blocks()[source.active().basic_block_index]
            .instructions
            .iter()
            .take(source.executed_pcs.len())
            .next_back()
            && matches!(
                last.opcode,
                opcode::CALL | opcode::CALLCODE | opcode::DELEGATECALL | opcode::STATICCALL
            )
            && source.exit.is_some()
            && !execution.diagnostics().iter().any(|d| {
                d.state == source.id
                    && d.pc == last.pc
                    && matches!(
                        d.kind,
                        DiagnosticKind::StackUnderflow | DiagnosticKind::StackOverflow
                    )
            })
        {
            // The local view has no callee facts with which to correlate the
            // deferred boolean. Native transitions retain the actual result.
            exit_stack.push(Value::top());
        }
        states.push(State {
            id,
            key: StateKey {
                basic_block_index: source.active().basic_block_index,
                stack_height: source.active().stack_height,
                context: source.active().jump_history.clone(),
            },
            entry_stack: source.entry.active().stack.clone(),
            exit_stack,
            executed_pcs: source.executed_pcs.clone(),
        });
    }
    let edges = execution
        .edges()
        .iter()
        .filter_map(|edge| {
            let kind = match edge.kind {
                MachineEdgeKind::Intraprocedural(kind) => kind,
                MachineEdgeKind::Failure => EdgeKind::Fallthrough,
                _ => return None,
            };
            Some(Edge {
                from: *ids.get(&edge.from)?,
                to: *ids.get(&edge.to)?,
                kind,
            })
        })
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
                from: f.from.and_then(|id| ids.get(&id).copied()),
                target,
                limit,
            }
        })
        .collect();
    Ok(Analysis {
        program,
        config,
        states,
        edges,
        diagnostics,
        frontiers,
        status: execution.status(),
        transfers: execution.transfers(),
        execution,
    })
}
