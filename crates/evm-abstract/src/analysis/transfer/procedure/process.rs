//! Source-qualified MAY control in the native Body/Resume relation table.
//!
//! Private stack/memory are quotiented out under an explicit interpretation.
//! Every primitive action has a typed receipt; unresolved targets/allocation
//! remain frontiers. Repeating the envelope and its callback continuations is
//! the existing table fixed point, never recursive frame unrolling.

use super::Composition;
use super::{
    EvmByteArray, EvmExecution, EvmMachinePayload, EvmSolver, EvmSuccessor, FrontierReason,
    MachineError, OutcomeKind, TransferContext, calls as completion,
};
use crate::{
    analysis::engine::{ContextPolicy, PartitionPolicy, StatePolicy},
    analysis::{EdgeKind, EvmProcessTransfer, MachineEdgeKind, ProcessAction, ProcessLocation},
    domain::{EvmNumericAlgebra, SummaryConstruction},
};

mod admission;
mod allocation;
mod calls;
mod capabilities;
mod control;
pub(in crate::analysis::transfer::procedure) use control::Cache as ControlCache;
pub mod creation;
pub(super) use admission::export_store;
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
        let boundary = self.table.procedures[self.table.nodes[id].procedure]
            .process
            .expect("process admission retains capabilities");
        let location = payload
            .active()
            .key
            .process
            .as_ref()
            .ok_or_else(|| MachineError::Shape.frontier_reason())?
            .location;
        let plan = if !X::ENABLED
            && matches!(
                location,
                ProcessLocation::Block(_) | ProcessLocation::JumpTargets
            )
            && payload
                .active()
                .program
                .as_ref()
                .is_some_and(|program| !program.blocks().is_empty())
        {
            Some(
                self.table
                    .process_control
                    .plan(
                        payload.active().program.as_ref().expect("captured source"),
                        payload
                            .active()
                            .key
                            .process
                            .as_ref()
                            .expect("process source"),
                        self.budget,
                    )
                    .map_err(MachineError::frontier_reason)?,
            )
        } else {
            None
        };
        let (step, control) = match plan {
            Some(control::Plan::Boundary(step)) => (Some(step), None),
            Some(control::Plan::Region(region)) => (None, Some(region)),
            None => (
                match location {
                    ProcessLocation::Block(index) => Some(
                        control::Step::at(
                            payload
                                .active()
                                .program
                                .as_ref()
                                .ok_or_else(|| MachineError::Shape.frontier_reason())?,
                            index,
                            self.budget,
                        )
                        .map_err(MachineError::frontier_reason)?,
                    ),
                    _ => None,
                },
                None,
            ),
        };
        let capabilities = match location {
            _ if control.is_some() => Capabilities::empty(),
            ProcessLocation::Block(_) => step.as_ref().expect("block step").capabilities,
            ProcessLocation::JumpTargets => Capabilities::empty(),
            ProcessLocation::End => Capabilities::boundary(super::FrameCode::Empty),
            ProcessLocation::Any => boundary,
        };
        payload.pay_clone_with(self.budget)?;
        let mut result = super::super::empty_execution(payload.clone());
        self.reserve(64).map_err(MachineError::frontier_reason)?;
        let mut receipt = EvmProcessTransfer {
            control: control.as_ref().map(std::sync::Arc::clone),
            repeat: location == ProcessLocation::Any,
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
            crate::analysis::ProcessOwner::RootNamespace => {
                catalogue.namespace().map(|n| n.coordinate)
            }
            crate::analysis::ProcessOwner::Residual => None,
        };
        if capabilities.effects {
            payload.store = payload
                .store
                .process_effects_with_payment(
                    catalogue.coordinates(),
                    writable,
                    self.entry.environment.to.as_concrete().is_none(),
                    payload.active().key.is_static || capabilities.readonly,
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
                EvmByteArray::empty_with_payment(self.domain, self.budget)?
            } else {
                self.pay_process_return_data()
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
        // Call subscribers retain the parent at its following MAY block. Child
        // output delivery already restores this exact template and checkpoint;
        // no Native return cursor or result stack is consulted.
        if let Some(continuation) = step.as_ref().and_then(|step| step.continuation) {
            payload
                .active_mut()
                .key
                .process
                .as_mut()
                .expect("process frame")
                .location = continuation;
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
            // Depth/funding rejection and immediate completion may continue
            // without an entered child. Private CALL status/data are erased.
            self.process_successor(
                payload,
                location_of(payload)?,
                MachineEdgeKind::Process(action),
                &mut result,
            )?;
        }
        if let Some(control) = &control {
            for location in &control.boundaries {
                self.process_successor(
                    payload,
                    *location,
                    MachineEdgeKind::ProcessControl,
                    &mut result,
                )?;
            }
        } else if let Some(step) = &step {
            for (location, kind) in step.successors.into_iter().flatten() {
                self.process_successor(payload, location, kind, &mut result)?;
            }
        } else if location == ProcessLocation::JumpTargets {
            let program = payload
                .active()
                .program
                .clone()
                .ok_or_else(|| MachineError::Shape.frontier_reason())?;
            self.reserve(program.jumpdest_blocks().len().saturating_add(1))
                .map_err(MachineError::frontier_reason)?;
            // All unknown origins share this source/owner-qualified union.
            // The relation has O(jumps + legal targets) edges, not their product.
            for index in program.jumpdest_blocks().values() {
                self.process_successor(
                    payload,
                    ProcessLocation::Block(*index),
                    MachineEdgeKind::Intraprocedural(EdgeKind::Jump),
                    &mut result,
                )?;
            }
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

    fn process_successor(
        &mut self,
        payload: &EvmMachinePayload<D::Element>,
        location: ProcessLocation,
        kind: MachineEdgeKind,
        result: &mut EvmExecution<D::Element>,
    ) -> Result<(), FrontierReason> {
        payload.pay_clone_with(self.budget)?;
        let mut successor = payload.clone();
        successor
            .active_mut()
            .key
            .process
            .as_mut()
            .ok_or_else(|| MachineError::Shape.frontier_reason())?
            .location = location;
        result.successors.push(EvmSuccessor {
            payload: successor,
            kind,
        });
        Ok(())
    }

    fn pay_process_return_data(&mut self) -> Result<(), MachineError> {
        // The unknown public output really needs an independent source. Private
        // adapter arrays instead use the existing paid canonical constructors.
        let numeric = self.domain.numeric_algebra();
        let work = numeric
            .summary_construction_work(SummaryConstruction::Top)
            .saturating_mul(2)
            .saturating_add(numeric.summary_construction_work(SummaryConstruction::UnknownByte))
            .saturating_add(128);
        self.reserve(work)
    }
}

fn location_of<N: crate::domain::WordElement>(
    payload: &EvmMachinePayload<N>,
) -> Result<ProcessLocation, FrontierReason> {
    payload
        .active()
        .key
        .process
        .as_ref()
        .map(|key| key.location)
        .ok_or_else(|| MachineError::Shape.frontier_reason())
}
