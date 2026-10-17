//! Account-code resolution, frame entry and rollback-aware caller resumption.
use super::budget::byte_work;
use super::{
    CompletedCall, Execution, Outcome, Successor, TransferContext, address, boundary, boundary_on,
    touch_memory, zero,
};
use crate::{
    analysis::{
        CallStack, ChildFrame, Continuation, FrameCode, FrameKey, FrameState, FrontierReason,
        MachineEdgeKind, MachinePayload, OutcomeKind, RootFrame,
    },
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{AddressInput, ByteArray, Code, Entry, Snapshot, Store, World},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use revm_bytecode::opcode;

mod dispatch;
#[cfg(test)]
mod tests;

use dispatch::target_guard;

fn precompile(world: &World, address: Address) -> bool {
    super::precompile::contains(world.fork(), address)
}

/// 7702 follows exactly one pointer. The second marker is invalid executable
/// EF code; a delegated precompile target executes as empty code.
pub(super) fn resolve(
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
                _ => Ok((*implementation, FrameCode::Unknown, None)),
            }
        }
        _ => Ok((target, FrameCode::Unknown, None)),
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
    let mut store = Store::new_checked(world, domain).map_err(super::errors::store)?;
    store.project(domain);
    store
        .check_existing(domain.spec().relations().expressions())
        .map_err(super::errors::store)?;
    let (code_address, code, program) = resolve(world, &store, entry.address)?;
    let code_hash = captured_hash(&program, &store, code_address, code);
    let mut calldata = entry.environment.calldata.project(domain);
    if let Some(scope) = entry.environment.input_scope.id() {
        calldata.bind_input(scope).map_err(super::errors::bytes)?;
    }
    calldata
        .check_existing(domain.spec().relations().expressions())
        .map_err(super::errors::bytes)?;
    // Capture the transaction's pre-entry state first. An entered root is a
    // message-call recipient, while the separate pre-entry gas failure and
    // ordinary root rollback must retain this untouched checkpoint.
    let saved_store = store.snapshot();
    if let Some(recipient) = entry.environment.to.as_concrete() {
        store.touch_account(recipient);
    } else {
        // Bytecode-only entry uses a synthetic code index, not an observed
        // recipient. Any account may be touched; no particular one must be.
        store.may_touch_all();
    }
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
                calldata,
                environment_calldata: true,
                returndata: ByteArray::empty(),
                call_value: domain.project(&entry.environment.value.clone().with_symbol(
                    crate::world::Symbol::CallValue,
                    entry.environment.input_scope.id(),
                )),
                address_operand: None,
                caller_operand: None,
                saved_store,
            },
        }),
        store,
    })
}

/// Complete a root's Store without retaining frame data that outcomes discard.
/// Creation completion stays with `finish`: callers may bypass it only for an
/// actual native root with no child, where `finish_creation` is a no-op.
pub(super) fn finish_root_store(
    store: &mut Store,
    saved_store: &Snapshot,
    kind: OutcomeKind,
    context: &mut TransferContext<'_>,
) -> Result<(), FrontierReason> {
    if !context.procedure {
        let rollback = (kind != OutcomeKind::Return).then_some(saved_store);
        store
            .finalize_transaction_checked(context.domain, rollback, context.budget)
            .map_err(|error| {
                if error.is_cancelled() {
                    FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
                } else if let Some(error) = error.into_store_error() {
                    super::errors::store(error)
                } else {
                    FrontierReason::Work
                }
            })?;
    } else if kind != OutcomeKind::Return {
        store.restore(saved_store.clone());
    }
    Ok(())
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
        boundary_on(result, pc, reason, payload.key());
        return;
    }
    if let Some(child) = payload.call_stack.active_child() {
        let continuation = &child.continuation;
        let parent = payload.call_stack.parent().expect("a child has a parent");
        let cost = parent.memory.copy_return_data_work(
            &data,
            &continuation.output_offset,
            &continuation.output_size,
            domain,
        );
        if !context.budget.charge(cost) {
            boundary_on(result, pc, FrontierReason::Work, payload.key());
            return;
        }
    }
    let Some(child) = payload.call_stack.pop_child() else {
        if let Err(reason) = finish_root_store(
            &mut payload.store,
            &payload.call_stack.root().state.saved_store,
            kind,
            context,
        ) {
            boundary_on(result, pc, reason, payload.key());
            return;
        }
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
    if let Err(error) = parent.memory.copy_return_data(
        &continuation.output_offset,
        &parent.returndata,
        &continuation.output_size,
        config.max_memory_bytes,
        domain,
    ) {
        boundary_on(result, pc, super::errors::bytes(error), payload.key());
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
    // A static CALL with nonzero value faults before any memory access. Retain
    // the value condition on each possible outcome when it is symbolic.
    if result.payload.active().key.is_static && op == opcode::CALL {
        if value.may_be_nonzero()
            && let Some(payload) = super::relations::branch(result, &value, true, context, pc)
        {
            finish(
                result,
                payload,
                OutcomeKind::Failure,
                ByteArray::empty(),
                context,
                pc,
            );
        }
        if !value.may_be_zero() {
            return;
        }
        let Some(payload) = super::relations::branch(result, &value, false, context, pc) else {
            return;
        };
        result.payload = payload;
    }
    if !touch_memory(result, &args[input], &args[input + 1], context, pc)
        || !touch_memory(result, &args[output], &args[output + 1], context, pc)
    {
        return;
    }
    let next = result.payload.active().key.basic_block_index + 1;
    // The decoder splits after every CALL. An end-of-code continuation is a
    // synthetic empty block, making each return pop exactly one frame.
    debug_assert!(next <= program.blocks().len());
    // Gas, balance or the physical EVM depth limit can reject a call before
    // entry. Relative procedures abstract absolute depth, so retaining this
    // branch also covers maximum-depth rejection without unrolling 1024 frames.
    // It preserves all caller writes and clears returndata before real bytecode
    // continues; procedure saturation never invents a root terminal from it.
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
        0,
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
    let target_value =
        super::relations::identify(args[1].clone(), config.analysis.relations.enabled);
    let projected = domain.address_projection(&target_value);
    let finite = projected.constants().is_some();
    let mut targets = if let Some(values) = projected.constants() {
        values.iter().map(|v| address(*v)).collect::<Vec<_>>()
    } else {
        if !context.budget.charge(
            world
                .accounts()
                .len()
                .saturating_add(result.payload.store.addresses().len())
                .saturating_add(18),
        ) {
            boundary(result, pc, FrontierReason::Work);
            return;
        }
        let mut addresses = result.payload.store.addresses();
        addresses.extend(world.accounts().keys().copied());
        // All native addresses in the pinned post-Merge fork tables. Native
        // resolution takes precedence over observed account code.
        addresses.extend(
            (1u64..=17)
                .chain([0x100])
                .map(|number| Address::from_word(U256::from(number).into()))
                .filter(|target| precompile(world, *target)),
        );
        addresses
            .into_iter()
            .filter(|target| projected.contains(U256::from_be_slice(target.as_slice())))
            .collect()
    };
    targets.sort();
    targets.dedup();
    let mut remaining = Some(result.payload.clone());
    let mut branches = Vec::new();
    // A bytecode-only root may have a symbolic logical address. Its physical
    // owner namespace is not an observed twenty-byte destination.
    if let Some((owner, logical)) = context
        .root_address
        .filter(|(_, logical)| logical.as_concrete().is_none())
    {
        let candidate = logical.scoped_value(context.input_scope);
        if let Some(input) = remaining.take() {
            if let Some(payload) =
                target_guard(result, &input, &projected, &candidate, true, context, pc)
            {
                branches.push((owner, logical, None, payload));
            }
            remaining = target_guard(result, &input, &projected, &candidate, false, context, pc);
        }
        if !finite {
            targets.retain(|target| *target != owner);
        }
    }
    for target in targets {
        let Some(input) = remaining.take() else { break };
        let candidate = AbstractValue::constant(U256::from_be_slice(target.as_slice()));
        if let Some(payload) =
            target_guard(result, &input, &projected, &candidate, true, context, pc)
        {
            branches.push((target, AddressInput::Concrete(target), None, payload));
        }
        remaining = target_guard(result, &input, &projected, &candidate, false, context, pc);
    }
    if !finite && let Some(payload) = remaining {
        let namespace = super::procedure::owner_namespace(world, &payload.store);
        branches.push((
            namespace,
            AddressInput::Concrete(namespace),
            Some(projected.clone()),
            payload,
        ));
    }
    for (target, logical_target, target_operand, input_payload) in branches {
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
        if !context.procedure && result.payload.call_stack.depth() >= config.max_call_depth {
            boundary(result, pc, FrontierReason::CallDepth);
            continue;
        }
        let caller = input_payload.active();
        let caller_address = caller.key.address;
        let balance = input_payload.store.read_balance(caller_address);
        if with_value
            && let (Some(balances), Some(values)) = (balance.constants(), value.constants())
            && balances
                .iter()
                .all(|balance| values.iter().all(|value| value > balance))
        {
            continue;
        }
        let (code_address, code, program) = match if target_operand.is_some()
            || context.root_address.is_some_and(|(owner, logical)| {
                owner == target
                    && logical.as_concrete().is_none()
                    && logical_target.as_concrete().is_some()
            }) {
            Ok((target, FrameCode::Unknown, None))
        } else {
            resolve(world, &input_payload.store, target)
        } {
            Ok(resolution) => resolution,
            Err(reason) => {
                boundary(result, pc, reason);
                continue;
            }
        };
        if code == FrameCode::Unknown && context.discover_code && target_operand.is_none() {
            boundary_on(
                result,
                pc,
                FrontierReason::MissingCode(code_address),
                input_payload.key(),
            );
            continue;
        }
        let calldata = match caller.memory.slice(
            &args[input],
            &args[input + 1],
            config.max_memory_bytes,
            domain,
        ) {
            Ok(data) => data,
            Err(error) => {
                boundary(result, pc, super::errors::bytes(error));
                continue;
            }
        };
        let (state_address, address_value, frame_caller, call_value) = match op {
            opcode::CALL => (
                target,
                logical_target,
                caller.key.address_value,
                value.clone(),
            ),
            opcode::STATICCALL => (target, logical_target, caller.key.address_value, zero()),
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
        let unknown_target = target_operand.is_some();
        let address_operand = if matches!(op, opcode::CALL | opcode::STATICCALL) {
            target_operand
        } else {
            caller.address_operand.clone()
        };
        let caller_operand = if op == opcode::DELEGATECALL {
            caller.caller_operand.clone()
        } else {
            caller.address_operand.clone()
        };
        let saved_store = input_payload.store.snapshot();
        let code_hash = captured_hash(&program, &input_payload.store, code_address, code);
        let mut payload = input_payload.clone();
        // revm's CALL checkpoint precedes transfer_loaded: a zero transfer
        // touches its recipient, and failure restores that touch with the child
        // store. CALLCODE's self-transfer and DELEGATECALL do not add a touch.
        if matches!(op, opcode::CALL | opcode::STATICCALL) && target != caller_address {
            if unknown_target {
                payload.store.may_touch_all();
            } else {
                payload.store.touch_account(target);
            }
            if op == opcode::CALL && value.may_be_nonzero() {
                if value.may_be_zero() {
                    payload.store.may_touch_account(caller_address);
                } else {
                    payload.store.touch_account(caller_address);
                }
            }
        }
        // A proven zero transfer leaves both balances and their input identities
        // unchanged. Rebuilding them with ADD/SUB would add arithmetic metadata
        // and make otherwise identical rollback preconditions look different.
        if op == opcode::CALL && target != caller_address && value.singleton() != Some(U256::ZERO) {
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
                address_operand,
                caller_operand,
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
