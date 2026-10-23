//! Typed CREATE/CREATE2 entries installed as native Body/Resume dependencies.

use super::{
    super::{
        Composition, EvmExecution, EvmMachinePayload, EvmPayloadSummary, EvmSolver, EvmSuccessor,
        MachineError,
    },
    creation,
};
use crate::{
    analysis::{
        CreationContinuation, EvmChildFrame, EvmContinuation, EvmFrameState, FrameCode, FrameKey,
        MachineEdgeKind, ProcessAction, ProcessCreation, ProcessCreationTransfer, ProcessFrameKey,
        ProcessOwner, ProcessSource,
        engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    },
    domain::{EvmNumericAlgebra, summary},
    resource::clone_payment::checkpoint,
    world::{AddressInput, EvmByteArray},
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
    pub(super) fn creation_error(&mut self, error: creation::CreationError) -> MachineError {
        // Helper's descriptor error intentionally combines work/cancellation;
        // recover the original typed cause from the same sticky payer.
        match checkpoint(self.budget) {
            Err(error) => error.into(),
            Ok(()) => {
                if error == creation::CreationError::Uncompleted {
                    MachineError::Work
                } else {
                    MachineError::Shape
                }
            }
        }
    }

    pub(super) fn process_allocations(
        &mut self,
        payload: &EvmMachinePayload<D::Element>,
        action: ProcessAction,
        result: &mut EvmExecution<D::Element>,
    ) -> Result<ProcessCreationTransfer, MachineError> {
        let catalogue = Arc::clone(
            self.table
                .process_catalogue
                .as_ref()
                .ok_or(MachineError::Shape)?,
        );
        let parent = payload.active();
        let creator = parent
            .key
            .process
            .as_ref()
            .ok_or(MachineError::Shape)?
            .owner;
        let state = payload
            .store
            .summary_projection_with_payment(self.domain, self.budget)?;
        let nonce = state.process_nonce(creator, self.budget)?;
        self.reserve(catalogue.named().len().saturating_add(1).saturating_mul(64))?;
        let mut targets = Vec::with_capacity(catalogue.named().len().saturating_add(1));
        for owner in catalogue.unknown_targets() {
            targets.push(creation::Destination {
                owner,
                collision: if owner == creator && owner != ProcessOwner::Residual {
                    // Collision is checked after the creator increment.
                    creation::Collision::NonZeroNonce
                } else {
                    state.process_collision(owner, self.budget)?
                },
            });
        }
        let plan = creation::prepare(
            creation::CreationRequest {
                action,
                creator,
                is_static: parent.key.is_static,
                depth: creation::Admission::Unknown,
                nonce: nonce.map_or(
                    creation::CreatorNonce::Unknown,
                    creation::CreatorNonce::Exact,
                ),
                funding: creation::Admission::Unknown,
                initcode_size: creation::Admission::Unknown,
                code: creation::ConstructorCode::AnyInitCode,
            },
            &targets,
            0,
            self.budget,
        )
        .map_err(|error| self.creation_error(error))?;
        let needs_nonce =
            !plan.constructors.is_empty() || plan.rejections.iter().any(|r| r.increments_nonce);
        let post_nonce = if needs_nonce {
            Some(state.process_increment_nonce(creator, self.domain, self.budget)?)
        } else {
            None
        };
        // The guarded no-child paths are actual local process recurrences.
        // Private result zero/empty data is quotiented out, not passed through
        // native CALL completion or a fabricated instruction stack.
        for incremented in [false, true] {
            if !plan
                .rejections
                .iter()
                .any(|r| r.increments_nonce == incremented)
            {
                continue;
            }
            payload.pay_clone_with(self.budget)?;
            let mut resumed = payload.clone();
            if incremented {
                resumed.store = post_nonce
                    .as_ref()
                    .ok_or(MachineError::Shape)?
                    .instantiate_with_payment(self.domain, self.budget)?;
            }
            result.successors.push(EvmSuccessor {
                payload: resumed,
                kind: MachineEdgeKind::Process(action),
            });
        }
        for entry in plan.constructors {
            let post_nonce = post_nonce.as_ref().ok_or(MachineError::Shape)?;
            let initialized = post_nonce.process_begin_constructor(
                creator,
                entry.destination,
                self.domain,
                self.budget,
            )?;
            let saved_store = crate::world::EvmSnapshot::from_state(
                post_nonce.instantiate_with_payment(self.domain, self.budget)?,
            );
            let store = initialized.instantiate_with_payment(self.domain, self.budget)?;
            self.reserve(256)?;
            let mut key = FrameKey {
                process: None,
                code_address: Address::ZERO,
                code_hash: B256::ZERO,
                mode: FrameCode::InitCode,
                address: Address::ZERO,
                address_value: AddressInput::unknown_to(),
                caller: AddressInput::unknown_caller(),
                is_static: false,
                basic_block_index: 0,
                stack_height: 0,
                jump_history: Vec::new(),
            };
            key.install_process(ProcessFrameKey {
                epoch: catalogue.epoch(),
                owner: entry.destination,
                source: ProcessSource::AnyInitCode {
                    fork: self.world.fork(),
                },
                is_static: false,
            });
            // Unknown creation addresses are independent operands within the
            // selected cell; Named root namespaces need not be EVM constants.
            let address_operand = match entry.destination {
                ProcessOwner::Named(address) if address != self.entry.address => summary::constant(
                    U256::from_be_slice(address.as_slice()),
                    self.domain,
                    self.budget,
                )?,
                _ => summary::unsigned_range(
                    U256::ZERO,
                    U256::MAX >> 96usize,
                    self.domain,
                    self.budget,
                )?,
            };
            let caller_operand = summary::clone_value(
                parent.address_operand.as_ref().ok_or(MachineError::Shape)?,
                self.budget,
            )?;
            let endowment = summary::top(self.domain, self.budget)?;
            self.pay_process_arrays()?;
            payload.pay_clone_with(self.budget)?;
            let mut child = payload.clone();
            child.store = store;
            child.call_stack.push_child(EvmChildFrame {
                state: EvmFrameState {
                    key,
                    code: FrameCode::InitCode,
                    program: None,
                    stack: Vec::new(),
                    memory: EvmByteArray::memory(),
                    calldata: EvmByteArray::empty(),
                    environment_calldata: false,
                    returndata: EvmByteArray::empty(),
                    call_value: endowment,
                    address_operand: Some(address_operand),
                    caller_operand: Some(caller_operand),
                    saved_store,
                },
                continuation: EvmContinuation {
                    return_block: Some(0),
                    output_offset: summary::constant(U256::ZERO, self.domain, self.budget)?,
                    output_size: summary::constant(U256::ZERO, self.domain, self.budget)?,
                    creation: Some(CreationContinuation::Process(ProcessCreation {
                        action,
                        creator,
                        destination: entry.destination,
                        epoch: catalogue.epoch(),
                    })),
                },
            });
            child.normalize();
            let index = result.successors.len();
            result.successors.push(EvmSuccessor {
                payload: child,
                kind: MachineEdgeKind::Call,
            });
            result
                .process_calls
                .push((index, action, entry.destination));
        }
        Ok(ProcessCreationTransfer {
            action,
            epoch: catalogue.epoch(),
            creator,
            exceptions: plan.exceptions,
            rejections: plan.rejections,
        })
    }

    /// Apply every constructor output case before committing its subscriber
    /// stamp. Process-only parents forget private data while retaining typed
    /// address/zero/returndata/deployment receipts and shared Store effects.
    pub(in crate::analysis::transfer::procedure) fn deliver_process_creation(
        &mut self,
        subscriber: usize,
        from: usize,
        kind: crate::analysis::OutcomeKind,
        template: &EvmPayloadSummary<D::Element>,
        output: &super::super::EvmOutput<D::Element>,
        creation: ProcessCreation,
    ) -> Result<(), MachineError> {
        let catalogue = self
            .table
            .process_catalogue
            .as_ref()
            .ok_or(MachineError::Shape)?;
        if catalogue.epoch() != creation.epoch
            || template.frames.len() != 2
            || template.frames[0].key.process.is_none()
        {
            return Err(MachineError::Shape);
        }
        let cases = creation::complete(
            creation::ConstructorEntry {
                action: creation.action,
                creator: creation.creator,
                destination: creation.destination,
                code: creation::ConstructorCode::AnyInitCode,
                vocabulary: 0,
                nonce: creation::CreatorNonce::Unknown,
                actions: super::capabilities::ACTIONS,
            },
            kind,
            self.budget,
        )
        .map_err(|error| self.creation_error(error))?;
        self.reserve(256)?;
        self.regional_creation_cases(subscriber, kind, cases)?;
        for case in cases.into_iter().flatten() {
            let mut resumed = template.clone_with_payment(self.budget)?;
            let child = resumed.frames.pop().ok_or(MachineError::Shape)?;
            resumed.continuations.pop();
            resumed.location.frames.pop();
            resumed.store = match case.store {
                creation::StoreDisposition::CommitConstructor => output
                    .summary
                    .store
                    .process_deploy(creation.destination, self.domain, self.budget)?,
                creation::StoreDisposition::RestorePostNonceCheckpoint => {
                    let mut failed = output
                        .summary
                        .store
                        .instantiate_with_payment(self.domain, self.budget)?;
                    let saved = child
                        .saved_store
                        .instantiate_with_payment(self.domain, self.budget)?;
                    failed.restore(crate::world::EvmSnapshot::from_state(saved));
                    failed.summary_projection_with_payment(self.domain, self.budget)?
                }
            };
            resumed.storage_observations = Default::default();
            resumed.work = resumed.measure_work();
            let to =
                self.enqueue_summary(self.table.nodes[from].procedure, resumed, None, Some(from))?;
            self.observe_body_edge(from, to, MachineEdgeKind::Summary(kind))?;
        }
        Ok(())
    }
}
