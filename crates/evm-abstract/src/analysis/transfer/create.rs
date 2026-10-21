//! CREATE/CREATE2 lifecycle. Initcode is a fresh frame of the shared machine;
//! the destination's code remains empty until a successful RETURN installs it.

use crate::domain::{EvmNumericAlgebra, SingletonProof, WordElement};

use super::budget::{byte_work, maximum};
use super::{EvmExecution, EvmSuccessor, TransferContext, boundary, zero};
use super::{candidates, query};

use crate::{
    analysis::{
        EvmChildFrame, EvmContinuation, EvmFrameState, EvmMachinePayload, FrameCode, FrameKey,
        FrontierReason, MachineEdgeKind, OutcomeKind,
    },
    bytecode::Program,
    domain::{
        EvmValue,
        symbolic::{ByteView, ExprId, ExprLimits},
    },
    world::{AddressInput, EvmByteArray, EvmWorld},
};
use alloy_primitives::{Address, U256, keccak256};
use revm_bytecode::opcode;
use serde::Serialize;

/// Creation facts that cannot be represented exactly by the finite fixture model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum CreationBoundary {
    /// The creator logical address is symbolic; hashing a placeholder is unsound.
    UnknownCreator,
    /// The creator nonce is not finitely observed.
    UnknownNonce(Address),
    /// Destination nonce or code does not establish the collision decision.
    UnknownCollision(Address),
    /// Endowment or the balances needed for its checked transfer are unknown.
    UnknownEndowment,
    /// Extracted initcode bytes are not concrete.
    UnknownInitCode,
    /// Returned deployed runtime bytes are not concrete.
    UnknownRuntimeCode,
    /// CREATE2 salt is outside the finite value domain.
    UnknownSalt,
    /// A supplied nonce cannot be represented by the protocol's 64-bit nonce.
    NonceOverflow(Address),
    /// The hash-derived creation address is a reserved native precompile.
    ReservedAddress(Address),
}

fn incomplete<N: WordElement>(result: &mut EvmExecution<N>, pc: usize, reason: CreationBoundary) {
    boundary(result, pc, FrontierReason::Creation(reason));
}

fn write_balance_literal<N: WordElement>(
    store: &mut crate::world::EvmStore<N>,
    address: Address,
    word: U256,
) {
    store.write_balance_with_truth(
        address,
        EvmValue::constant(word),
        word.is_zero(),
        !word.is_zero(),
    );
}

fn immediate_failure<N: WordElement>(
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

fn reserved<N: WordElement>(world: &EvmWorld<N>, address: Address) -> bool {
    let number = U256::from_be_slice(address.as_slice());
    (number > U256::ZERO
        && number
            <= U256::from(if world.fork() == crate::Fork::Cancun {
                10
            } else {
                17
            }))
        || (world.fork() == crate::Fork::Osaka && number == U256::from(0x100))
}

/// Consume the caller's existing extraction reservation and proved host extent.
/// Constructing the view does not rediscover its numeric length.
/// Each newly read opaque byte still needs its own independent query payment.
fn materialize<D: EvmNumericAlgebra>(
    data: &EvmByteArray<D::Element>,
    length: usize,
    context: &mut TransferContext<'_, D>,
) -> Result<Option<Vec<u8>>, FrontierReason> {
    query::checkpoint(context.budget)?;
    data.check_existing(context.domain.spec().relations().expressions())
        .map_err(super::errors::bytes)?;
    if length > context.config.max_memory_bytes {
        return Ok(None);
    }
    let view = ByteView::new(
        data.state_root().clone(),
        ExprId::constant(U256::ZERO),
        ExprId::constant(U256::from(length)),
    );
    let limits = ExprLimits {
        max_nodes: length
            .saturating_mul(data.state_root().nodes().saturating_add(2))
            .max(view.nodes()),
        max_depth: data.state_root().depth().saturating_add(2),
    };
    let known = view.known_bytes(limits);
    query::checkpoint(context.budget)?;
    if let Some(bytes) = known.map_err(super::errors::expression)? {
        return Ok(Some(bytes));
    }
    let mut bytes = Vec::new();
    for index in 0..length {
        query::checkpoint(context.budget)?;
        let value = data
            .read_byte_checked_with(&EvmValue::constant(U256::from(index)), context.domain)
            .map_err(super::errors::bytes)?;
        query::checkpoint(context.budget)?;
        let (_, mut proof) = candidates::prepare(&value, context.budget)?;
        let Some(byte) = query::resolve(value.numeric(), &mut proof, context.budget)? else {
            return Ok(None);
        };
        let Ok(byte) = u8::try_from(byte) else {
            return Ok(None);
        };
        bytes.push(byte);
    }
    Ok(Some(bytes))
}

/// Suspend the caller and enter arbitrary concrete EVM initcode. Nonce bump
/// precedes the child's savepoint: failed initcode keeps that bump, while an
/// enclosing REVERT still restores it through the enclosing frame's savepoint.
pub(super) fn create<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    world: &EvmWorld<D::Element>,
    op: u8,
    args: &[EvmValue<D::Element>],
    program: &Program,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) {
    if let Err(reason) = create_prepared(result, world, op, args, program, context, pc) {
        // Completed alternatives remain evidence for the original input:
        // a later stop can follow a static-value fault, oversize initcode or
        // an already completed gas-failure branch. Only the remaining path
        // becomes a frontier; no partial child state is published after stop.
        boundary(result, pc, reason);
    }
}

fn create_prepared<D: EvmNumericAlgebra>(
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
    let next = result.payload.active().key.basic_block_index + 1;
    debug_assert!(next <= program.blocks().len());
    let (sizes, _) = candidates::prepare(&args[2], context.budget)?;
    let Some(sizes) = sizes else {
        incomplete(result, pc, CreationBoundary::UnknownInitCode);
        return Ok(());
    };
    if sizes.iter().any(|size| *size > U256::from(49_152)) {
        // EIP-3860 oversize initcode faults the CREATE opcode's current frame.
        super::calls::failure(result, context, pc);
    }
    let sizes: Vec<_> = sizes
        .iter()
        .filter(|size| **size <= U256::from(49_152))
        .copied()
        .collect();
    let Some(memory_size) = sizes
        .iter()
        .copied()
        .map(EvmValue::<D::Element>::constant)
        .reduce(|left, right| domain.join(&left, &right))
    else {
        return Ok(());
    };
    let (offsets, offset_proof) = if sizes.iter().all(U256::is_zero) {
        (vec![U256::ZERO], SingletonProof::NotQueried)
    } else {
        let (offsets, proof) = candidates::prepare(&args[1], context.budget)?;
        let Some(offsets) = offsets else {
            incomplete(result, pc, CreationBoundary::UnknownInitCode);
            return Ok(());
        };
        (offsets.iter().copied().collect(), proof)
    };
    let size_proof = if sizes.len() == 1 {
        // memory_size was constructed from this independent host literal.
        SingletonProof::Answer(Some(sizes[0]))
    } else {
        SingletonProof::NotQueried
    };
    if !super::ranges::memory_after_queries(
        result,
        &args[1],
        &memory_size,
        offset_proof,
        size_proof,
        context,
        pc,
    ) || !super::expand_memory(result, &args[1], &memory_size, context, pc)
    {
        return Ok(());
    }
    // revm/Yellow-Paper depth starts at zero for the transaction entry. Child
    // depth equals the current frame count and must not exceed 1024.
    if result.payload.call_stack.depth() > 1024 {
        immediate_failure(result, result.payload.clone(), next);
        return Ok(());
    }
    if result.payload.call_stack.depth() >= config.max_call_depth {
        boundary(result, pc, FrontierReason::CallDepth);
        return Ok(());
    }
    // Abstract execution can carry an address expression beyond the fixed
    // environment vocabulary. Its storage namespace is not a concrete creator.
    let frame = result.payload.active();
    let caller_address = match &frame.address_operand {
        Some(value) => {
            let (_, mut proof) = candidates::prepare(value, context.budget)?;
            query::resolve(value.numeric(), &mut proof, context.budget)?.map(super::address)
        }
        None => frame.key.address_value.as_concrete(),
    };
    let Some(caller_address) = caller_address else {
        incomplete(result, pc, CreationBoundary::UnknownCreator);
        return Ok(());
    };
    let nonce = result.payload.store.nonce(caller_address);
    let (nonces, _) = candidates::prepare(&nonce, context.budget)?;
    let Some(nonces) = nonces else {
        incomplete(result, pc, CreationBoundary::UnknownNonce(caller_address));
        return Ok(());
    };
    let (values, _) = candidates::prepare(&args[0], context.budget)?;
    let Some(values) = values else {
        incomplete(result, pc, CreationBoundary::UnknownEndowment);
        return Ok(());
    };
    let salts = if op == opcode::CREATE2 {
        let (salts, _) = candidates::prepare(&args[3], context.budget)?;
        let Some(salts) = salts else {
            incomplete(result, pc, CreationBoundary::UnknownSalt);
            return Ok(());
        };
        salts.iter().copied().collect::<Vec<_>>()
    } else {
        vec![U256::ZERO]
    };
    let balance = result.payload.store.read_balance(caller_address);
    let balances = if values.iter().all(U256::is_zero) {
        // Zero endowment preserves the complete balance, including identities.
        vec![None]
    } else {
        match candidates::prepare(&balance, context.budget)?.0 {
            Some(balances) => balances.iter().copied().map(Some).collect::<Vec<_>>(),
            None => {
                incomplete(result, pc, CreationBoundary::UnknownEndowment);
                return Ok(());
            }
        }
    };
    for size in &sizes {
        for offset in &offsets {
            if !context.budget.charge(byte_work(
                &[
                    &EvmValue::<D::Element>::constant(*offset),
                    &EvmValue::<D::Element>::constant(*size),
                ],
                usize::try_from(*size).unwrap_or(usize::MAX),
                result.payload.active().memory.work_size(),
                domain,
            )) {
                return Err(FrontierReason::Work);
            }
            let initcode = match result.payload.active().memory.slice_with(
                &EvmValue::<D::Element>::constant(*offset),
                &EvmValue::<D::Element>::constant(*size),
                config.max_memory_bytes,
                domain,
            ) {
                Ok(bytes) => {
                    match materialize(
                        &bytes,
                        usize::try_from(*size).expect("bounded initcode size"),
                        context,
                    )? {
                        Some(bytes) => bytes,
                        None => {
                            incomplete(result, pc, CreationBoundary::UnknownInitCode);
                            continue;
                        }
                    }
                }
                Err(error) => {
                    boundary(result, pc, super::errors::bytes(error));
                    continue;
                }
            };
            for nonce in nonces.iter() {
                if *nonce > U256::from(u64::MAX) {
                    incomplete(result, pc, CreationBoundary::NonceOverflow(caller_address));
                    continue;
                }
                if *nonce == U256::from(u64::MAX) {
                    let mut payload = result.payload.clone();
                    payload
                        .store
                        .write_nonce(caller_address, EvmValue::<D::Element>::constant(*nonce));
                    immediate_failure(result, payload, next);
                    continue;
                }
                for value in values.iter() {
                    for balance in &balances {
                        if balance.is_some_and(|balance| *value > balance) {
                            let mut payload = result.payload.clone();
                            payload.store.write_nonce(
                                caller_address,
                                EvmValue::<D::Element>::constant(*nonce),
                            );
                            write_balance_literal(
                                &mut payload.store,
                                caller_address,
                                balance.expect("insufficient funds require an observed balance"),
                            );
                            immediate_failure(result, payload, next);
                            continue;
                        }
                        for salt in &salts {
                            if !context.budget.charge(
                                result
                                    .payload
                                    .work_size()
                                    .saturating_add(initcode.len())
                                    .saturating_add(1),
                            ) {
                                return Err(FrontierReason::Work);
                            }
                            let destination = if op == opcode::CREATE {
                                caller_address.create(nonce.to::<u64>())
                            } else {
                                caller_address
                                    .create2_from_code(salt.to_be_bytes::<32>(), &initcode)
                            };
                            let mut payload = result.payload.clone();
                            if let Some(balance) = balance {
                                write_balance_literal(&mut payload.store, caller_address, *balance);
                            }
                            // The creator's nonce change survives a failed
                            // initcode child, but rolls back with its own frame.
                            payload.store.touch_account(caller_address);
                            payload.store.write_nonce(
                                caller_address,
                                EvmValue::<D::Element>::constant(*nonce + U256::from(1)),
                            );
                            // Gas remains abstract, but insufficient initcode gas
                            // fails after the caller nonce has been incremented.
                            immediate_failure(result, payload.clone(), next);
                            // The savepoint deliberately includes the caller nonce bump.
                            let saved_store = payload.store.snapshot();
                            if reserved(world, destination) {
                                incomplete(
                                    result,
                                    pc,
                                    CreationBoundary::ReservedAddress(destination),
                                );
                                continue;
                            }
                            let destination_nonce = payload.store.nonce(destination);
                            let code = payload.store.raw_account_code(destination);
                            if code.as_ref().is_some_and(|code| !code.is_empty()) {
                                // Either known fact alone proves a collision; the
                                // other account observation need not be present.
                                immediate_failure(result, payload, next);
                                continue;
                            }
                            let (destination_nonces, _) =
                                candidates::prepare(&destination_nonce, context.budget)?;
                            let Some(destination_nonces) = destination_nonces else {
                                incomplete(
                                    result,
                                    pc,
                                    CreationBoundary::UnknownCollision(destination),
                                );
                                continue;
                            };
                            if destination_nonces.iter().all(|nonce| !nonce.is_zero()) {
                                immediate_failure(result, payload, next);
                                continue;
                            }
                            let Some(code) = code else {
                                incomplete(
                                    result,
                                    pc,
                                    CreationBoundary::UnknownCollision(destination),
                                );
                                continue;
                            };
                            if destination_nonces.iter().any(|nonce| !nonce.is_zero())
                                || !code.is_empty()
                            {
                                immediate_failure(result, payload.clone(), next);
                            }
                            if !code.is_empty() || !destination_nonces.contains(&U256::ZERO) {
                                continue;
                            }
                            if initcode.first() == Some(&0xef) {
                                // EF is invalid EVM initcode under every supported fork;
                                // it must not be interpreted as an account delegation marker.
                                immediate_failure(result, payload, next);
                                continue;
                            }
                            let init_program = Program::decode_with_fork(&initcode, world.fork())
                                .expect("EVM initcode with a non-EF first byte always decodes");
                            let recipient = payload.store.read_balance(destination);
                            let recipients = if value.is_zero() {
                                vec![None]
                            } else {
                                match candidates::prepare(&recipient, context.budget)?.0 {
                                    Some(balances) => {
                                        balances.iter().copied().map(Some).collect::<Vec<_>>()
                                    }
                                    None => {
                                        incomplete(result, pc, CreationBoundary::UnknownEndowment);
                                        continue;
                                    }
                                }
                            };
                            for recipient in recipients {
                                let mut branch = payload.clone();
                                if let Some(recipient) = recipient {
                                    let Some(total) = recipient.checked_add(*value) else {
                                        immediate_failure(result, branch, next);
                                        continue;
                                    };
                                    write_balance_literal(&mut branch.store, destination, total);
                                }
                                if let Some(balance) = balance {
                                    write_balance_literal(
                                        &mut branch.store,
                                        caller_address,
                                        *balance - *value,
                                    );
                                }
                                branch.store.begin_creation(destination);
                                branch.call_stack.push_child(EvmChildFrame {
                                    state: EvmFrameState {
                                        key: FrameKey {
                                            mode: FrameCode::InitCode,
                                            code_address: destination,
                                            code_hash: keccak256(&initcode),
                                            address: destination,
                                            address_value: AddressInput::Concrete(destination),
                                            caller: AddressInput::Concrete(caller_address),
                                            is_static: false,
                                            basic_block_index: 0,
                                            stack_height: 0,
                                            jump_history: Vec::new(),
                                        },
                                        code: FrameCode::InitCode,
                                        program: Some(init_program.clone()),
                                        stack: Vec::new(),
                                        memory: EvmByteArray::<D::Element>::memory(),
                                        calldata: EvmByteArray::<D::Element>::empty(),
                                        environment_calldata: false,
                                        address_operand: None,
                                        caller_operand: None,
                                        returndata: EvmByteArray::<D::Element>::empty(),
                                        call_value: EvmValue::<D::Element>::constant(*value),
                                        saved_store: saved_store.clone(),
                                    },
                                    continuation: EvmContinuation {
                                        return_block: Some(next),
                                        output_offset: zero(),
                                        output_size: zero(),
                                        creation: Some(destination),
                                    },
                                });
                                query::checkpoint(context.budget)?;
                                branch.normalize();
                                query::checkpoint(context.budget)?;
                                result.successors.push(EvmSuccessor {
                                    payload: branch,
                                    kind: MachineEdgeKind::Call,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    query::checkpoint(context.budget)?;
    Ok(())
}

/// Validate and deploy initcode's RETURN before normal frame completion. Invalid
/// deployed code is child failure; unknown code leaves an explicit frontier.
pub(super) fn finish_creation<D: EvmNumericAlgebra>(
    payload: &mut EvmMachinePayload<D::Element>,
    kind: &mut OutcomeKind,
    data: &mut EvmByteArray<D::Element>,
    context: &mut TransferContext<'_, D>,
    _pc: usize,
) -> Result<(), FrontierReason> {
    let Some(address) = payload
        .call_stack
        .active_child()
        .and_then(|child| child.continuation.creation)
    else {
        return Ok(());
    };
    if *kind != OutcomeKind::Return {
        return Ok(());
    }
    let (lengths, mut proof) = candidates::prepare(data.len(), context.budget)?;
    let work = data.work_size().saturating_add(maximum(lengths.as_ref()));
    if work == usize::MAX || !context.budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    if lengths.is_some_and(|lengths| lengths.iter().all(|length| *length > U256::from(24_576))) {
        *kind = OutcomeKind::Failure;
        *data = EvmByteArray::<D::Element>::empty();
        return Ok(());
    }
    let length = query::resolve(data.len().numeric(), &mut proof, context.budget)?
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(FrontierReason::Creation(
            CreationBoundary::UnknownRuntimeCode,
        ))?;
    let bytes = materialize(data, length, context)?.ok_or(FrontierReason::Creation(
        CreationBoundary::UnknownRuntimeCode,
    ))?;
    if bytes.len() > 24_576 || bytes.first() == Some(&0xef) {
        *kind = OutcomeKind::Failure;
        *data = EvmByteArray::<D::Element>::empty();
        return Ok(());
    }
    let runtime = Program::decode_with_fork(
        &bytes,
        payload
            .active()
            .program
            .as_ref()
            .expect("initcode frames capture their program")
            .fork(),
    )
    .expect("validated EVM runtime bytecode with a non-EF first byte always decodes");
    payload.store.deploy_code(address, runtime);
    // Successful CREATE never exposes its runtime as RETURNDATA in the caller.
    *data = EvmByteArray::<D::Element>::empty();
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::reserved;
    use crate::{Fork, world::World};
    use alloy_primitives::{Address, U256};

    #[test]
    fn reservation_follows_the_selected_fork_precompile_set() {
        for (fork, number, expected) in [
            (Fork::Cancun, 0, false),
            (Fork::Cancun, 10, true),
            (Fork::Cancun, 11, false),
            (Fork::Prague, 17, true),
            (Fork::Prague, 18, false),
            (Fork::Prague, 0x100, false),
            (Fork::Osaka, 17, true),
            (Fork::Osaka, 18, false),
            (Fork::Osaka, 0x100, true),
        ] {
            let address = Address::from_word(U256::from(number).into());
            assert_eq!(
                reserved(
                    &World::new(fork, "reserved creation address boundary"),
                    address
                ),
                expected
            );
        }
    }
}
