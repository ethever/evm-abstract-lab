//! Primitive process CALL-family dependencies, including a residual chain.
//!
//! The class controls storage effects, the source controls code capabilities,
//! and CallRoles binds actual logical operands. An internal frame anchor is
//! never resolved, touched or executed as a residual representative account.

use super::super::Composition;
use super::super::{
    AddressInput, EvmByteArray, EvmChildFrame, EvmContinuation, EvmExecution, EvmFrameState,
    EvmMachinePayload, EvmSolver, EvmSuccessor, FrameCode, FrameKey, FrontierReason,
    MachineEdgeKind, MachineError, StorageReadPolicy, calls as native_calls,
};
use crate::{
    analysis::{
        ProcessAction, ProcessFrameKey, ProcessOwner, ProcessSource,
        engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    },
    domain::{EvmNumericAlgebra, summary},
    world::process::{CallRoles, ProcessCallOperands, ProcessTargetAddress},
};
use alloy_primitives::{Address, B256, U256};
use std::sync::Arc;

impl<D, C, P, S, X, E> EvmSolver<'_, D, C, P, S, X, E>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
{
    pub(super) fn process_calls(
        &mut self,
        payload: &EvmMachinePayload<D::Element>,
        action: ProcessAction,
        result: &mut EvmExecution<D::Element>,
    ) -> Result<(), FrontierReason> {
        let catalogue = Arc::clone(
            self.table
                .process_catalogue
                .as_ref()
                .ok_or_else(|| MachineError::Shape.frontier_reason())?,
        );
        self.reserve(
            catalogue
                .named()
                .len()
                .saturating_add(1)
                .saturating_mul(256),
        )
        .map_err(MachineError::frontier_reason)?;
        let targets: Vec<_> = catalogue.unknown_targets().collect();
        let root_address = Arc::clone(
            self.table
                .process_root_address
                .as_ref()
                .ok_or_else(|| MachineError::Shape.frontier_reason())?,
        );
        let parent = payload.active();
        let parent_owner = parent
            .key
            .process
            .as_ref()
            .ok_or_else(|| MachineError::Shape.frontier_reason())?
            .owner;
        let roles = CallRoles::for_action(action, parent.key.is_static)
            .map_err(super::admission::owner_error)
            .map_err(MachineError::frontier_reason)?;
        let parent_operands = ProcessCallOperands {
            address: summary::clone_value(
                parent
                    .address_operand
                    .as_ref()
                    .ok_or_else(|| MachineError::Shape.frontier_reason())?,
                self.budget,
            )?,
            caller: summary::clone_value(
                parent
                    .caller_operand
                    .as_ref()
                    .ok_or_else(|| MachineError::Shape.frontier_reason())?,
                self.budget,
            )?,
            value: summary::clone_value(&parent.call_value, self.budget)?,
        };
        for target in targets {
            let (code_address, code, program, hash) = match target {
                ProcessOwner::Named(address) => {
                    let (source, code, program) =
                        native_calls::resolve(self.world, &payload.store, address)?;
                    if code == FrameCode::Unknown
                        && self.storage_reads == StorageReadPolicy::Discover
                    {
                        super::super::boundary_on(
                            result,
                            0,
                            FrontierReason::MissingCode(source),
                            payload.key(),
                        );
                        continue;
                    }
                    self.reserve(
                        program
                            .as_ref()
                            .map_or(0, |p| p.byte_len())
                            .saturating_mul(4)
                            .saturating_add(64),
                    )
                    .map_err(MachineError::frontier_reason)?;
                    let hash = native_calls::captured_hash(&program, &payload.store, source, code);
                    (source, code, program, hash)
                }
                ProcessOwner::Residual => (Address::ZERO, FrameCode::Unknown, None, B256::ZERO),
            };
            let target_address = match target {
                ProcessOwner::Named(address) if address == self.entry.address => {
                    ProcessTargetAddress::Operand(root_address.as_ref())
                }
                ProcessOwner::Named(address) => ProcessTargetAddress::Exact(address),
                ProcessOwner::Residual => ProcessTargetAddress::Independent,
            };
            let operands = roles
                .bind(target_address, &parent_operands, self.domain, self.budget)
                .map_err(super::admission::owner_error)
                .map_err(MachineError::frontier_reason)?;
            let owner = roles.child_owner(parent_owner, target);
            let fork = self.world.fork();
            let source = match code {
                FrameCode::Runtime => ProcessSource::Runtime {
                    source: code_address,
                    hash,
                    fork: program
                        .as_ref()
                        .expect("runtime retains its Program")
                        .fork(),
                },
                FrameCode::Unknown => ProcessSource::AnyRuntime { fork },
                FrameCode::Empty => ProcessSource::Empty { fork },
                FrameCode::Precompile(address) => ProcessSource::Precompile { address, fork },
                FrameCode::InvalidDelegation => ProcessSource::InvalidDelegation {
                    source: code_address,
                    hash,
                    fork,
                },
                FrameCode::InitCode => {
                    unreachable!("account CALL resolution never selects initcode")
                }
            };
            self.reserve(256).map_err(MachineError::frontier_reason)?;
            let mut key = FrameKey {
                process: None,
                code_address,
                code_hash: hash,
                mode: code,
                address: Address::ZERO,
                address_value: AddressInput::unknown_to(),
                caller: AddressInput::unknown_caller(),
                is_static: roles.is_static,
                basic_block_index: 0,
                stack_height: 0,
                jump_history: Vec::new(),
            };
            key.install_process(ProcessFrameKey {
                epoch: catalogue.epoch(),
                owner,
                source,
                is_static: roles.is_static,
            });
            payload.pay_clone_with(self.budget)?;
            self.pay_process_arrays()
                .map_err(MachineError::frontier_reason)?;
            let mut callback = payload.clone();
            let saved_store = payload.store.snapshot();
            if matches!(action, ProcessAction::Call | ProcessAction::StaticCall) {
                if let ProcessOwner::Named(address) = target {
                    if address == self.entry.address
                        && self.entry.environment.to.as_concrete().is_none()
                    {
                        callback.store.may_touch_all();
                    } else {
                        callback.store.touch_account(address);
                    }
                } else {
                    callback.store.may_touch_all();
                }
            }
            callback.call_stack.push_child(EvmChildFrame {
                state: EvmFrameState {
                    key,
                    code,
                    program,
                    stack: Vec::new(),
                    memory: EvmByteArray::memory(),
                    calldata: EvmByteArray::unknown_checked()
                        .map_err(|e| MachineError::Bytes(e).frontier_reason())?,
                    environment_calldata: false,
                    returndata: EvmByteArray::empty(),
                    call_value: operands.value,
                    address_operand: Some(operands.address),
                    caller_operand: Some(operands.caller),
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
            let successor = result.successors.len();
            result.successors.push(EvmSuccessor {
                payload: callback,
                kind: MachineEdgeKind::Call,
            });
            result.process_calls.push((successor, action, target));
        }
        Ok(())
    }
}
