//! CREATE/CREATE2 lifecycle. Initcode is a fresh frame of the shared machine;
//! the destination's code remains empty until a successful RETURN installs it.

use super::budget::{byte_work, maximum};
use super::{Execution, Successor, TransferContext, boundary, touch_memory, zero};
use crate::{
    analysis::{
        ChildFrame, Continuation, FrameCode, FrameKey, FrameState, FrontierReason, MachineEdgeKind,
        MachinePayload, OutcomeKind,
    },
    bytecode::Program,
    domain::Value,
    world::{AddressInput, ByteArray, World},
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

fn incomplete(result: &mut Execution, pc: usize, reason: CreationBoundary) {
    boundary(result, pc, FrontierReason::Creation(reason));
}

fn immediate_failure(result: &mut Execution, mut payload: MachinePayload, next: usize) {
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

fn reserved(world: &World, address: Address) -> bool {
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

/// Suspend the caller and enter arbitrary concrete EVM initcode. Nonce bump
/// precedes the child's savepoint: failed initcode keeps that bump, while an
/// enclosing REVERT still restores it through the enclosing frame's savepoint.
pub(super) fn create(
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
    let next = result.payload.active().key.basic_block_index + 1;
    debug_assert!(next <= program.blocks().len());
    let Some(sizes) = args[2].constants() else {
        incomplete(result, pc, CreationBoundary::UnknownInitCode);
        return;
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
        .map(Value::constant)
        .reduce(|left, right| domain.join(&left, &right))
    else {
        return;
    };
    let offsets = if sizes.iter().all(U256::is_zero) {
        vec![U256::ZERO]
    } else if let Some(offsets) = args[1].constants() {
        offsets.iter().copied().collect()
    } else {
        incomplete(result, pc, CreationBoundary::UnknownInitCode);
        return;
    };
    if !touch_memory(result, &args[1], &memory_size, context, pc) {
        return;
    }
    // revm/Yellow-Paper depth starts at zero for the transaction entry. Child
    // depth equals the current frame count and must not exceed 1024.
    if result.payload.call_stack.depth() > 1024 {
        immediate_failure(result, result.payload.clone(), next);
        return;
    }
    if result.payload.call_stack.depth() >= config.max_call_depth {
        boundary(result, pc, FrontierReason::CallDepth);
        return;
    }
    let Some(caller_address) = result.payload.active().key.address_value.as_concrete() else {
        incomplete(result, pc, CreationBoundary::UnknownCreator);
        return;
    };
    let nonce = result.payload.store.nonce(caller_address);
    let Some(nonces) = nonce.constants() else {
        incomplete(result, pc, CreationBoundary::UnknownNonce(caller_address));
        return;
    };
    let Some(values) = args[0].constants() else {
        incomplete(result, pc, CreationBoundary::UnknownEndowment);
        return;
    };
    let salts = if op == opcode::CREATE2 {
        let Some(salts) = args[3].constants() else {
            incomplete(result, pc, CreationBoundary::UnknownSalt);
            return;
        };
        salts.iter().copied().collect::<Vec<_>>()
    } else {
        vec![U256::ZERO]
    };
    let balance = result.payload.store.read_balance(caller_address);
    let balances = match balance.constants() {
        Some(balances) => balances.iter().copied().map(Some).collect::<Vec<_>>(),
        None if values.iter().all(U256::is_zero) => vec![None],
        None => {
            incomplete(result, pc, CreationBoundary::UnknownEndowment);
            return;
        }
    };
    for size in &sizes {
        for offset in &offsets {
            if !context.budget.charge(byte_work(
                &[&Value::constant(*offset), &Value::constant(*size)],
                maximum(&Value::constant(*size)),
                result.payload.active().memory.work_size(),
                domain,
            )) {
                boundary(result, pc, FrontierReason::Work);
                return;
            }
            let initcode = match result.payload.active().memory.slice(
                &Value::constant(*offset),
                &Value::constant(*size),
                config.max_memory_bytes,
                domain,
            ) {
                Ok(bytes) => match bytes.exact_bytes_bounded(config.max_memory_bytes) {
                    Some(bytes) => bytes,
                    None => {
                        incomplete(result, pc, CreationBoundary::UnknownInitCode);
                        continue;
                    }
                },
                Err(_) => {
                    boundary(result, pc, FrontierReason::Memory);
                    continue;
                }
            };
            for nonce in nonces {
                if *nonce > U256::from(u64::MAX) {
                    incomplete(result, pc, CreationBoundary::NonceOverflow(caller_address));
                    continue;
                }
                if *nonce == U256::from(u64::MAX) {
                    let mut payload = result.payload.clone();
                    payload
                        .store
                        .write_nonce(caller_address, Value::constant(*nonce));
                    immediate_failure(result, payload, next);
                    continue;
                }
                for value in values {
                    for balance in &balances {
                        if balance.is_some_and(|balance| *value > balance) {
                            let mut payload = result.payload.clone();
                            payload
                                .store
                                .write_nonce(caller_address, Value::constant(*nonce));
                            payload.store.write_balance(
                                caller_address,
                                Value::constant(
                                    balance
                                        .expect("insufficient funds require an observed balance"),
                                ),
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
                                boundary(result, pc, FrontierReason::Work);
                                return;
                            }
                            let destination = if op == opcode::CREATE {
                                caller_address.create(nonce.to::<u64>())
                            } else {
                                caller_address
                                    .create2_from_code(salt.to_be_bytes::<32>(), &initcode)
                            };
                            let mut payload = result.payload.clone();
                            if let Some(balance) = balance {
                                payload
                                    .store
                                    .write_balance(caller_address, Value::constant(*balance));
                            }
                            payload.store.write_nonce(
                                caller_address,
                                Value::constant(*nonce + U256::from(1)),
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
                            if code.as_ref().is_some_and(|code| !code.is_empty())
                                || destination_nonce.constants().is_some_and(|nonces| {
                                    nonces.iter().all(|nonce| !nonce.is_zero())
                                })
                            {
                                // Either known fact alone proves a collision; the
                                // other account observation need not be present.
                                immediate_failure(result, payload, next);
                                continue;
                            }
                            let Some(destination_nonces) = destination_nonce.constants() else {
                                incomplete(
                                    result,
                                    pc,
                                    CreationBoundary::UnknownCollision(destination),
                                );
                                continue;
                            };
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
                            let recipients = match recipient.constants() {
                                Some(balances) => {
                                    balances.iter().copied().map(Some).collect::<Vec<_>>()
                                }
                                None if value.is_zero() => vec![None],
                                None => {
                                    incomplete(result, pc, CreationBoundary::UnknownEndowment);
                                    continue;
                                }
                            };
                            for recipient in recipients {
                                let mut branch = payload.clone();
                                if let Some(recipient) = recipient {
                                    let Some(total) = recipient.checked_add(*value) else {
                                        immediate_failure(result, branch, next);
                                        continue;
                                    };
                                    branch
                                        .store
                                        .write_balance(destination, Value::constant(total));
                                }
                                if let Some(balance) = balance {
                                    branch.store.write_balance(
                                        caller_address,
                                        Value::constant(*balance - *value),
                                    );
                                }
                                branch.store.begin_creation(destination);
                                branch.call_stack.push_child(ChildFrame {
                                    state: FrameState {
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
                                        memory: ByteArray::memory(),
                                        calldata: ByteArray::empty(),
                                        environment_calldata: false,
                                        returndata: ByteArray::empty(),
                                        call_value: Value::constant(*value),
                                        saved_store: saved_store.clone(),
                                    },
                                    continuation: Continuation {
                                        return_block: Some(next),
                                        output_offset: zero(),
                                        output_size: zero(),
                                        creation: Some(destination),
                                    },
                                });
                                branch.normalize();
                                result.successors.push(Successor {
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
}

/// Validate and deploy initcode's RETURN before normal frame completion. Invalid
/// deployed code is child failure; unknown code leaves an explicit frontier.
pub(super) fn finish_creation(
    payload: &mut MachinePayload,
    kind: &mut OutcomeKind,
    data: &mut ByteArray,
    context: &mut TransferContext<'_>,
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
    if !context
        .budget
        .charge(data.work_size().saturating_add(maximum(data.len())))
    {
        return Err(FrontierReason::Work);
    }
    if data
        .len()
        .constants()
        .is_some_and(|lengths| lengths.iter().all(|length| *length > U256::from(24_576)))
        || data
            .byte_at(0, context.domain)
            .constants()
            .is_some_and(|bytes| bytes.len() == 1 && bytes.contains(&U256::from(0xef)))
    {
        *kind = OutcomeKind::Failure;
        *data = ByteArray::empty();
        return Ok(());
    }
    let bytes = data
        .exact_bytes_bounded(context.config.max_memory_bytes)
        .ok_or(FrontierReason::Creation(
            CreationBoundary::UnknownRuntimeCode,
        ))?;
    if bytes.len() > 24_576 || bytes.first() == Some(&0xef) {
        *kind = OutcomeKind::Failure;
        *data = ByteArray::empty();
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
    *data = ByteArray::empty();
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
