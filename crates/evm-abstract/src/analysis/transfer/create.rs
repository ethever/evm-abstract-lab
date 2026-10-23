//! CREATE/CREATE2 lifecycle. Initcode is a fresh frame of the shared machine;
//! the destination's code remains empty until a successful RETURN installs it.

use crate::domain::{EvmNumericAlgebra, SingletonProof, WordElement, summary};

use super::budget::{byte_work, maximum};
use super::{EvmExecution, EvmSuccessor, TransferContext, boundary};
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

fn write_balance_literal<D: EvmNumericAlgebra>(
    store: &mut crate::world::EvmStore<D::Element>,
    address: Address,
    word: U256,
    context: &mut TransferContext<'_, D>,
) -> Result<(), FrontierReason> {
    store.write_balance_with_truth(
        address,
        summary::literal(word, context.domain, context.budget)?,
        word.is_zero(),
        !word.is_zero(),
    );
    Ok(())
}

/// Range admission already owns discovery. Allocation consumes the retained
/// terms, literal certificates and finite views, including a negative answer,
/// without asking the unchanged operands for another singleton.
pub(super) fn expand_prepared<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
) -> Result<(), FrontierReason> {
    query::checkpoint(context.budget)?;
    if size.expression().and_then(ExprId::as_constant) != Some(U256::ZERO) {
        let length = result.payload.active().memory.len();
        let (lower, upper) = length.interval().unsigned_bounds();
        let independently_known = length.expression().and_then(ExprId::as_constant).is_some()
            || length.constants().is_some_and(|values| values.len() == 1)
            || length.known_bits().singleton().is_some()
            || lower == upper;
        if !independently_known
            && let Some(word) = query::singleton(length.numeric(), context.budget)?
        {
            // Independent allocation also needs the unchanged old MSIZE.
            // Retain its exact opaque answer without copying N. One constant
            // leaf plus release of the previous term have their own owner.
            let work = length
                .expression()
                .map_or(0, ExprId::work_size)
                .saturating_add(2);
            query::checkpoint(context.budget)?;
            if work == usize::MAX || !context.budget.charge(work) {
                return Err(FrontierReason::Work);
            }
            let expression = ExprId::constant(word);
            query::checkpoint(context.budget)?;
            result
                .payload
                .active_mut()
                .memory
                .retain_length_literal(expression);
            query::checkpoint(context.budget)?;
        }
    }
    let work = byte_work(
        &[offset, size],
        0,
        result.payload.active().memory.work_size(),
        context.domain,
    );
    if work == usize::MAX || !context.budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    query::checkpoint(context.budget)?;
    result
        .payload
        .active_mut()
        .memory
        .expand_independent(offset, size, context.domain)
        .map_err(super::errors::bytes)?;
    query::checkpoint(context.budget)
}

fn immediate_failure<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    mut payload: EvmMachinePayload<D::Element>,
    next: usize,
    context: &mut TransferContext<'_, D>,
) -> Result<(), FrontierReason> {
    let zero = summary::literal(U256::ZERO, context.domain, context.budget)?;
    let data = EvmByteArray::empty_with_payment(context.domain, context.budget)?;
    let caller = payload.active_mut();
    caller.stack.push(zero);
    caller.returndata = data;
    caller.key.basic_block_index = next;
    payload.normalize();
    result.successors.push(EvmSuccessor {
        payload,
        kind: MachineEdgeKind::Failure,
    });
    query::checkpoint(context.budget)
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

/// A prefix can reject runtime code without materializing the unknown suffix.
enum Materialized {
    Bytes(Vec<u8>),
    ForbiddenPrefix,
    Unknown,
}

/// Consume the caller's existing extraction reservation and proved host extent.
/// Constructing the view does not rediscover its numeric length.
/// Each newly read opaque byte still needs its own independent query payment.
fn materialize<D: EvmNumericAlgebra>(
    data: &EvmByteArray<D::Element>,
    length: usize,
    forbidden_prefix: Option<u8>,
    context: &mut TransferContext<'_, D>,
) -> Result<Materialized, FrontierReason> {
    query::checkpoint(context.budget)?;
    data.check_existing(context.domain.spec().relations().expressions())
        .map_err(super::errors::bytes)?;
    let over_cap = length > context.config.max_memory_bytes;
    if over_cap && forbidden_prefix.is_none() {
        return Ok(Materialized::Unknown);
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
    if !over_cap {
        let known = view.known_bytes(limits);
        query::checkpoint(context.budget)?;
        if let Some(bytes) = known.map_err(super::errors::expression)? {
            return Ok(Materialized::Bytes(bytes));
        }
    }
    let mut bytes = Vec::new();
    // A forbidden first byte settles runtime rejection without extracting the
    // unknown suffix, even above the full-buffer host cap. Reuse this actual
    // byte's paid proof; the length proof cannot classify a different N.
    let reads = if over_cap { 1 } else { length };
    for index in 0..reads {
        query::checkpoint(context.budget)?;
        let value = data
            .read_byte_checked(
                &summary::literal(U256::from(index), context.domain, context.budget)?,
                context.domain,
            )
            .map_err(super::errors::bytes)?;
        query::checkpoint(context.budget)?;
        let (_, mut proof) = candidates::prepare(&value, context.budget)?;
        let Some(byte) = query::resolve(value.numeric(), &mut proof, context.budget)? else {
            return Ok(Materialized::Unknown);
        };
        let Ok(byte) = u8::try_from(byte) else {
            return Ok(Materialized::Unknown);
        };
        if index == 0 && forbidden_prefix == Some(byte) {
            return Ok(Materialized::ForbiddenPrefix);
        }
        if over_cap {
            return Ok(Materialized::Unknown);
        }
        bytes.push(byte);
    }
    Ok(Materialized::Bytes(bytes))
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
        super::calls::creation_failure(result, context, pc);
    }
    let sizes: Vec<_> = sizes
        .iter()
        .filter(|size| **size <= U256::from(49_152))
        .copied()
        .collect();
    let mut memory_size = None;
    for size in &sizes {
        let value = summary::literal(*size, domain, context.budget)?;
        memory_size = Some(match memory_size {
            None => value,
            Some(previous) => summary::join(&previous, &value, domain, false, context.budget)?,
        });
    }
    let Some(mut memory_size) = memory_size else {
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
        // Carry the host literal into allocation too, rather than making the
        // opaque element rediscover the word after range admission.
        memory_size = memory_size.with_expression(ExprId::constant(sizes[0]));
        SingletonProof::Answer(Some(sizes[0]))
    } else {
        SingletonProof::NotQueried
    };
    let proved_offset = if let SingletonProof::Answer(Some(word)) = offset_proof
        && args[1].expression().and_then(ExprId::as_constant) != Some(word)
    {
        // Allocation's value API cannot consume SingletonProof directly. Keep
        // the exact paid word on the same N as a literal term, with the copy
        // reserved before construction rather than another numeric query.
        query::checkpoint(context.budget)?;
        let work = args[1].clone_work();
        query::checkpoint(context.budget)?;
        if work == usize::MAX || !context.budget.charge(work) {
            return Err(FrontierReason::Work);
        }
        query::checkpoint(context.budget)?;
        let value = args[1].clone().with_expression(ExprId::constant(word));
        query::checkpoint(context.budget)?;
        Some(value)
    } else {
        None
    };
    let memory_offset = proved_offset.as_ref().unwrap_or(&args[1]);
    if !super::ranges::memory_after_queries(
        result,
        memory_offset,
        &memory_size,
        offset_proof,
        size_proof,
        context,
        pc,
    ) {
        return Ok(());
    }
    expand_prepared(result, memory_offset, &memory_size, context)?;
    // revm/Yellow-Paper depth starts at zero for the transaction entry. Child
    // depth equals the current frame count and must not exceed 1024.
    if result.payload.call_stack.depth() > 1024 {
        result.payload.pay_clone_with(context.budget)?;
        immediate_failure(result, result.payload.clone(), next, context)?;
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
    let nonce = result
        .payload
        .store
        .nonce_with_payment(caller_address, domain, context.budget)?;
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
    let balance = result
        .payload
        .store
        .balance_with_payment(caller_address, context.budget)?;
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
            let offset_value = summary::literal(*offset, domain, context.budget)?;
            let size_value = summary::literal(*size, domain, context.budget)?;
            if !context.budget.charge(byte_work(
                &[&offset_value, &size_value],
                usize::try_from(*size).unwrap_or(usize::MAX),
                result.payload.active().memory.work_size(),
                domain,
            )) {
                return Err(FrontierReason::Work);
            }
            let extracted = if size.is_zero() {
                Ok(EvmByteArray::empty_with_payment(domain, context.budget)?)
            } else {
                result
                    .payload
                    .active()
                    .memory
                    .slice(&offset_value, &size_value, domain)
            };
            let initcode = match extracted {
                Ok(bytes) => {
                    match materialize(
                        &bytes,
                        usize::try_from(*size).expect("bounded initcode size"),
                        None,
                        context,
                    )? {
                        Materialized::Bytes(bytes) => bytes,
                        Materialized::Unknown | Materialized::ForbiddenPrefix => {
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
                    result.payload.pay_clone_with(context.budget)?;
                    let mut payload = result.payload.clone();
                    payload.store.write_nonce(
                        caller_address,
                        summary::literal(*nonce, domain, context.budget)?,
                    );
                    immediate_failure(result, payload, next, context)?;
                    continue;
                }
                for value in values.iter() {
                    for balance in &balances {
                        if balance.is_some_and(|balance| *value > balance) {
                            result.payload.pay_clone_with(context.budget)?;
                            let mut payload = result.payload.clone();
                            payload.store.write_nonce(
                                caller_address,
                                summary::literal(*nonce, domain, context.budget)?,
                            );
                            write_balance_literal(
                                &mut payload.store,
                                caller_address,
                                balance.expect("insufficient funds require an observed balance"),
                                context,
                            )?;
                            immediate_failure(result, payload, next, context)?;
                            continue;
                        }
                        for salt in &salts {
                            crate::resource::clone_payment::pay(
                                context.budget,
                                initcode.len().saturating_add(1),
                            )?;
                            let destination = if op == opcode::CREATE {
                                caller_address.create(nonce.to::<u64>())
                            } else {
                                caller_address
                                    .create2_from_code(salt.to_be_bytes::<32>(), &initcode)
                            };
                            result.payload.pay_clone_with(context.budget)?;
                            let mut payload = result.payload.clone();
                            if let Some(balance) = balance {
                                write_balance_literal(
                                    &mut payload.store,
                                    caller_address,
                                    *balance,
                                    context,
                                )?;
                            }
                            // The creator's nonce change survives a failed
                            // initcode child, but rolls back with its own frame.
                            payload.store.touch_account(caller_address);
                            payload.store.write_nonce(
                                caller_address,
                                summary::literal(*nonce + U256::from(1), domain, context.budget)?,
                            );
                            // Gas remains abstract, but insufficient initcode gas
                            // fails after the caller nonce has been incremented.
                            payload.pay_clone_with(context.budget)?;
                            immediate_failure(result, payload.clone(), next, context)?;
                            // The savepoint deliberately includes the caller nonce bump.
                            payload.store.pay_clone_with(context.budget)?;
                            let saved_store = payload.store.snapshot();
                            if reserved(world, destination) {
                                incomplete(
                                    result,
                                    pc,
                                    CreationBoundary::ReservedAddress(destination),
                                );
                                continue;
                            }
                            let destination_nonce = payload.store.nonce_with_payment(
                                destination,
                                domain,
                                context.budget,
                            )?;
                            let code = payload.store.raw_account_code(destination);
                            if code.as_ref().is_some_and(|code| !code.is_empty()) {
                                // Either known fact alone proves a collision; the
                                // other account observation need not be present.
                                immediate_failure(result, payload, next, context)?;
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
                                immediate_failure(result, payload, next, context)?;
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
                                payload.pay_clone_with(context.budget)?;
                                immediate_failure(result, payload.clone(), next, context)?;
                            }
                            if !code.is_empty() || !destination_nonces.contains(&U256::ZERO) {
                                continue;
                            }
                            if initcode.first() == Some(&0xef) {
                                // EF is invalid EVM initcode under every supported fork;
                                // it must not be interpreted as an account delegation marker.
                                immediate_failure(result, payload, next, context)?;
                                continue;
                            }
                            let init_program = Program::decode_with_fork(&initcode, world.fork())
                                .expect("EVM initcode with a non-EF first byte always decodes");
                            let recipient = payload
                                .store
                                .balance_with_payment(destination, context.budget)?;
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
                                payload.pay_clone_with(context.budget)?;
                                let mut branch = payload.clone();
                                if let Some(recipient) = recipient {
                                    let Some(total) = recipient.checked_add(*value) else {
                                        immediate_failure(result, branch, next, context)?;
                                        continue;
                                    };
                                    write_balance_literal(
                                        &mut branch.store,
                                        destination,
                                        total,
                                        context,
                                    )?;
                                }
                                if let Some(balance) = balance {
                                    write_balance_literal(
                                        &mut branch.store,
                                        caller_address,
                                        *balance - *value,
                                        context,
                                    )?;
                                }
                                branch.store.begin_creation_with_payment(
                                    destination,
                                    domain,
                                    context.budget,
                                )?;
                                branch.call_stack.push_child(EvmChildFrame {
                                    state: EvmFrameState {
                                        key: FrameKey {
                                            process: None,
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
                                        memory: EvmByteArray::memory_with_payment(
                                            domain,
                                            context.budget,
                                        )?,
                                        calldata: EvmByteArray::empty_with_payment(
                                            domain,
                                            context.budget,
                                        )?,
                                        environment_calldata: false,
                                        address_operand: None,
                                        caller_operand: None,
                                        returndata: EvmByteArray::empty_with_payment(
                                            domain,
                                            context.budget,
                                        )?,
                                        call_value: summary::literal(
                                            *value,
                                            domain,
                                            context.budget,
                                        )?,
                                        saved_store: saved_store.clone(),
                                    },
                                    continuation: EvmContinuation {
                                        return_block: Some(next),
                                        output_offset: summary::literal(U256::ZERO,domain,context.budget)?,
                                        output_size: summary::literal(U256::ZERO,domain,context.budget)?,
                                        creation: Some(crate::analysis::CreationContinuation::Native { destination }),
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
        .and_then(crate::analysis::CreationContinuation::native_address)
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
        *data = EvmByteArray::empty_with_payment(context.domain, context.budget)?;
        return Ok(());
    }
    let length = query::resolve(data.len().numeric(), &mut proof, context.budget)?
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(FrontierReason::Creation(
            CreationBoundary::UnknownRuntimeCode,
        ))?;
    let bytes = match materialize(data, length, Some(0xef), context)? {
        Materialized::Bytes(bytes) => bytes,
        Materialized::ForbiddenPrefix => {
            *kind = OutcomeKind::Failure;
            *data = EvmByteArray::empty_with_payment(context.domain, context.budget)?;
            return Ok(());
        }
        Materialized::Unknown => {
            return Err(FrontierReason::Creation(
                CreationBoundary::UnknownRuntimeCode,
            ));
        }
    };
    if bytes.len() > 24_576 || bytes.first() == Some(&0xef) {
        *kind = OutcomeKind::Failure;
        *data = EvmByteArray::empty_with_payment(context.domain, context.budget)?;
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
    *data = EvmByteArray::empty_with_payment(context.domain, context.budget)?;
    Ok(())
}

#[cfg(test)]
pub(super) mod test_support;

#[cfg(test)]
mod tests {

    mod lifecycle;
    mod payment;

    use super::reserved;
    use crate::{Fork, analysis::testing::World};
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
