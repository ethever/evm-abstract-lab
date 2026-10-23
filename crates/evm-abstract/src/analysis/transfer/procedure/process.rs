//! Whole-runtime processes executed by the native Body/Resume relation table.
//!
//! Private stack/memory are quotiented out under an explicit interpretation.
//! Every primitive action has a typed receipt; unresolved targets/allocation
//! remain frontiers. Repeating the envelope and its callback continuations is
//! the existing table fixed point, never recursive frame unrolling.

use super::Composition;
use super::{
    EvmByteArray, EvmExecution, EvmMachinePayload, EvmSolver, FrontierReason, MachineError,
    OutcomeKind, TransferContext, calls as completion,
};
use crate::{
    analysis::engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    analysis::{EvmProcessTransfer, ProcessAction},
    domain::{EvmNumericAlgebra, SummaryConstruction},
};

mod admission;
mod allocation;
mod calls;
mod capabilities;
pub mod creation;
pub(super) use capabilities::Capabilities;

impl<
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
    E,
> EvmSolver<'_, D, C, P, S, X, E>
{
    pub(super) fn process(
        &mut self,
        id: usize,
        mut payload: EvmMachinePayload<D::Element>,
    ) -> EvmExecution<D::Element> {
        let execution = self.process_inner(id, &mut payload);
        match execution {
            Ok(result) => result,
            Err(reason) => {
                let mut result = super::super::empty_execution(payload);
                // Refusal is retained, never retried as a successful envelope.
                super::super::boundary(&mut result, 0, reason);
                result
            }
        }
    }

    fn process_inner(
        &mut self,
        id: usize,
        payload: &mut EvmMachinePayload<D::Element>,
    ) -> Result<EvmExecution<D::Element>, FrontierReason> {
        let capabilities = self.table.procedures[self.table.nodes[id].procedure]
            .process
            .expect("process admission retains capabilities");
        payload.pay_clone_with(self.budget)?;
        let mut result = super::super::empty_execution(payload.clone());
        self.reserve(64).map_err(MachineError::frontier_reason)?;
        let mut receipt = EvmProcessTransfer {
            repeat: true,
            actions: capabilities.actions().collect(),
            creations: Vec::new(),
            unresolved: Vec::new(),
        };
        let catalogue = std::sync::Arc::clone(
            self.table
                .process_catalogue
                .as_ref()
                .ok_or_else(|| MachineError::Shape.frontier_reason())?,
        );
        let owner = payload
            .active()
            .key
            .process
            .as_ref()
            .ok_or_else(|| MachineError::Shape.frontier_reason())?
            .owner;
        let writable = match owner {
            crate::analysis::ProcessOwner::Named(owner) => Some(owner),
            crate::analysis::ProcessOwner::Residual => None,
        };
        if capabilities.effects {
            payload.store = payload
                .store
                .process_effects_with_payment(
                    catalogue.named(),
                    writable,
                    payload.active().key.is_static,
                    self.domain,
                    self.budget,
                )
                .map_err(|error| match error {
                    crate::resource::SummaryError::Budget(error) => FrontierReason::from(error),
                    crate::resource::SummaryError::Source(error) => {
                        MachineError::Store(error).frontier_reason()
                    }
                })?;
        }
        // Entered-frame exceptional completion covers finite gas exhaustion,
        // including positive-cost pure loops. It uses actual rollback/sticky
        // handling rather than Work exhaustion or the root terminal envelope.
        for kind in [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ] {
            if (kind == OutcomeKind::Return && !capabilities.returns)
                || (kind == OutcomeKind::Revert && !capabilities.reverts)
            {
                continue;
            }
            payload.pay_clone_with(self.budget)?;
            let data = if kind == OutcomeKind::Failure {
                self.pay_process_arrays()
                    .map_err(MachineError::frontier_reason)?;
                EvmByteArray::empty()
            } else {
                self.pay_process_arrays()
                    .map_err(MachineError::frontier_reason)?;
                EvmByteArray::unknown_checked()
                    .map_err(|e| MachineError::Bytes(e).frontier_reason())?
            };
            let mut cx = TransferContext {
                config: self.config,
                domain: self.domain,
                budget: self.budget,
                root_address: Some((self.entry.address, self.entry.environment.to)),
                input_scope: self.entry.environment.input_scope.id(),
                query_cache: Default::default(),
                procedure: true,
                discover_code: false,
            };
            completion::finish(&mut result, payload.clone(), kind, data, &mut cx, 0);
        }
        for action in capabilities.actions() {
            if matches!(action, ProcessAction::Create | ProcessAction::Create2) {
                receipt.creations.push(
                    self.process_allocations(payload, action, &mut result)
                        .map_err(MachineError::frontier_reason)?,
                );
                continue;
            }
            self.process_calls(payload, action, &mut result)?;
        }
        self.reserve(
            receipt
                .actions
                .len()
                .saturating_add(receipt.unresolved.len())
                .saturating_add(8),
        )
        .map_err(MachineError::frontier_reason)?;
        result.process = Some(receipt);
        payload.pay_clone_with(self.budget)?;
        result.payload = payload.clone();
        Ok(result)
    }

    fn pay_process_arrays(&mut self) -> Result<(), MachineError> {
        // Covers memory/empty arrays, one independent unknown calldata and its
        // metadata sources. Scalar invocation values are paid independently.
        let numeric = self.domain.numeric_algebra();
        let work = numeric
            .summary_construction_work(SummaryConstruction::Top)
            .saturating_mul(2)
            .saturating_add(numeric.summary_construction_work(SummaryConstruction::UnknownByte))
            .saturating_add(
                self.domain
                    .literal_quote_work(alloy_primitives::U256::ZERO)
                    .saturating_mul(8),
            )
            .saturating_add(128);
        self.reserve(work)
    }
}
