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
            let (code_address, code, program, hash, source) = match target {
                ProcessOwner::RootNamespace => {
                    let (code, program, source) = self
                        .table
                        .process_root_code
                        .as_ref()
                        .ok_or_else(|| MachineError::Shape.frontier_reason())?;
                    let (address, hash) = match *source {
                        ProcessSource::Runtime { source, hash, .. }
                        | ProcessSource::InvalidDelegation { source, hash, .. } => (source, hash),
                        ProcessSource::InputProgram { hash, .. } => (Address::ZERO, hash),
                        ProcessSource::InputDelegation { target, .. } => (target, B256::ZERO),
                        _ => (Address::ZERO, B256::ZERO),
                    };
                    (address, *code, program.clone(), hash, *source)
                }
                ProcessOwner::Named(address) => {
                    // A delegation pointer is also a physical address. Never
                    // follow it into the private root template just because
                    // its numeric bits happen to equal that coordinate.
                    let (source, code, program) =
                        native_calls::resolve_code(self.world.fork(), address, |physical| {
                            if catalogue
                                .namespace()
                                .is_some_and(|n| n.coordinate == physical)
                            {
                                None
                            } else {
                                payload.store.code(physical)
                            }
                        });
                    let program = program.cloned();
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
                    let provenance = match code {
                        FrameCode::Runtime => ProcessSource::Runtime {
                            source,
                            hash,
                            fork: program.as_ref().expect("runtime Program").fork(),
                        },
                        FrameCode::Unknown => ProcessSource::AnyRuntime {
                            fork: self.world.fork(),
                        },
                        FrameCode::Empty => ProcessSource::Empty {
                            fork: self.world.fork(),
                        },
                        FrameCode::Precompile(address) => ProcessSource::Precompile {
                            address,
                            fork: self.world.fork(),
                        },
                        FrameCode::InvalidDelegation => ProcessSource::InvalidDelegation {
                            source,
                            hash,
                            fork: self.world.fork(),
                        },
                        FrameCode::InitCode => {
                            unreachable!("account resolution cannot select initcode")
                        }
                    };
                    (source, code, program, hash, provenance)
                }
                ProcessOwner::Residual => (
                    Address::ZERO,
                    FrameCode::Unknown,
                    None,
                    B256::ZERO,
                    ProcessSource::AnyRuntime {
                        fork: self.world.fork(),
                    },
                ),
            };
            let target_address = match target {
                ProcessOwner::RootNamespace => ProcessTargetAddress::Operand(root_address.as_ref()),
                ProcessOwner::Named(address) => ProcessTargetAddress::Exact(address),
                ProcessOwner::Residual => ProcessTargetAddress::Independent,
            };
            let operands = roles
                .bind(target_address, &parent_operands, self.domain, self.budget)
                .map_err(super::admission::owner_error)
                .map_err(MachineError::frontier_reason)?;
            let owner = roles.child_owner(parent_owner, target);
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
                location: if program.is_some() {
                    crate::analysis::ProcessLocation::Block(0)
                } else {
                    crate::analysis::ProcessLocation::Any
                },
            });
            payload.pay_clone_with(self.budget)?;
            let memory = EvmByteArray::memory_with_payment(self.domain, self.budget)?;
            let calldata = EvmByteArray::empty_with_payment(self.domain, self.budget)?;
            let returndata = EvmByteArray::empty_with_payment(self.domain, self.budget)?;
            let mut callback = payload.clone();
            let saved_store = payload.store.snapshot();
            if matches!(action, ProcessAction::Call | ProcessAction::StaticCall) {
                if let ProcessOwner::Named(address) = target {
                    callback.store.touch_account(address);
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
                    memory,
                    // Private calldata is arbitrary under the Process
                    // interpretation, independent of this adapter placeholder.
                    calldata,
                    environment_calldata: false,
                    returndata,
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
