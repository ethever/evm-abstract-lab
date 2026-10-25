//! Arbitrary local effects and repeatable ordinary calls into protected owners.

use super::Composition;
use crate::analysis::engine::{ContextPolicy, PartitionPolicy, StatePolicy};
use crate::domain::EvmNumericAlgebra;

use super::{
    AddressInput, BTreeSet, Code, EvmByteArray, EvmChildFrame, EvmContinuation, EvmExecution,
    EvmFrameState, EvmMachinePayload, EvmSolver, EvmSuccessor, EvmValue, FrameCode, FrameKey,
    FrontierReason, MachineEdgeKind, MachineError, OutcomeKind, StorageReadPolicy, TransferContext,
    boundary_on, calls, keccak256, zero,
};

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    pub(super) fn environment(
        &mut self,
        mut payload: EvmMachinePayload<D::Element>,
    ) -> EvmExecution<D::Element> {
        let mut result = super::super::empty_execution(payload.clone());
        let protected = self
            .world
            .accounts()
            .keys()
            .copied()
            .chain(payload.store.addresses())
            .filter(|owner| {
                matches!(
                    payload.store.code(*owner),
                    Some(Code::Runtime(_) | Code::Delegation(_))
                )
            })
            .collect::<BTreeSet<_>>();
        let owner = payload.active().key.address;
        let is_static = payload.active().key.is_static;
        if let Err(error) = self.reserve(payload.work_size().saturating_mul(4)) {
            boundary_on(&mut result, 0, error.frontier_reason(), payload.key());
            return result;
        }
        if let Err(error) =
            payload
                .store
                .unknown_effects(&protected, Some(owner), is_static, self.domain)
        {
            boundary_on(
                &mut result,
                0,
                MachineError::Store(error).frontier_reason(),
                payload.key(),
            );
            return result;
        }
        let data = match EvmByteArray::unknown_checked() {
            Ok(data) => data,
            Err(error) => {
                boundary_on(
                    &mut result,
                    0,
                    MachineError::Bytes(error).frontier_reason(),
                    payload.key(),
                );
                return result;
            }
        };
        {
            let mut context = TransferContext {
                config: self.config,
                domain: self.domain,
                budget: self.budget,
                root_address: Some((self.entry.address, self.entry.environment.to)),
                input_scope: self.entry.environment.input_scope.id(),
                query_cache: Default::default(),
                procedure: true,
                discover_code: self.storage_reads == StorageReadPolicy::Discover,
            };
            for kind in [
                OutcomeKind::Return,
                OutcomeKind::Revert,
                OutcomeKind::Failure,
            ] {
                calls::finish(
                    &mut result,
                    payload.clone(),
                    kind,
                    if kind == OutcomeKind::Failure {
                        EvmByteArray::empty()
                    } else {
                        data.clone()
                    },
                    &mut context,
                    0,
                );
            }
        }
        // Static callbacks preserve store values, but their known control and
        // nested call/return obligations still belong to this relative system.
        // The envelope above covers zero callbacks; subscribers cover repeated
        // calls with the same static mode and an independent rollback snapshot.
        // Internal steps and arbitrary chains of unknown callees are included
        // in the effect envelope. Each known-code entry may follow any such
        // chain, so CALLER is an independent address, not just the direct U.
        // Delegating known code keeps execution at this unknown process's
        // owner: its local effects are already in that envelope. Any switch to
        // another protected owner requires a normal call, covered by this loop.
        // An unknown DELEGATECALL/CALLCODE entered by known code still applies
        // the envelope to its actual known writable owner above.
        let targets = protected.iter().copied().collect::<Vec<_>>();
        for target in targets {
            let (code_address, code, program) =
                match calls::resolve(self.world, &payload.store, target) {
                    Ok(resolved) => resolved,
                    Err(reason) => {
                        boundary_on(&mut result, 0, reason, payload.key());
                        continue;
                    }
                };
            if code == FrameCode::Unknown {
                // A delegated missing implementation is another process in
                // this same table, not an empty-code callback.
                if self.storage_reads == StorageReadPolicy::Discover {
                    boundary_on(
                        &mut result,
                        0,
                        FrontierReason::MissingCode(code_address),
                        payload.key(),
                    );
                    continue;
                }
            }
            {
                if let Err(error) = self.reserve(payload.work_size().saturating_mul(4)) {
                    boundary_on(&mut result, 0, error.frontier_reason(), payload.key());
                    return result;
                }
                let calldata = match EvmByteArray::unknown_checked() {
                    Ok(data) => data,
                    Err(error) => {
                        boundary_on(
                            &mut result,
                            0,
                            MachineError::Bytes(error).frontier_reason(),
                            payload.key(),
                        );
                        continue;
                    }
                };
                let logical_owner = if target == self.entry.address {
                    self.entry.environment.to
                } else {
                    AddressInput::Concrete(target)
                };
                let mut callback = payload.clone();
                callback.call_stack.push_child(EvmChildFrame {
                    state: EvmFrameState {
                        key: FrameKey {
                            process: None,
                            code_address,
                            code_hash: program.as_ref().map_or_else(
                                || keccak256([]),
                                |program| keccak256(program.bytes()),
                            ),
                            mode: code,
                            address: target,
                            address_value: logical_owner,
                            caller: AddressInput::unknown_caller(),
                            is_static,
                            basic_block_index: 0,
                            stack_height: 0,
                            jump_history: Vec::new(),
                        },
                        code,
                        program: program.clone(),
                        stack: Vec::new(),
                        memory: EvmByteArray::memory(),
                        calldata,
                        environment_calldata: false,
                        returndata: EvmByteArray::empty(),
                        call_value: if is_static { zero() } else { EvmValue::top() },
                        address_operand: None,
                        caller_operand: Some(EvmValue::unknown_address()),
                        saved_store: payload.store.snapshot(),
                    },
                    continuation: EvmContinuation {
                        return_block: Some(0),
                        output_offset: zero(),
                        output_size: zero(),
                        creation: None,
                    },
                });
                callback.normalize();
                result.successors.push(EvmSuccessor {
                    payload: callback,
                    kind: MachineEdgeKind::Call,
                });
            }
        }
        result.payload = payload;
        result
    }
}
