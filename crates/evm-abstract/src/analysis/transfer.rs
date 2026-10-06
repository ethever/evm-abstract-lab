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
    domain::{Domain, Profile, Value, provenance::Origin},
    world::{AbstractLog, AddressInput, ByteArray, Code, Entry, LogKey, Store, Symbol, World},
};
use alloy_primitives::{Address, U256, keccak256};
use revm_bytecode::opcode;
use std::sync::atomic::{AtomicU64, Ordering};

// 仅用于本进程的临时复制身份。计数耗尽时丢弃这项可选精度，不复用编号。
static NEXT_IDENTITY_SCOPE: AtomicU64 = AtomicU64::new(1);
fn identity_scope() -> Option<u64> {
    NEXT_IDENTITY_SCOPE
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .ok()
}

mod budget;
mod calls;
pub(super) mod create;
mod precompile;

pub(super) use crate::resource::WorkBudget;
use budget::{byte_work, operation_work};
use calls::{call, failure, finish};

struct TransferContext<'a> {
    config: &'a ExecutionConfig,
    domain: Domain,
    budget: &'a mut WorkBudget,
    root_address: Option<(Address, AddressInput)>,
    input_scope: Option<u64>,
}

pub(super) fn initial(
    world: &World,
    entry: &Entry,
    domain: Domain,
) -> Result<MachinePayload, FrontierReason> {
    calls::initial(world, entry, domain)
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
    pub completed_calls: Vec<CompletedCall>,
}

/// A specific child terminal before caller joins, with qualified world effects.
#[derive(Clone, Debug)]
pub(super) struct CompletedCall {
    pub payload: MachinePayload,
    pub kind: OutcomeKind,
    pub data: ByteArray,
    pub output_store: Store,
    pub output_kind: OutcomeKind,
    pub output_data: ByteArray,
}

pub(super) fn resume_summary(
    payload: MachinePayload,
    kind: OutcomeKind,
    data: ByteArray,
    config: &ExecutionConfig,
    domain: Domain,
    budget: &mut WorkBudget,
) -> Execution {
    let mut result = Execution {
        payload: payload.clone(),
        executed_pcs: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
    };
    let mut context = TransferContext {
        config,
        domain,
        budget,
        root_address: None,
        input_scope: None,
    };
    let pc = payload
        .active()
        .program
        .as_ref()
        .and_then(|p| p.blocks().get(payload.active().key.basic_block_index))
        .and_then(|b| b.instructions.last())
        .map_or(0, |i| i.pc);
    finish(&mut result, payload, kind, data, &mut context, pc);
    result
}

fn address(value: U256) -> Address {
    Address::from_slice(&value.to_be_bytes::<32>()[12..])
}
fn constant(value: usize) -> Value {
    Value::constant(U256::from(value))
}
fn zero() -> Value {
    Value::constant(U256::ZERO)
}
fn boundary(result: &mut Execution, pc: usize, reason: FrontierReason) {
    result
        .frontiers
        .push((pc, reason, Some(result.payload.key())));
}

fn successor(
    result: &mut Execution,
    mut payload: MachinePayload,
    basic_block_index: usize,
    kind: EdgeKind,
) {
    payload.active_mut().key.basic_block_index = basic_block_index;
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

fn code_bytes(store: &Store, target: Address) -> Option<Vec<u8>> {
    store.raw_account_code(target)
}

fn environment_targets(
    value: &Value,
    domain: Domain,
    entry: &Entry,
    mut read: impl FnMut(Address) -> Value,
) -> Value {
    let symbolic_owner = entry.environment.to.as_concrete().is_none();
    if symbolic_owner
        && value.provenance().same_identity(
            entry
                .environment
                .address_value(entry.environment.to)
                .provenance(),
        )
    {
        return read(entry.address);
    }
    let Some(values) = value.constants() else {
        return Value::top();
    };
    values
        .iter()
        .map(|v| {
            let target = address(*v);
            if symbolic_owner && target == entry.address {
                Value::top()
            } else {
                read(target)
            }
        })
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
        root_address: Some((entry.address, entry.environment.to)),
        input_scope: entry.environment.input_scope.id(),
    };
    let context = &mut transfer_context;
    let mut result = Execution {
        payload,
        executed_pcs: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
    };
    let scope = identity_scope();
    let mut definition = 0_u32;
    for value in &mut result.payload.active_mut().stack {
        value.forget_identity();
        if domain.spec().profile() == Profile::Product {
            if let Some(scope) = scope {
                *value = value.clone().with_identity(scope, definition);
            }
            definition = definition.saturating_add(1);
        }
    }
    let code = result.payload.active().code;
    if let FrameCode::Precompile(address) = code {
        precompile::execute(&mut result, world.fork(), address, context);
        return result;
    }
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
    let program = result
        .payload
        .active()
        .program
        .clone()
        .expect("runtime and initcode frames capture their executable program");
    let program = &program;
    let basic_block_index = result.payload.active().key.basic_block_index;
    let Some(block) = program.blocks().get(basic_block_index) else {
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
        if matches!(op, opcode::MLOAD | opcode::CALLDATALOAD) {
            let scan = args[0]
                .constants()
                .map_or(1, |s| s.len())
                .saturating_mul(32)
                .saturating_mul(domain.capacity().saturating_add(32));
            if !context.budget.charge(scan) {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
        }
        if !context.budget.charge(operation_work(
            &result, op, &args, program, domain, config, entry,
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
                            for (target_pc, target) in program.jumpdest_blocks() {
                                if !args[0].contains(U256::from(*target_pc)) {
                                    continue;
                                }
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
                    if basic_block_index + 1 < program.blocks().len() {
                        let payload = result.payload.clone();
                        successor(
                            &mut result,
                            payload,
                            basic_block_index + 1,
                            EdgeKind::BranchFalse,
                        );
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
                result.payload.active_mut().returndata = ByteArray::empty();
                call(&mut result, world, op, &args, program, context, pc);
                return result;
            }
            opcode::CREATE | opcode::CREATE2 => {
                result.payload.active_mut().returndata = ByteArray::empty();
                create::create(&mut result, world, op, &args, program, context, pc);
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
            opcode::SELFDESTRUCT => {
                if result
                    .payload
                    .active()
                    .key
                    .address_value
                    .as_concrete()
                    .is_none()
                {
                    // A literal beneficiary can alias an unknown logical owner;
                    // the internal store namespace is not an address observation.
                    boundary(&mut result, pc, FrontierReason::UnknownTarget);
                    return result;
                }
                let Some(beneficiaries) = args[0].constants() else {
                    boundary(&mut result, pc, FrontierReason::UnknownTarget);
                    return result;
                };
                let owner = result.payload.active().key.address;
                for beneficiary in beneficiaries {
                    if !context
                        .budget
                        .charge(result.payload.work_size().saturating_add(1))
                    {
                        boundary(&mut result, pc, FrontierReason::Work);
                        break;
                    }
                    let mut payload = result.payload.clone();
                    payload
                        .store
                        .selfdestruct(owner, address(*beneficiary), domain);
                    finish(
                        &mut result,
                        payload,
                        OutcomeKind::Return,
                        ByteArray::empty(),
                        context,
                        pc,
                    );
                }
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
                let symbolic_owner = entry.environment.to.as_concrete().is_none();
                let exact_self = symbolic_owner
                    && args[0].provenance().same_identity(
                        entry
                            .environment
                            .address_value(entry.environment.to)
                            .provenance(),
                    );
                let source = if exact_self {
                    code_bytes(&result.payload.store, entry.address)
                        .map(|bytes| ByteArray::exact(&bytes))
                        .unwrap_or_else(ByteArray::unknown)
                } else {
                    match args[0].constants() {
                        Some(targets) => targets
                            .iter()
                            .map(|target| {
                                let target = address(*target);
                                if symbolic_owner && target == entry.address {
                                    ByteArray::unknown()
                                } else {
                                    code_bytes(&result.payload.store, target)
                                        .map(|bytes| ByteArray::exact(&bytes))
                                        .unwrap_or_else(ByteArray::unknown)
                                }
                            })
                            .reduce(|a, b| a.join(&b, domain))
                            .expect("nonempty value"),
                        None => ByteArray::unknown(),
                    }
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
                let mut value = match op {
                    opcode::ADDRESS => entry.environment.address_value(frame.key.address_value),
                    opcode::CALLER => entry.environment.address_value(frame.key.caller),
                    opcode::ORIGIN => entry
                        .environment
                        .address_value(entry.environment.resolved_origin()),
                    opcode::CALLVALUE => frame.call_value.clone(),
                    opcode::GASPRICE => entry
                        .environment
                        .gas_price
                        .clone()
                        .with_symbol(Symbol::GasPrice, entry.environment.input_scope.id()),
                    opcode::COINBASE => entry.environment.address_value(entry.environment.coinbase),
                    opcode::TIMESTAMP => entry
                        .environment
                        .timestamp
                        .clone()
                        .with_symbol(Symbol::Timestamp, entry.environment.input_scope.id()),
                    opcode::NUMBER => entry
                        .environment
                        .number
                        .clone()
                        .with_symbol(Symbol::Number, entry.environment.input_scope.id()),
                    opcode::DIFFICULTY => entry
                        .environment
                        .prevrandao
                        .clone()
                        .with_symbol(Symbol::Prevrandao, entry.environment.input_scope.id()),
                    opcode::GASLIMIT => entry
                        .environment
                        .gas_limit
                        .clone()
                        .with_symbol(Symbol::GasLimit, entry.environment.input_scope.id()),
                    opcode::CHAINID => entry.environment.chain_id(world.identity()),
                    opcode::BASEFEE => entry
                        .environment
                        .base_fee
                        .clone()
                        .with_symbol(Symbol::BaseFee, entry.environment.input_scope.id()),
                    opcode::BLOBBASEFEE => entry
                        .environment
                        .blob_base_fee
                        .clone()
                        .with_symbol(Symbol::BlobBaseFee, entry.environment.input_scope.id()),
                    opcode::BLOCKHASH => entry.environment.block_hash(&args[0], domain),
                    opcode::BLOBHASH => entry.environment.blob_hashes.get(
                        &args[0],
                        domain,
                        entry.environment.input_scope.id(),
                    ),
                    opcode::GAS => entry.environment.gas.value(),
                    opcode::PC => constant(pc),
                    opcode::CODESIZE => constant(program.byte_len()),
                    opcode::CALLDATASIZE => {
                        let value = frame.calldata.len().clone();
                        if frame.environment_calldata {
                            value.with_symbol(
                                Symbol::CalldataLength,
                                entry.environment.input_scope.id(),
                            )
                        } else {
                            value
                        }
                    }
                    opcode::RETURNDATASIZE => frame.returndata.len().clone(),
                    opcode::MSIZE => frame.memory.len().clone(),
                    opcode::CALLDATALOAD => {
                        let value = frame.calldata.read_word(&args[0], domain);
                        if frame.environment_calldata {
                            args[0].singleton().map_or(value.clone(), |offset| {
                                value.with_symbol(
                                    Symbol::CalldataWord(offset),
                                    entry.environment.input_scope.id(),
                                )
                            })
                        } else {
                            value
                        }
                    }
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
                    opcode::BALANCE => environment_targets(&args[0], domain, entry, |address| {
                        result.payload.store.read_balance(address)
                    }),
                    opcode::EXTCODESIZE => environment_targets(&args[0], domain, entry, |target| {
                        match result.payload.store.code(target) {
                            Some(Code::Runtime(program)) => constant(program.byte_len()),
                            Some(Code::Delegation(_)) => constant(23),
                            Some(Code::Empty) => zero(),
                            _ => Value::top(),
                        }
                    }),
                    opcode::EXTCODEHASH => environment_targets(&args[0], domain, entry, |target| {
                        code_bytes(&result.payload.store, target)
                            .map(|bytes| {
                                let hash = Value::constant(U256::from_be_slice(
                                    keccak256(&bytes).as_slice(),
                                ));
                                if !bytes.is_empty() {
                                    return hash;
                                }
                                let balance = result.payload.store.read_balance(target);
                                let nonce = result.payload.store.nonce(target);
                                let may_be_empty = balance.may_be_zero() && nonce.may_be_zero();
                                let may_exist = balance.may_be_nonzero() || nonce.may_be_nonzero();
                                if may_be_empty && may_exist {
                                    domain.join(&hash, &zero())
                                } else if may_be_empty {
                                    zero()
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
                        let reduction = domain.apply_detailed(op, &args);
                        if matches!(
                            reduction.status,
                            crate::domain::ReductionStatus::RoundLimit
                                | crate::domain::ReductionStatus::FactLimit
                        ) {
                            result
                                .diagnostics
                                .push((pc, DiagnosticKind::FactExchangeLimited(reduction.status)));
                        }
                        reduction.value
                    }
                };
                value = domain.project(&value);
                if domain.spec().profile() == Profile::Product {
                    let origin = match op {
                        opcode::ADDRESS | opcode::CALLER | opcode::ORIGIN => Some(Origin::Address),
                        opcode::CALLDATALOAD | opcode::CALLDATASIZE => Some(Origin::Calldata),
                        opcode::MLOAD | opcode::MSIZE => Some(Origin::Memory),
                        opcode::SLOAD => Some(Origin::Storage),
                        opcode::TLOAD => Some(Origin::TransientStorage),
                        opcode::CALLVALUE => Some(Origin::CallValue),
                        opcode::BALANCE | opcode::SELFBALANCE => Some(Origin::Balance),
                        opcode::RETURNDATASIZE => Some(Origin::Returndata),
                        0x01..=0x0b | 0x10..=0x1e => None,
                        _ => Some(Origin::Environment),
                    };
                    if let Some(origin) = origin {
                        value.set_origin(origin);
                    }
                    if let Some(scope) = scope {
                        value = value.with_identity(scope, definition);
                    }
                    if let Some(next) = definition.checked_add(1) {
                        definition = next;
                    } else {
                        value.forget_identity();
                    }
                }
                result
                    .payload
                    .active_mut()
                    .stack
                    .extend(std::iter::repeat_n(value, outputs));
            }
            _ => {}
        }
    }
    if basic_block_index + 1 < program.blocks().len() {
        let payload = result.payload.clone();
        successor(
            &mut result,
            payload,
            basic_block_index + 1,
            EdgeKind::Fallthrough,
        );
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
