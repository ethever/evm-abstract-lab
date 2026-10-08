//! Account-code resolution, frame entry and rollback-aware caller resumption.
use super::budget::{byte_work, maximum};
use super::{
    CompletedCall, Execution, Outcome, Successor, TransferContext, address, boundary, touch_memory,
    zero,
};
use crate::{
    analysis::{
        CallStack, ChildFrame, Continuation, FrameCode, FrameKey, FrameState, FrontierReason,
        MachineEdgeKind, MachinePayload, OutcomeKind, RootFrame,
    },
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{AddressInput, ByteArray, Code, Entry, Store, World},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use revm_bytecode::opcode;

fn precompile(world: &World, address: Address) -> bool {
    super::precompile::contains(world.fork(), address)
}

/// 7702 follows exactly one pointer. The second marker is invalid executable
/// EF code; a delegated precompile target executes as empty code.
fn resolve(
    world: &World,
    store: &Store,
    target: Address,
) -> Result<(Address, FrameCode, Option<Program>), FrontierReason> {
    if precompile(world, target) {
        return Ok((target, FrameCode::Precompile(target), None));
    }
    match store.code(target) {
        Some(Code::Runtime(program)) => Ok((target, FrameCode::Runtime, Some(program.clone()))),
        Some(Code::Empty) => Ok((target, FrameCode::Empty, None)),
        Some(Code::Delegation(implementation)) => {
            if precompile(world, *implementation) {
                return Ok((*implementation, FrameCode::Empty, None));
            }
            match store.code(*implementation) {
                Some(Code::Runtime(program)) => {
                    Ok((*implementation, FrameCode::Runtime, Some(program.clone())))
                }
                Some(Code::Empty) => Ok((*implementation, FrameCode::Empty, None)),
                Some(Code::Delegation(_)) => {
                    Ok((*implementation, FrameCode::InvalidDelegation, None))
                }
                _ => Err(FrontierReason::MissingCode(*implementation)),
            }
        }
        _ => Err(FrontierReason::MissingCode(target)),
    }
}

fn captured_hash(
    program: &Option<Program>,
    store: &Store,
    address: Address,
    mode: FrameCode,
) -> B256 {
    program.as_ref().map_or_else(
        || {
            if mode == FrameCode::InvalidDelegation {
                keccak256(store.raw_account_code(address).unwrap_or_default())
            } else {
                keccak256([])
            }
        },
        |program| keccak256(program.bytes()),
    )
}

pub(super) fn initial(
    world: &World,
    entry: &Entry,
    domain: Domain,
) -> Result<MachinePayload, FrontierReason> {
    let mut store = Store::new(world);
    store.project(domain);
    let (code_address, code, program) = resolve(world, &store, entry.address)?;
    let code_hash = captured_hash(&program, &store, code_address, code);
    Ok(MachinePayload {
        relations: crate::domain::relational::RelationState::default(),
        call_stack: CallStack::new(RootFrame {
            state: FrameState {
                key: FrameKey {
                    code_address,
                    code_hash,
                    mode: code,
                    address: entry.address,
                    address_value: entry.environment.to,
                    caller: entry.environment.caller,
                    is_static: entry.environment.is_static,
                    basic_block_index: 0,
                    stack_height: 0,
                    jump_history: Vec::new(),
                },
                code,
                program,
                stack: Vec::new(),
                memory: ByteArray::memory(),
                calldata: entry.environment.calldata.project(domain),
                environment_calldata: true,
                returndata: ByteArray::empty(),
                call_value: domain.project(&entry.environment.value.clone().with_symbol(
                    crate::world::Symbol::CallValue,
                    entry.environment.input_scope.id(),
                )),
                saved_store: store.snapshot(),
            },
        }),
        store,
    })
}

pub(super) fn finish(
    result: &mut Execution,
    mut payload: MachinePayload,
    mut kind: OutcomeKind,
    mut data: ByteArray,
    context: &mut TransferContext<'_>,
    pc: usize,
) {
    let config = context.config;
    let domain = context.domain;
    let completed = payload
        .call_stack
        .active_child()
        .map(|_| (payload.clone(), kind, data.clone()));
    if let Err(reason) =
        super::create::finish_creation(&mut payload, &mut kind, &mut data, context, pc)
    {
        result.frontiers.push((pc, reason, Some(payload.key())));
        return;
    }
    if let Some(child) = payload.call_stack.active_child() {
        let continuation = &child.continuation;
        let parent = payload.call_stack.parent().expect("a child has a parent");
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
    let Some(child) = payload.call_stack.pop_child() else {
        if kind != OutcomeKind::Return {
            payload
                .store
                .restore(payload.call_stack.root().state.saved_store.clone());
        }
        payload.store.finalize_transaction(domain);
        result.outcomes.push(Outcome {
            kind,
            data,
            store: payload.store,
        });
        return;
    };
    if kind != OutcomeKind::Return {
        payload.store.restore(child.state.saved_store);
    }
    let continuation = child.continuation;
    let output_store = payload.store.clone();
    let output_data = data.clone();
    let output_kind = kind;
    let parent = payload.active_mut();
    parent.returndata = data;
    // Failure has an empty buffer. Revert exposes revert data, but both report 0.
    let result_value = if kind == OutcomeKind::Return {
        continuation.creation.map_or(U256::from(1), |address| {
            U256::from_be_slice(address.as_slice())
        })
    } else {
        U256::ZERO
    };
    parent.stack.push(AbstractValue::constant(result_value));
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
    parent.key.basic_block_index = continuation
        .return_block
        .expect("call continuations include synthetic end blocks");
    payload.normalize();
    if let Some((payload, kind, data)) = completed {
        result.completed_calls.push(CompletedCall {
            payload,
            kind,
            data,
            output_store,
            output_kind,
            output_data,
        });
    }
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
    caller.key.basic_block_index = next;
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
    args: &[AbstractValue],
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
    let next = result.payload.active().key.basic_block_index + 1;
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
    // The transaction entry has physical depth zero. No child is entered once
    // the EVM's 1024 depth limit is exceeded, independent of analysis budgets.
    if result.payload.call_stack.depth() > 1024 {
        return;
    }
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
    let projection_args = [
        args[1].clone(),
        AbstractValue::constant(U256::MAX >> 96usize),
    ];
    if !context
        .budget
        .charge(domain.operation_work(&projection_args))
    {
        boundary(result, pc, FrontierReason::Work);
        return;
    }
    let logical_address = result.payload.active().key.address_value;
    let exact_symbolic_self = logical_address.as_concrete().is_none()
        && args[1]
            .identity()
            .same_identity(logical_address.scoped_value(context.input_scope).identity());
    let projected = domain.address_projection(&args[1]);
    let mut targets = if exact_symbolic_self {
        vec![result.payload.active().key.address]
    } else if let Some(values) = projected.constants() {
        values.iter().map(|v| address(*v)).collect::<Vec<_>>()
    } else {
        boundary(result, pc, FrontierReason::UnknownTarget);
        if !context.budget.charge(world.accounts().len()) {
            boundary(result, pc, FrontierReason::Work);
            return;
        }
        // 开放 world 仍有外部候选，保留 UnknownTarget；只排除明确不符合投影的账户。
        world
            .accounts()
            .keys()
            .copied()
            .filter(|target| projected.contains(U256::from_be_slice(target.as_slice())))
            .collect::<Vec<_>>()
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
        // Do not request callee facts for a child that cannot be entered.
        if result.payload.call_stack.depth() >= config.max_call_depth {
            boundary(result, pc, FrontierReason::CallDepth);
            continue;
        }
        if context.root_address.is_some_and(|(owner, logical)| {
            logical.as_concrete().is_none() && owner == target && !exact_symbolic_self
        }) {
            boundary(result, pc, FrontierReason::MissingCode(target));
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
        let (code_address, code, program) = match resolve(world, &result.payload.store, target) {
            Ok(resolution) => resolution,
            Err(reason) => {
                boundary(result, pc, reason);
                continue;
            }
        };
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
        let (state_address, address_value, frame_caller, call_value) = match op {
            opcode::CALL => (
                target,
                if exact_symbolic_self {
                    logical_address
                } else {
                    AddressInput::Concrete(target)
                },
                caller.key.address_value,
                value.clone(),
            ),
            opcode::STATICCALL => (
                target,
                if exact_symbolic_self {
                    logical_address
                } else {
                    AddressInput::Concrete(target)
                },
                caller.key.address_value,
                zero(),
            ),
            opcode::CALLCODE => (
                caller_address,
                caller.key.address_value,
                caller.key.address_value,
                value.clone(),
            ),
            opcode::DELEGATECALL => (
                caller_address,
                caller.key.address_value,
                caller.key.caller,
                caller.call_value.clone(),
            ),
            _ => unreachable!(),
        };
        let saved_store = result.payload.store.snapshot();
        let code_hash = captured_hash(&program, &result.payload.store, code_address, code);
        let mut payload = result.payload.clone();
        if op == opcode::CALL && target != caller_address {
            let recipient = payload.store.read_balance(target);
            if !context.budget.charge(
                domain
                    .operation_work(&[balance.clone(), value.clone()])
                    .saturating_add(domain.operation_work(&[recipient.clone(), value.clone()])),
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
        payload.call_stack.push_child(ChildFrame {
            state: FrameState {
                key: FrameKey {
                    code_address,
                    code_hash,
                    mode: code,
                    address: state_address,
                    address_value,
                    caller: frame_caller,
                    is_static: caller.key.is_static || op == opcode::STATICCALL,
                    basic_block_index: 0,
                    stack_height: 0,
                    jump_history: Vec::new(),
                },
                code,
                program,
                stack: Vec::new(),
                memory: ByteArray::memory(),
                calldata,
                environment_calldata: false,
                returndata: ByteArray::empty(),
                call_value,
                saved_store,
            },
            continuation: Continuation {
                return_block: Some(next),
                output_offset: args[output].clone(),
                output_size: args[output + 1].clone(),
                creation: None,
            },
        });
        payload.normalize();
        result.successors.push(Successor {
            payload,
            kind: MachineEdgeKind::Call,
        });
    }
}
