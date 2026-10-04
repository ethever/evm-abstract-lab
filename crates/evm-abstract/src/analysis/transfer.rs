//! Basic-block execution on a transaction-wide machine. Calls suspend a frame;
//! returns resume it with the same transaction store and an explicit result.

use super::{
    DiagnosticKind, EdgeKind,
    machine::{
        ExecutionConfig, FrameCode, FrontierReason, MachineEdgeKind, MachineKey, MachinePayload,
        OutcomeKind,
    },
};
use crate::{
    bytecode::Program,
    domain::{Domain, Value},
    world::{AbstractLog, ByteArray, Code, Entry, LogKey, Store, World},
};
use alloy_primitives::{Address, U256, keccak256};
use revm_bytecode::opcode;

mod budget;
mod calls;

pub(super) use budget::WorkBudget;
use budget::{byte_work, operation_work};
use calls::{call, failure, finish};

struct TransferContext<'a> {
    config: &'a ExecutionConfig,
    domain: Domain,
    budget: &'a mut WorkBudget,
}

pub(super) fn initial(world: &World, entry: &Entry) -> Result<MachinePayload, FrontierReason> {
    calls::initial(world, entry)
}

pub(super) struct Successor {
    pub payload: MachinePayload,
    pub kind: MachineEdgeKind,
}
pub(super) struct Outcome {
    pub kind: OutcomeKind,
    pub data: ByteArray,
    pub store: Store,
}
pub(super) struct Execution {
    pub payload: MachinePayload,
    pub executed_pcs: Vec<usize>,
    pub successors: Vec<Successor>,
    pub diagnostics: Vec<(usize, DiagnosticKind)>,
    pub frontiers: Vec<(usize, FrontierReason, Option<MachineKey>)>,
    pub outcomes: Vec<Outcome>,
}

fn address(value: U256) -> Address {
    Address::from_slice(&value.to_be_bytes::<32>()[12..])
}
fn address_value(value: Address) -> Value {
    Value::constant(U256::from_be_slice(value.as_slice()))
}
fn constant(value: usize) -> Value {
    Value::constant(U256::from(value))
}
fn zero() -> Value {
    Value::constant(U256::ZERO)
}
fn program(world: &World, code_address: Address) -> Option<&Program> {
    match &world.account(code_address)?.code {
        Code::Runtime(program) => Some(program),
        _ => None,
    }
}

fn boundary(result: &mut Execution, pc: usize, reason: FrontierReason) {
    result
        .frontiers
        .push((pc, reason, Some(result.payload.key())));
}

fn successor(result: &mut Execution, mut payload: MachinePayload, block: usize, kind: EdgeKind) {
    payload.active_mut().key.block = block;
    payload.normalize();
    result.successors.push(Successor {
        payload,
        kind: MachineEdgeKind::Intraprocedural(kind),
    });
}

fn touch_memory(
    result: &mut Execution,
    offset: &Value,
    size: &Value,
    context: &mut TransferContext<'_>,
    pc: usize,
) -> bool {
    let config = context.config;
    let domain = context.domain;
    if !context.budget.charge(byte_work(
        &[offset, size],
        0,
        result.payload.active().memory.work_size(),
        domain,
    )) {
        boundary(result, pc, FrontierReason::Work);
        return false;
    }
    if result
        .payload
        .active_mut()
        .memory
        .expand(offset, size, config.max_memory_bytes, domain)
        .is_err()
    {
        boundary(result, pc, FrontierReason::Memory);
        return false;
    }
    true
}

fn code_bytes(world: &World, target: Address) -> Option<Vec<u8>> {
    match &world.account(target)?.code {
        Code::Runtime(program) => Some(program.bytes().to_vec()),
        Code::Empty => Some(Vec::new()),
        Code::Delegation(target) => {
            let mut bytes = vec![0xef, 0x01, 0x00];
            bytes.extend_from_slice(target.as_slice());
            Some(bytes)
        }
        Code::Unknown => None,
    }
}

fn environment_targets(
    value: &Value,
    domain: Domain,
    mut read: impl FnMut(Address) -> Value,
) -> Value {
    let Some(values) = value.constants() else {
        return Value::top();
    };
    values
        .iter()
        .map(|v| read(address(*v)))
        .reduce(|a, b| domain.join(&a, &b))
        .expect("Value constant sets are nonempty")
}

pub(super) fn execute(
    world: &World,
    entry: &Entry,
    payload: MachinePayload,
    config: &ExecutionConfig,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Execution {
    let mut transfer_context = TransferContext {
        config,
        domain,
        budget,
    };
    let context = &mut transfer_context;
    let mut result = Execution {
        payload,
        executed_pcs: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
    };
    let code = result.payload.active().code;
    if code == FrameCode::InvalidDelegation {
        result.diagnostics.push((0, DiagnosticKind::InvalidOpcode));
        failure(&mut result, context, 0);
        return result;
    }
    if code == FrameCode::Empty {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            ByteArray::empty(),
            context,
            0,
        );
        return result;
    }
    let program = program(world, result.payload.active().key.code_address)
        .expect("runtime frames originate from resolved world code");
    let block_id = result.payload.active().key.block;
    let Some(block) = program.blocks().get(block_id) else {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            ByteArray::empty(),
            context,
            program.byte_len(),
        );
        return result;
    };
    for instruction in &block.instructions {
        let pc = instruction.pc;
        let op = instruction.opcode;
        if !context.budget.charge(1) {
            boundary(&mut result, pc, FrontierReason::Work);
            return result;
        }
        result.executed_pcs.push(pc);
        if !instruction.is_valid() {
            result.diagnostics.push((pc, DiagnosticKind::InvalidOpcode));
            failure(&mut result, context, pc);
            return result;
        }
        let (inputs, outputs) = instruction.stack_io();
        let stack = &mut result.payload.active_mut().stack;
        if stack.len() < inputs {
            result
                .diagnostics
                .push((pc, DiagnosticKind::StackUnderflow));
            failure(&mut result, context, pc);
            return result;
        }
        if stack.len() - inputs + outputs > 1024 {
            result.diagnostics.push((pc, DiagnosticKind::StackOverflow));
            failure(&mut result, context, pc);
            return result;
        }
        if let Some(value) = instruction.immediate {
            stack.push(Value::constant(value));
            continue;
        }
        if (opcode::DUP1..=opcode::DUP16).contains(&op) {
            let depth = usize::from(op - opcode::DUP1 + 1);
            stack.push(stack[stack.len() - depth].clone());
            continue;
        }
        if (opcode::SWAP1..=opcode::SWAP16).contains(&op) {
            let top = stack.len() - 1;
            stack.swap(top, top - usize::from(op - opcode::SWAP1 + 1));
            continue;
        }
        let mut args: Vec<Value> = stack.drain(stack.len() - inputs..).collect();
        args.reverse();
        if !context.budget.charge(operation_work(
            &result, op, &args, program, domain, config, world,
        )) {
            boundary(&mut result, pc, FrontierReason::Work);
            return result;
        }
        if result.payload.active().key.is_static
            && matches!(
                op,
                opcode::SSTORE
                    | opcode::TSTORE
                    | opcode::CREATE
                    | opcode::CREATE2
                    | opcode::SELFDESTRUCT
                    | opcode::LOG0..=opcode::LOG4
            )
        {
            failure(&mut result, context, pc);
            return result;
        }
        match op {
            opcode::JUMP | opcode::JUMPI => {
                let frame = result.payload.active_mut();
                frame.key.jump_history.push(block.start_pc);
                let discard = frame
                    .key
                    .jump_history
                    .len()
                    .saturating_sub(config.analysis.context_depth);
                frame.key.jump_history.drain(..discard);
                let condition = args.get(1);
                if condition.is_none_or(Value::may_be_nonzero) {
                    let kind = if op == opcode::JUMP {
                        EdgeKind::Jump
                    } else {
                        EdgeKind::BranchTrue
                    };
                    match args[0].constants() {
                        Some(targets) => {
                            for target in targets {
                                if !context.budget.charge(1) {
                                    boundary(&mut result, pc, FrontierReason::Work);
                                    break;
                                }
                                let valid = usize::try_from(*target).ok().and_then(|target| {
                                    program.jumpdest_blocks().get(&target).copied()
                                });
                                if let Some(target) = valid {
                                    let payload = result.payload.clone();
                                    successor(&mut result, payload, target, kind);
                                } else {
                                    result.diagnostics.push((pc, DiagnosticKind::InvalidJump));
                                    failure(&mut result, context, pc);
                                }
                            }
                        }
                        None => {
                            result.diagnostics.push((pc, DiagnosticKind::UnknownJump));
                            failure(&mut result, context, pc);
                            for target in program.jumpdest_blocks().values() {
                                if !context.budget.charge(1) {
                                    boundary(&mut result, pc, FrontierReason::Work);
                                    break;
                                }
                                let payload = result.payload.clone();
                                successor(&mut result, payload, *target, kind);
                            }
                        }
                    }
                }
                if condition.is_some_and(Value::may_be_zero) {
                    if block_id + 1 < program.blocks().len() {
                        let payload = result.payload.clone();
                        successor(&mut result, payload, block_id + 1, EdgeKind::BranchFalse);
                    } else {
                        let payload = result.payload.clone();
                        finish(
                            &mut result,
                            payload,
                            OutcomeKind::Return,
                            ByteArray::empty(),
                            context,
                            pc,
                        );
                    }
                }
                return result;
            }
            opcode::CALL | opcode::CALLCODE | opcode::DELEGATECALL | opcode::STATICCALL => {
                call(&mut result, world, op, &args, program, context, pc);
                return result;
            }
            opcode::STOP => {
                let payload = result.payload.clone();
                finish(
                    &mut result,
                    payload,
                    OutcomeKind::Return,
                    ByteArray::empty(),
                    context,
                    pc,
                );
                return result;
            }
            opcode::RETURN | opcode::REVERT => {
                if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                    return result;
                }
                let data = match result.payload.active().memory.slice(
                    &args[0],
                    &args[1],
                    config.max_memory_bytes,
                    domain,
                ) {
                    Ok(data) => data,
                    Err(_) => {
                        boundary(&mut result, pc, FrontierReason::Memory);
                        return result;
                    }
                };
                let payload = result.payload.clone();
                finish(
                    &mut result,
                    payload,
                    if op == opcode::RETURN {
                        OutcomeKind::Return
                    } else {
                        OutcomeKind::Revert
                    },
                    data,
                    context,
                    pc,
                );
                return result;
            }
            opcode::CREATE | opcode::CREATE2 | opcode::SELFDESTRUCT => {
                boundary(&mut result, pc, FrontierReason::UnsupportedOpcode(op));
                return result;
            }
            opcode::SSTORE | opcode::TSTORE => {
                let state_address = result.payload.active().key.address;
                if op == opcode::SSTORE {
                    result
                        .payload
                        .store
                        .write(state_address, &args[0], &args[1], domain);
                } else {
                    result
                        .payload
                        .store
                        .write_transient(state_address, &args[0], &args[1], domain);
                }
            }
            opcode::MSTORE | opcode::MSTORE8 => {
                let memory = &mut result.payload.active_mut().memory;
                let write = if op == opcode::MSTORE {
                    memory.write_word(&args[0], &args[1], config.max_memory_bytes, domain)
                } else {
                    memory.write_byte(&args[0], &args[1], config.max_memory_bytes, domain)
                };
                if write.is_err() {
                    boundary(&mut result, pc, FrontierReason::Memory);
                    return result;
                }
            }
            opcode::CALLDATACOPY | opcode::CODECOPY | opcode::RETURNDATACOPY | opcode::MCOPY => {
                let source = match op {
                    opcode::CALLDATACOPY => result.payload.active().calldata.clone(),
                    opcode::CODECOPY => ByteArray::exact(program.bytes()),
                    opcode::RETURNDATACOPY => result.payload.active().returndata.clone(),
                    _ => result.payload.active().memory.clone(),
                };
                if op == opcode::RETURNDATACOPY {
                    let (valid, invalid) = return_copy_bounds(&args[1], &args[2], source.len());
                    if invalid {
                        failure(&mut result, context, pc);
                    }
                    if !valid {
                        return result;
                    }
                }
                if op == opcode::MCOPY
                    && !touch_memory(&mut result, &args[1], &args[2], context, pc)
                {
                    return result;
                }
                if result
                    .payload
                    .active_mut()
                    .memory
                    .copy_from(
                        &args[0],
                        &source,
                        &args[1],
                        &args[2],
                        config.max_memory_bytes,
                        domain,
                    )
                    .is_err()
                {
                    boundary(&mut result, pc, FrontierReason::Memory);
                    return result;
                }
            }
            opcode::EXTCODECOPY => {
                let source = match args[0].constants() {
                    Some(targets) => targets
                        .iter()
                        .map(|target| {
                            code_bytes(world, address(*target))
                                .map(|bytes| ByteArray::exact(&bytes))
                                .unwrap_or_else(ByteArray::unknown)
                        })
                        .reduce(|a, b| a.join(&b, domain))
                        .expect("nonempty value"),
                    None => ByteArray::unknown(),
                };
                if result
                    .payload
                    .active_mut()
                    .memory
                    .copy_from(
                        &args[1],
                        &source,
                        &args[2],
                        &args[3],
                        config.max_memory_bytes,
                        domain,
                    )
                    .is_err()
                {
                    boundary(&mut result, pc, FrontierReason::Memory);
                    return result;
                }
            }
            opcode::LOG0..=opcode::LOG4 => {
                if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                    return result;
                }
                let data = match result.payload.active().memory.slice(
                    &args[0],
                    &args[1],
                    config.max_memory_bytes,
                    domain,
                ) {
                    Ok(data) => data,
                    Err(_) => {
                        boundary(&mut result, pc, FrontierReason::Memory);
                        return result;
                    }
                };
                let frame = result.payload.active();
                let source = LogKey {
                    address: frame.key.address,
                    code_address: frame.key.code_address,
                    pc,
                };
                if result
                    .payload
                    .store
                    .emit_log(
                        source,
                        AbstractLog {
                            topics: args[2..].to_vec(),
                            data,
                        },
                        domain,
                    )
                    .is_err()
                {
                    boundary(&mut result, pc, FrontierReason::UnsupportedOpcode(op));
                    return result;
                }
            }
            _ if outputs > 0 => {
                let frame = result.payload.active();
                let value = match op {
                    opcode::ADDRESS if !config.symbolic_entry_environment => {
                        address_value(frame.key.address)
                    }
                    opcode::CALLER if !config.symbolic_entry_environment => {
                        address_value(frame.key.caller)
                    }
                    opcode::ORIGIN if !config.symbolic_entry_environment => {
                        address_value(entry.caller)
                    }
                    opcode::CALLVALUE => frame.call_value.clone(),
                    opcode::PC => constant(pc),
                    opcode::CODESIZE => constant(program.byte_len()),
                    opcode::CALLDATASIZE => frame.calldata.len().clone(),
                    opcode::RETURNDATASIZE => frame.returndata.len().clone(),
                    opcode::MSIZE => frame.memory.len().clone(),
                    opcode::CALLDATALOAD => frame.calldata.read_word(&args[0], domain),
                    opcode::SLOAD => result
                        .payload
                        .store
                        .read(frame.key.address, &args[0], domain),
                    opcode::TLOAD => {
                        result
                            .payload
                            .store
                            .read_transient(frame.key.address, &args[0], domain)
                    }
                    opcode::SELFBALANCE => result.payload.store.read_balance(frame.key.address),
                    opcode::BALANCE => environment_targets(&args[0], domain, |address| {
                        result.payload.store.read_balance(address)
                    }),
                    opcode::EXTCODESIZE => environment_targets(&args[0], domain, |target| {
                        match world.account(target).map(|account| &account.code) {
                            Some(Code::Runtime(program)) => constant(program.byte_len()),
                            Some(Code::Delegation(_)) => constant(23),
                            Some(Code::Empty) => zero(),
                            _ => Value::top(),
                        }
                    }),
                    opcode::EXTCODEHASH => environment_targets(&args[0], domain, |target| {
                        code_bytes(world, target)
                            .map(|bytes| {
                                let hash = Value::constant(U256::from_be_slice(
                                    keccak256(&bytes).as_slice(),
                                ));
                                if bytes.is_empty()
                                    && result.payload.store.read_balance(target).may_be_zero()
                                {
                                    domain.join(&hash, &zero())
                                } else {
                                    hash
                                }
                            })
                            .unwrap_or_else(Value::top)
                    }),
                    opcode::MLOAD => {
                        if !touch_memory(&mut result, &args[0], &constant(32), context, pc) {
                            return result;
                        }
                        result.payload.active().memory.read_word(&args[0], domain)
                    }
                    opcode::KECCAK256 => {
                        if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                            return result;
                        }
                        match result.payload.active().memory.slice(
                            &args[0],
                            &args[1],
                            config.max_memory_bytes,
                            domain,
                        ) {
                            Ok(bytes) => bytes
                                .exact_bytes_bounded(config.max_memory_bytes)
                                .map(|bytes| {
                                    Value::constant(U256::from_be_slice(
                                        keccak256(bytes).as_slice(),
                                    ))
                                })
                                .unwrap_or_else(Value::top),
                            Err(_) => {
                                boundary(&mut result, pc, FrontierReason::Memory);
                                return result;
                            }
                        }
                    }
                    _ => {
                        if !matches!(op, 0x01..=0x0b | 0x10..=0x1e) {
                            result.diagnostics.push((pc, DiagnosticKind::OpaqueResult));
                        }
                        domain.apply(op, &args)
                    }
                };
                result
                    .payload
                    .active_mut()
                    .stack
                    .extend(std::iter::repeat_n(value, outputs));
            }
            _ => {}
        }
    }
    if block_id + 1 < program.blocks().len() {
        let payload = result.payload.clone();
        successor(&mut result, payload, block_id + 1, EdgeKind::Fallthrough);
    } else {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            ByteArray::empty(),
            context,
            program.byte_len(),
        );
    }
    result
}

fn return_copy_bounds(offset: &Value, size: &Value, length: &Value) -> (bool, bool) {
    let (Some(offsets), Some(sizes), Some(lengths)) =
        (offset.constants(), size.constants(), length.constants())
    else {
        return (true, true);
    };
    let mut valid = false;
    let mut invalid = false;
    for offset in offsets {
        for size in sizes {
            for length in lengths {
                let fits = offset.checked_add(*size).is_some_and(|end| end <= *length);
                valid |= fits;
                invalid |= !fits;
            }
        }
    }
    (valid, invalid)
}
