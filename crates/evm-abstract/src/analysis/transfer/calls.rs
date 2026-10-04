//! Account-code resolution, frame entry and rollback-aware caller resumption.
use super::budget::{arithmetic_work, byte_work, maximum};
use super::{
    Execution, Outcome, Successor, TransferContext, address, boundary, touch_memory, zero,
};
use crate::{
    analysis::{
        Continuation, Frame, FrameCode, FrameKey, FrontierReason, MachineEdgeKind, MachinePayload,
        OutcomeKind,
    },
    bytecode::Program,
    domain::Value,
    world::{ByteArray, Code, Entry, Store, World},
};
use alloy_primitives::{Address, U256};
use revm_bytecode::opcode;

fn precompile(world: &World, address: Address) -> bool {
    let number = U256::from_be_slice(address.as_slice());
    (number > U256::ZERO
        && number
            <= U256::from(if world.fork().supports_delegation() {
                0x11
            } else {
                0x0a
            }))
        || (world.fork() == crate::Fork::Osaka && number == U256::from(0x100))
}

/// 7702 follows exactly one pointer. The second marker is invalid executable
/// EF code; a delegated precompile target executes as empty code.
fn resolve(world: &World, target: Address) -> Result<(Address, FrameCode), FrontierReason> {
    if precompile(world, target) {
        return Err(FrontierReason::Precompile(target));
    }
    match world.account(target).map(|a| &a.code) {
        Some(Code::Runtime(_)) => Ok((target, FrameCode::Runtime)),
        Some(Code::Empty) => Ok((target, FrameCode::Empty)),
        Some(Code::Delegation(implementation)) => {
            if precompile(world, *implementation) {
                return Ok((*implementation, FrameCode::Empty));
            }
            match world.account(*implementation).map(|a| &a.code) {
                Some(Code::Runtime(_)) => Ok((*implementation, FrameCode::Runtime)),
                Some(Code::Empty) => Ok((*implementation, FrameCode::Empty)),
                Some(Code::Delegation(_)) => Ok((*implementation, FrameCode::InvalidDelegation)),
                _ => Err(FrontierReason::MissingCode(*implementation)),
            }
        }
        _ => Err(FrontierReason::MissingCode(target)),
    }
}

pub(super) fn initial(world: &World, entry: &Entry) -> Result<MachinePayload, FrontierReason> {
    let (code_address, code) = resolve(world, entry.address)?;
    let store = Store::new(world);
    Ok(MachinePayload {
        frames: vec![Frame {
            key: FrameKey {
                code_address,
                address: entry.address,
                caller: entry.caller,
                is_static: entry.is_static,
                block: 0,
                stack_height: 0,
                jump_history: Vec::new(),
            },
            code,
            stack: Vec::new(),
            memory: ByteArray::memory(),
            calldata: entry.calldata.clone(),
            returndata: ByteArray::empty(),
            call_value: entry.value.clone(),
            saved_store: Some(store.clone()),
            continuation: None,
        }],
        store,
    })
}

pub(super) fn finish(
    result: &mut Execution,
    mut payload: MachinePayload,
    kind: OutcomeKind,
    data: ByteArray,
    context: &mut TransferContext<'_>,
    pc: usize,
) {
    let config = context.config;
    let domain = context.domain;
    if let Some(frame) = payload.frames.last()
        && let Some(continuation) = &frame.continuation
    {
        let parent = &payload.frames[payload.frames.len() - 2];
        let cost = byte_work(
            &[&continuation.output_offset, &continuation.output_size],
            maximum(&continuation.output_size),
            parent.memory.work_size().saturating_add(data.work_size()),
            domain,
        );
        if !context.budget.charge(cost) {
            result
                .frontiers
                .push((pc, FrontierReason::Work, Some(payload.key())));
            return;
        }
    }
    let frame = payload.frames.pop().expect("finish has a frame");
    if kind != OutcomeKind::Return
        && let Some(saved) = frame.saved_store
    {
        payload.store = saved;
    }
    let Some(continuation) = frame.continuation else {
        result.outcomes.push(Outcome {
            kind,
            data,
            store: payload.store,
        });
        return;
    };
    let parent = payload.active_mut();
    parent.returndata = data;
    // Failure has an empty buffer. Revert exposes revert data, but both report 0.
    parent.stack.push(Value::constant(U256::from(u8::from(
        kind == OutcomeKind::Return,
    ))));
    if parent
        .memory
        .copy_return_data(
            &continuation.output_offset,
            &parent.returndata,
            &continuation.output_size,
            config.max_memory_bytes,
            domain,
        )
        .is_err()
    {
        result
            .frontiers
            .push((pc, FrontierReason::Memory, Some(payload.key())));
        return;
    }
    parent.key.block = continuation
        .return_block
        .expect("call continuations include synthetic end blocks");
    payload.normalize();
    result.successors.push(Successor {
        payload,
        kind: match kind {
            OutcomeKind::Return => MachineEdgeKind::Return,
            OutcomeKind::Revert => MachineEdgeKind::Revert,
            OutcomeKind::Failure => MachineEdgeKind::Failure,
        },
    });
}

pub(super) fn failure(result: &mut Execution, context: &mut TransferContext<'_>, pc: usize) {
    finish(
        result,
        result.payload.clone(),
        OutcomeKind::Failure,
        ByteArray::empty(),
        context,
        pc,
    );
}

fn immediate_call_failure(result: &mut Execution, mut payload: MachinePayload, next: usize) {
    let caller = payload.active_mut();
    caller.stack.push(zero());
    caller.returndata = ByteArray::empty();
    caller.key.block = next;
    payload.normalize();
    result.successors.push(Successor {
        payload,
        kind: MachineEdgeKind::Failure,
    });
}

pub(super) fn call(
    result: &mut Execution,
    world: &World,
    op: u8,
    args: &[Value],
    program: &Program,
    context: &mut TransferContext<'_>,
    pc: usize,
) {
    let config = context.config;
    let domain = context.domain;
    let with_value = matches!(op, opcode::CALL | opcode::CALLCODE);
    let (value, input, output) = if with_value {
        (args[2].clone(), 3, 5)
    } else {
        (zero(), 2, 4)
    };
    if !touch_memory(result, &args[input], &args[input + 1], context, pc)
        || !touch_memory(result, &args[output], &args[output + 1], context, pc)
    {
        return;
    }
    let next = result.payload.active().key.block + 1;
    // The decoder splits after every CALL. An end-of-code continuation is a
    // synthetic empty block, making each return pop exactly one frame.
    debug_assert!(next <= program.blocks().len());
    if result.payload.active().key.is_static && op == opcode::CALL && value.may_be_nonzero() {
        failure(result, context, pc);
        if !value.may_be_zero() {
            return;
        }
    }
    // Gas and (possibly abstract) balance can reject the call before entry.
    // This failure preserves all caller writes and clears returndata.
    immediate_call_failure(result, result.payload.clone(), next);
    let value = if result.payload.active().key.is_static && op == opcode::CALL {
        zero()
    } else {
        value
    };
    if !context.budget.charge(byte_work(
        &[&args[input], &args[input + 1]],
        maximum(&args[input + 1]),
        result.payload.active().memory.work_size(),
        domain,
    )) {
        boundary(result, pc, FrontierReason::Work);
        return;
    }
    let mut targets = if let Some(values) = args[1].constants() {
        values.iter().map(|v| address(*v)).collect::<Vec<_>>()
    } else {
        boundary(result, pc, FrontierReason::UnknownTarget);
        if !context.budget.charge(world.accounts().len()) {
            boundary(result, pc, FrontierReason::Work);
            return;
        }
        world.accounts().keys().copied().collect::<Vec<_>>()
    };
    targets.sort();
    targets.dedup();
    for target in targets {
        if !context.budget.charge(
            result
                .payload
                .work_size()
                .saturating_add(result.payload.store.work_size())
                .saturating_add(1),
        ) {
            boundary(result, pc, FrontierReason::Work);
            break;
        }
        let (code_address, code) = match resolve(world, target) {
            Ok(resolution) => resolution,
            Err(reason) => {
                boundary(result, pc, reason);
                continue;
            }
        };
        if result.payload.frames.len() >= config.max_call_depth {
            boundary(result, pc, FrontierReason::CallDepth);
            continue;
        }
        let caller = result.payload.active();
        let caller_address = caller.key.address;
        let balance = result.payload.store.read_balance(caller_address);
        if with_value
            && let (Some(balances), Some(values)) = (balance.constants(), value.constants())
            && balances
                .iter()
                .all(|balance| values.iter().all(|value| value > balance))
        {
            continue;
        }
        let calldata = match caller.memory.slice(
            &args[input],
            &args[input + 1],
            config.max_memory_bytes,
            domain,
        ) {
            Ok(data) => data,
            Err(_) => {
                boundary(result, pc, FrontierReason::Memory);
                continue;
            }
        };
        let (state_address, frame_caller, call_value) = match op {
            opcode::CALL => (target, caller_address, value.clone()),
            opcode::STATICCALL => (target, caller_address, zero()),
            opcode::CALLCODE => (caller_address, caller_address, value.clone()),
            opcode::DELEGATECALL => (caller_address, caller.key.caller, caller.call_value.clone()),
            _ => unreachable!(),
        };
        let saved_store = result.payload.store.clone();
        let mut payload = result.payload.clone();
        if op == opcode::CALL && target != caller_address {
            let recipient = payload.store.read_balance(target);
            if !context.budget.charge(
                arithmetic_work(&[balance.clone(), value.clone()])
                    .saturating_add(arithmetic_work(&[recipient.clone(), value.clone()])),
            ) {
                boundary(result, pc, FrontierReason::Work);
                continue;
            }
            payload.store.write_balance(
                caller_address,
                domain.apply(opcode::SUB, &[balance, value.clone()]),
            );
            payload.store.write_balance(
                target,
                domain.apply(opcode::ADD, &[recipient, value.clone()]),
            );
        }
        payload.frames.push(Frame {
            key: FrameKey {
                code_address,
                address: state_address,
                caller: frame_caller,
                is_static: caller.key.is_static || op == opcode::STATICCALL,
                block: 0,
                stack_height: 0,
                jump_history: Vec::new(),
            },
            code,
            stack: Vec::new(),
            memory: ByteArray::memory(),
            calldata,
            returndata: ByteArray::empty(),
            call_value,
            saved_store: Some(saved_store),
            continuation: Some(Continuation {
                return_block: Some(next),
                output_offset: args[output].clone(),
                output_size: args[output + 1].clone(),
            }),
        });
        payload.normalize();
        result.successors.push(Successor {
            payload,
            kind: MachineEdgeKind::Call,
        });
    }
}
