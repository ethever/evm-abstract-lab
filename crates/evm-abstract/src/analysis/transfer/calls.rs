//! Account-code resolution, frame entry and rollback-aware caller resumption.

use super::{candidates, dispatch::Truth, query};

use super::budget::byte_work;
use super::{
    EvmCompletedCall, EvmExecution, EvmOutcome, EvmSuccessor, TransferContext, address, boundary,
    boundary_on, touch_memory, zero,
};
use crate::domain::{EvmNumericAlgebra, SingletonProof, WordElement};

use crate::{
    analysis::{
        EvmCallStack, EvmChildFrame, EvmContinuation, EvmFrameState, EvmMachinePayload,
        EvmRootFrame, FrameCode, FrameKey, FrontierReason, MachineEdgeKind, OutcomeKind,
    },
    bytecode::Program,
    domain::{AlgebraRef, EvmValue},
    world::{AddressInput, Code, EvmByteArray, EvmEntry, EvmSnapshot, EvmStore, EvmWorld},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use revm_bytecode::opcode;

mod dispatch;
#[cfg(test)]
mod tests;

use dispatch::target_guard;

fn precompile<N: WordElement>(world: &EvmWorld<N>, address: Address) -> bool {
    super::precompile::contains(world.fork(), address)
}

/// 7702 follows exactly one pointer. The second marker is invalid executable
/// EF code; a delegated precompile target executes as empty code.
pub(super) fn resolve<N: WordElement>(
    world: &EvmWorld<N>,
    store: &EvmStore<N>,
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

fn captured_hash<N: WordElement>(
    program: &Option<Program>,
    store: &EvmStore<N>,
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

pub(super) fn initial<D: EvmNumericAlgebra>(
    world: &EvmWorld<D::Element>,
    entry: &EvmEntry<D::Element>,
    domain: AlgebraRef<'_, D>,
) -> Result<EvmMachinePayload<D::Element>, FrontierReason> {
    let mut store =
        EvmStore::<D::Element>::new_checked_with(world, domain).map_err(super::errors::store)?;
    store.project_with(domain);
    store
        .check_existing(domain.spec().relations().expressions())
        .map_err(super::errors::store)?;
    let (code_address, code, program) = resolve(world, &store, entry.address)?;
    let code_hash = captured_hash(&program, &store, code_address, code);
    let mut calldata = entry.environment.calldata.project_with(domain);
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
    Ok(EvmMachinePayload {
        relations: crate::domain::relational::RelationState::default(),
        call_stack: EvmCallStack::<D::Element>::new(EvmRootFrame {
            state: EvmFrameState {
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
                memory: EvmByteArray::<D::Element>::memory(),
                calldata,
                environment_calldata: true,
                returndata: EvmByteArray::<D::Element>::empty(),
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

/// Complete a root's EvmStore without retaining frame data that outcomes discard.
/// Creation completion stays with `finish`: callers may bypass it only for an
/// actual native root with no child, where `finish_creation` is a no-op.
pub(super) fn finish_root_store<D: EvmNumericAlgebra>(
    store: &mut EvmStore<D::Element>,
    saved_store: &EvmSnapshot<D::Element>,
    kind: OutcomeKind,
    context: &mut TransferContext<'_, D>,
) -> Result<(), FrontierReason> {
    if !context.procedure {
        let rollback = (kind != OutcomeKind::Return).then_some(saved_store);
        store
            .finalize_transaction_checked_with(context.domain, rollback, context.budget)
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

pub(super) fn finish<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    mut payload: EvmMachinePayload<D::Element>,
    mut kind: OutcomeKind,
    mut data: EvmByteArray<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) {
    let config = context.config;
    let domain = context.domain;
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary_on(result, pc, reason, payload.key());
        return;
    }
    if payload.call_stack.active_child().is_some() {
        let paid = payload
            .pay_clone_with(context.budget)
            .and_then(|()| data.pay_clone_with(context.budget));
        if let Err(error) = paid {
            boundary_on(result, pc, clone_reason(error), payload.key());
            return;
        }
    }
    let completed = payload
        .call_stack
        .active_child()
        .map(|_| (payload.clone(), kind, data.clone()));
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary_on(result, pc, reason, payload.key());
        return;
    }
    if let Err(reason) =
        super::create::finish_creation(&mut payload, &mut kind, &mut data, context, pc)
    {
        boundary_on(result, pc, reason, payload.key());
        return;
    }
    if let Some(child) = payload.call_stack.active_child() {
        let continuation = &child.continuation;
        let parent = payload.call_stack.parent().expect("a child has a parent");
        // Creation has finished: price the actual rollback/output objects,
        // while the original pre-creation completed-call capture stays intact.
        let snapshots = (|| {
            let store = if kind == OutcomeKind::Return {
                &payload.store
            } else {
                let saved = child.state.saved_store.state();
                saved.pay_clone_with(context.budget)?;
                saved
            };
            store.pay_clone_with(context.budget)?;
            data.pay_clone_with(context.budget)
        })();
        if let Err(error) = snapshots {
            boundary_on(result, pc, clone_reason(error), payload.key());
            return;
        }
        let paid = parent.memory.pay_return_copy_with(
            &data,
            &continuation.output_offset,
            &continuation.output_size,
            domain,
            context.budget,
        );
        match paid {
            Ok(true) => {}
            Ok(false) => {
                boundary_on(result, pc, FrontierReason::Work, payload.key());
                return;
            }
            Err(error) => {
                boundary_on(result, pc, super::errors::bytes(error), payload.key());
                return;
            }
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
        result.outcomes.push(EvmOutcome {
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
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary_on(result, pc, reason, payload.key());
        return;
    }
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
    parent
        .stack
        .push(EvmValue::<D::Element>::constant(result_value));
    let copied = parent.memory.copy_return_data_budgeted_with(
        &continuation.output_offset,
        &parent.returndata,
        &continuation.output_size,
        config.max_memory_bytes,
        domain,
        context.budget,
    );
    match copied {
        Ok(true) => {}
        Ok(false) => {
            boundary_on(result, pc, FrontierReason::Work, payload.key());
            return;
        }
        Err(error) => {
            boundary_on(result, pc, super::errors::bytes(error), payload.key());
            return;
        }
    }
    if let Err(reason) = query::checkpoint(context.budget) {
        boundary_on(result, pc, reason, payload.key());
        return;
    }
    parent.key.basic_block_index = continuation
        .return_block
        .expect("call continuations include synthetic end blocks");
    payload.normalize();
    if let Some((payload, kind, data)) = completed {
        result.completed_calls.push(EvmCompletedCall {
            payload,
            kind,
            data,
            output_store,
            output_kind,
            output_data,
        });
    }
    result.successors.push(EvmSuccessor {
        payload,
        kind: match kind {
            OutcomeKind::Return => MachineEdgeKind::Return,
            OutcomeKind::Revert => MachineEdgeKind::Revert,
            OutcomeKind::Failure => MachineEdgeKind::Failure,
        },
    });
}

pub(super) fn failure<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) {
    if let Err(error) = result.payload.pay_clone_with(context.budget) {
        boundary_on(result, pc, clone_reason(error), result.payload.key());
        return;
    }
    finish(
        result,
        result.payload.clone(),
        OutcomeKind::Failure,
        EvmByteArray::<D::Element>::empty(),
        context,
        pc,
    );
}

fn clone_reason(error: crate::resource::CloneError) -> FrontierReason {
    match error {
        crate::resource::CloneError::Work => FrontierReason::Work,
        crate::resource::CloneError::Cancelled => {
            FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
        }
    }
}

fn immediate_call_failure<N: WordElement>(
    result: &mut EvmExecution<N>,
    mut payload: EvmMachinePayload<N>,
    next: usize,
) {
    let caller = payload.active_mut();
    caller.stack.push(zero());
    caller.returndata = EvmByteArray::<N>::empty();
    caller.key.basic_block_index = next;
    payload.normalize();
    result.successors.push(EvmSuccessor {
        payload,
        kind: MachineEdgeKind::Failure,
    });
}

pub(super) fn call<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    world: &EvmWorld<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    program: &Program,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) {
    if let Err(reason) = call_prepared(result, world, op, args, program, context, pc) {
        // Completed alternatives remain evidence for the original input:
        // a later stop can follow a static-value fault, oversize initcode or
        // an already completed gas-failure branch. Only the remaining path
        // becomes a frontier; no partial child state is published after stop.
        boundary(result, pc, reason);
    }
}

fn call_prepared<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    world: &EvmWorld<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    program: &Program,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> Result<(), FrontierReason> {
    let config = context.config;
    let domain = context.domain;
    let with_value = matches!(op, opcode::CALL | opcode::CALLCODE);
    let (value, input, output) = if with_value {
        (args[2].clone(), 3, 5)
    } else {
        (zero(), 2, 4)
    };
    let mut value_truth = if with_value {
        Truth::prepare(&value, SingletonProof::NotQueried, context.budget)?
    } else {
        Truth {
            zero: true,
            nonzero: false,
            singleton: SingletonProof::Answer(Some(U256::ZERO)),
        }
    };
    // A static CALL with nonzero value faults before any memory access. Retain
    // the value condition on each possible outcome when it is symbolic.
    if result.payload.active().key.is_static && op == opcode::CALL {
        if value_truth.nonzero
            && let Some(payload) = super::relations::branch_with_singleton(
                result,
                &value,
                true,
                value_truth.singleton,
                context,
                pc,
            )
        {
            finish(
                result,
                payload,
                OutcomeKind::Failure,
                EvmByteArray::<D::Element>::empty(),
                context,
                pc,
            );
        }
        query::checkpoint(context.budget)?;
        if !value_truth.zero {
            return Ok(());
        }
        let Some(payload) = super::relations::branch_with_singleton(
            result,
            &value,
            false,
            value_truth.singleton,
            context,
            pc,
        ) else {
            return Ok(());
        };
        result.payload = payload;
    }
    let value = if result.payload.active().key.is_static && op == opcode::CALL {
        value_truth = Truth {
            zero: true,
            nonzero: false,
            singleton: SingletonProof::Answer(Some(U256::ZERO)),
        };
        zero()
    } else {
        value
    };
    let (values, _) =
        candidates::prepare_with_proof(&value, value_truth.singleton, context.budget)?;
    let balance = result
        .payload
        .store
        .read_balance(result.payload.active().key.address);
    let balances = if with_value && value_truth.nonzero && values.is_some() {
        candidates::prepare(&balance, context.budget)?.0
    } else {
        None
    };
    if !context.budget.charge(byte_work(
        &[&args[input], &args[input + 1]],
        0,
        result.payload.active().memory.work_size(),
        domain,
    )) {
        return Err(FrontierReason::Work);
    }
    let mask = U256::MAX >> 96usize;
    query::checkpoint(context.budget)?;
    let literal_work = domain.literal_quote_work(mask);
    query::checkpoint(context.budget)?;
    let preparation_work = args[1].clone_work().saturating_add(literal_work);
    if preparation_work == usize::MAX || !context.budget.charge(preparation_work) {
        return Err(FrontierReason::Work);
    }
    query::checkpoint(context.budget)?;
    // Identity attachment is optional metadata, and does not need an opaque
    // singleton query. The paid projection owns its actual result discovery.
    let mut target_value = args[1].clone();
    if config.analysis.relations.enabled
        && target_value.expression().is_none()
        && target_value
            .constants()
            .is_none_or(|values| values.len() != 1)
        && let Some(expression) = crate::domain::symbolic::ExprId::fresh()
    {
        target_value = target_value.with_expression(expression);
    }
    let projection_args = [target_value, EvmValue::<D::Element>::constant(mask)];
    query::checkpoint(context.budget)?;
    let projection_work = domain.operation_work_for(opcode::AND, &projection_args);
    query::checkpoint(context.budget)?;
    if projection_work == usize::MAX || !context.budget.charge(projection_work) {
        return Err(FrontierReason::Work);
    }
    query::checkpoint(context.budget)?;
    let (projected, proof) = domain.address_projection_with_singleton(&projection_args);
    query::checkpoint(context.budget)?;
    let (cover, _) = candidates::prepare_with_proof(&projected, proof, context.budget)?;
    let finite = cover.is_some();
    let mut targets = if let Some(values) = cover {
        values.iter().map(|v| address(*v)).collect::<Vec<_>>()
    } else {
        if !context.budget.charge(
            world
                .accounts()
                .len()
                .saturating_add(result.payload.store.addresses().len())
                .saturating_add(18),
        ) {
            return Err(FrontierReason::Work);
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
        let work = addresses
            .len()
            .saturating_mul(projected.numeric().work_size());
        query::checkpoint(context.budget)?;
        if work == usize::MAX || !context.budget.charge(work) {
            return Err(FrontierReason::Work);
        }
        let targets = addresses
            .into_iter()
            .filter(|target| projected.contains(U256::from_be_slice(target.as_slice())))
            .collect();
        query::checkpoint(context.budget)?;
        targets
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
        // A symbolic address denotes the complete address input. Retain its
        // scoped term directly; constructing its metadata needs no discovery.
        let mut candidate = logical.value_with();
        if let (AddressInput::Symbolic(symbol), Some(scope)) = (logical, context.input_scope) {
            candidate =
                candidate.with_expression(crate::domain::symbolic::ExprId::input(scope, symbol));
        }
        if let Some(input) = remaining.take() {
            if let Some(payload) =
                target_guard(result, &input, &projected, &candidate, true, context, pc)
            {
                branches.push((owner, logical, None, payload));
            }
            query::checkpoint(context.budget)?;
            remaining = target_guard(result, &input, &projected, &candidate, false, context, pc);
            query::checkpoint(context.budget)?;
        }
        if !finite {
            targets.retain(|target| *target != owner);
        }
    }
    for target in targets {
        let Some(input) = remaining.take() else { break };
        let candidate = EvmValue::<D::Element>::constant(U256::from_be_slice(target.as_slice()));
        if let Some(payload) =
            target_guard(result, &input, &projected, &candidate, true, context, pc)
        {
            branches.push((target, AddressInput::Concrete(target), None, payload));
        }
        query::checkpoint(context.budget)?;
        remaining = target_guard(result, &input, &projected, &candidate, false, context, pc);
        query::checkpoint(context.budget)?;
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
    if !touch_memory(result, &args[input], &args[input + 1], context, pc) {
        return Ok(());
    }
    let Some(output_queries) =
        super::ranges::memory_with_queries(result, &args[output], &args[output + 1], context, pc)
    else {
        return Ok(());
    };
    // Prepare the continuation's owned operands once. The first entered child
    // takes these same copies; additional target branches own their own copies.
    let prepared_output =
        output_queries.prepare(&args[output], &args[output + 1], context.budget)?;
    super::create::expand_prepared(result, &prepared_output.0, &prepared_output.1, context)?;
    let mut prepared_output = Some(prepared_output);
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
        return Ok(());
    }
    query::checkpoint(context.budget)?;
    for (target, logical_target, target_operand, mut input_payload) in branches {
        if !context.budget.charge(
            result
                .payload
                .work_size()
                .saturating_add(result.payload.store.work_size())
                .saturating_add(1),
        ) {
            return Err(FrontierReason::Work);
        }
        // Do not request callee facts for a child that cannot be entered.
        if !context.procedure && result.payload.call_stack.depth() >= config.max_call_depth {
            boundary(result, pc, FrontierReason::CallDepth);
            continue;
        }
        // Target partitioning precedes memory admission so a refused discovery
        // cannot publish memory effects. Carry the admitted extent into each
        // already guarded caller before slicing input or entering its child.
        input_payload.active_mut().memory = result.payload.active().memory.clone();
        let caller = input_payload.active();
        let caller_address = caller.key.address;
        if with_value
            && let (Some(balances), Some(values)) = (balances.as_ref(), values.as_ref())
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
        let calldata = match caller.memory.slice_with(
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
            if op == opcode::CALL && value_truth.nonzero {
                if value_truth.zero {
                    payload.store.may_touch_account(caller_address);
                } else {
                    payload.store.touch_account(caller_address);
                }
            }
        }
        // A proven zero transfer leaves both balances and their input identities
        // unchanged. Rebuilding them with ADD/SUB would add arithmetic metadata
        // and make otherwise identical rollback preconditions look different.
        if op == opcode::CALL && target != caller_address && value_truth.nonzero {
            let recipient = payload.store.read_balance(target);
            if !context.budget.charge(
                domain
                    .operation_work(&[balance.clone(), value.clone()])
                    .saturating_add(domain.operation_work(&[recipient.clone(), value.clone()])),
            ) {
                return Err(FrontierReason::Work);
            }
            let (debit, proof) = domain
                .apply_detailed_with_singleton(opcode::SUB, &[balance.clone(), value.clone()]);
            let debit_truth = Truth::prepare(&debit.value, proof, context.budget)?;
            let (credit, proof) =
                domain.apply_detailed_with_singleton(opcode::ADD, &[recipient, value.clone()]);
            let credit_truth = Truth::prepare(&credit.value, proof, context.budget)?;
            payload.store.write_balance_with_truth(
                caller_address,
                debit.value,
                debit_truth.zero,
                debit_truth.nonzero,
            );
            payload.store.write_balance_with_truth(
                target,
                credit.value,
                credit_truth.zero,
                credit_truth.nonzero,
            );
        }
        let (output_offset, output_size) = match prepared_output.take() {
            Some(values) => values,
            None => output_queries.prepare(&args[output], &args[output + 1], context.budget)?,
        };
        query::checkpoint(context.budget)?;
        payload.call_stack.push_child(EvmChildFrame {
            state: EvmFrameState {
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
                memory: EvmByteArray::<D::Element>::memory(),
                calldata,
                environment_calldata: false,
                returndata: EvmByteArray::<D::Element>::empty(),
                call_value,
                address_operand,
                caller_operand,
                saved_store,
            },
            continuation: EvmContinuation {
                return_block: Some(next),
                output_offset,
                output_size,
                creation: None,
            },
        });
        query::checkpoint(context.budget)?;
        payload.normalize();
        query::checkpoint(context.budget)?;
        result.successors.push(EvmSuccessor {
            payload,
            kind: MachineEdgeKind::Call,
        });
    }
    query::checkpoint(context.budget)?;
    Ok(())
}
