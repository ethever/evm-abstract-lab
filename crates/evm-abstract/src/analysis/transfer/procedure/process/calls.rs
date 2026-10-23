//! Named-target EVM action preparation for process bodies.
//!
//! Effects have already passed through the covering envelope. The ordinary
//! native subscriber stores each pre-child checkpoint. Unlisted target classes
//! remain explicit residual obligations rather than concrete representative
//! accounts. No bytecode instruction or stack execution is synthesized here.

use super::super::Composition;
use super::super::{
    AddressInput, EvmByteArray, EvmChildFrame, EvmContinuation, EvmExecution, EvmFrameState,
    EvmMachinePayload, EvmSolver, EvmSuccessor, FrameCode, FrameKey, FrontierReason,
    MachineEdgeKind, MachineError, StorageReadPolicy, calls as native_calls,
};
use crate::{
    analysis::{
        ProcessAction,
        engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    },
    domain::{EvmNumericAlgebra, summary},
};
use alloy_primitives::U256;

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    pub(super) fn process_calls(
        &mut self,
        payload: &EvmMachinePayload<D::Element>,
        action: ProcessAction,
        result: &mut EvmExecution<D::Element>,
    ) -> Result<(), FrontierReason> {
        self.reserve(
            payload
                .store
                .clone_prefix_work()
                .saturating_add(self.world.accounts().len())
                .saturating_mul((usize::BITS as usize + 1).saturating_mul(64)),
        )
        .map_err(MachineError::frontier_reason)?;
        let mut targets = payload.store.addresses();
        targets.extend(self.world.accounts().keys().copied());
        self.reserve(targets.len().saturating_mul(32).saturating_add(8))
            .map_err(MachineError::frontier_reason)?;
        for target in targets {
            let (code_address, code, program) =
                native_calls::resolve(self.world, &payload.store, target)?;
            if code == FrameCode::Unknown && self.storage_reads == StorageReadPolicy::Discover {
                super::super::boundary_on(
                    result,
                    0,
                    FrontierReason::MissingCode(code_address),
                    payload.key(),
                );
                continue;
            }
            payload.pay_clone_with(self.budget)?;
            payload.store.pay_clone_with(self.budget)?;
            self.pay_process_arrays()
                .map_err(MachineError::frontier_reason)?;
            self.reserve(
                program
                    .as_ref()
                    .map_or(0, |p| p.byte_len())
                    .saturating_mul(4)
                    .saturating_add(160),
            )
            .map_err(MachineError::frontier_reason)?;
            let parent = payload.active();
            let is_static = parent.key.is_static || action == ProcessAction::StaticCall;
            let target_logical = if target == self.entry.address {
                self.entry.environment.to
            } else {
                AddressInput::Concrete(target)
            };
            let (owner, logical, caller, address_operand, caller_operand, value) = match action {
                ProcessAction::Call | ProcessAction::StaticCall => {
                    // An unknown process includes arbitrary internal unknown
                    // chains, so a protected-owner callback's caller need not
                    // be this process's direct concrete address.
                    let (caller, operand) = if parent.code == FrameCode::Unknown {
                        (
                            AddressInput::unknown_caller(),
                            Some(summary::unsigned_range(
                                U256::ZERO,
                                U256::MAX >> 96usize,
                                self.domain,
                                self.budget,
                            )?),
                        )
                    } else {
                        (
                            parent.key.address_value,
                            Some(summary::clone_value(
                                parent.address_operand.as_ref().expect("projected ADDRESS"),
                                self.budget,
                            )?),
                        )
                    };
                    let value = if is_static {
                        summary::constant(U256::ZERO, self.domain, self.budget)?
                    } else {
                        summary::top(self.domain, self.budget)?
                    };
                    (target, target_logical, caller, None, operand, value)
                }
                ProcessAction::DelegateCall => (
                    parent.key.address,
                    parent.key.address_value,
                    parent.key.caller,
                    Some(summary::clone_value(
                        parent.address_operand.as_ref().expect("projected ADDRESS"),
                        self.budget,
                    )?),
                    Some(summary::clone_value(
                        parent.caller_operand.as_ref().expect("projected CALLER"),
                        self.budget,
                    )?),
                    summary::clone_value(&parent.call_value, self.budget)?,
                ),
                ProcessAction::CallCode => (
                    parent.key.address,
                    parent.key.address_value,
                    parent.key.address_value,
                    Some(summary::clone_value(
                        parent.address_operand.as_ref().expect("projected ADDRESS"),
                        self.budget,
                    )?),
                    Some(summary::clone_value(
                        parent.address_operand.as_ref().expect("projected ADDRESS"),
                        self.budget,
                    )?),
                    // EIP-214 restricts nonzero CALL, but CALLCODE performs no
                    // balance transfer and may pass a nonzero value in static
                    // mode. The child still inherits the static restriction.
                    summary::top(self.domain, self.budget)?,
                ),
                ProcessAction::Create | ProcessAction::Create2 => {
                    unreachable!("allocation stays a typed rule obligation")
                }
            };
            let mut callback = payload.clone();
            let saved_store = payload.store.snapshot();
            if matches!(action, ProcessAction::Call | ProcessAction::StaticCall)
                && target != parent.key.address
            {
                callback.store.touch_account(target);
            }
            callback.call_stack.push_child(EvmChildFrame {
                state: EvmFrameState {
                    key: FrameKey {
                        code_address,
                        code_hash: native_calls::captured_hash(
                            &program,
                            &payload.store,
                            code_address,
                            code,
                        ),
                        mode: code,
                        address: owner,
                        address_value: logical,
                        caller,
                        is_static,
                        basic_block_index: 0,
                        stack_height: 0,
                        jump_history: Vec::new(),
                    },
                    code,
                    program,
                    stack: Vec::new(),
                    memory: EvmByteArray::memory(),
                    calldata: EvmByteArray::unknown_checked()
                        .map_err(|e| MachineError::Bytes(e).frontier_reason())?,
                    environment_calldata: false,
                    returndata: EvmByteArray::empty(),
                    call_value: value,
                    address_operand,
                    caller_operand,
                    saved_store,
                },
                continuation: EvmContinuation {
                    return_block: Some(0),
                    output_offset: summary::constant(U256::ZERO, self.domain, self.budget)?,
                    output_size: summary::constant(U256::ZERO, self.domain, self.budget)?,
                    creation: None,
                },
            });
            callback.normalize();
            self.reserve(8).map_err(MachineError::frontier_reason)?;
            result.process_calls.push((
                result.successors.len(),
                action,
                crate::analysis::ProcessOwner::Named(target),
            ));
            result.successors.push(EvmSuccessor {
                payload: callback,
                kind: MachineEdgeKind::Call,
            });
        }
        Ok(())
    }
}
