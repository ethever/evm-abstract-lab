//! Local Process rules on their paid numerical carrier, in the same Body task.
//!
//! Primitive calls/creation keep the existing adapter and subscriber machinery.
//! Enabled foreign compositions keep every admitted block/input hook. Neither
//! path constructs a second solver or interprets a Process location as Native PC.

use super::{Composition, EvmSolver, MachineError};
use crate::{
    analysis::{
        EvmProcessTransfer, MachineEdgeKind, OutcomeKind, ProcessLocation, ProcessOwner,
        engine::{ContextPolicy, PartitionPolicy, PolicyContext, ProcedureOutput, StatePolicy},
        state::EvmPayloadSummary,
    },
    domain::EvmNumericAlgebra,
    world::EvmByteArraySummary,
};
use std::sync::Arc;

impl<D, C, P, S, X, E> EvmSolver<'_, D, C, P, S, X, E>
where
    D: EvmNumericAlgebra,
    C: ContextPolicy,
    P: PartitionPolicy,
    S: StatePolicy<D, P::Class>,
    X: Composition<D::Element, Error = E>,
{
    /// None means this Body completed on its numerical carrier. Some returns
    /// the original owned input for a primitive/unsupported-shape adapter path.
    pub(in crate::analysis::transfer::procedure) fn process_compact(
        &mut self,
        id: usize,
        mut input: EvmPayloadSummary<D::Element>,
    ) -> Result<Option<EvmPayloadSummary<D::Element>>, MachineError> {
        self.reserve(32)?;
        input.check_shape()?;
        if input.location.frames.len() != 1
            || input.active().key.process.is_none()
            || input.location.frames[0].process != input.active().key.process
        {
            return Ok(Some(input));
        }
        let plan = self.process_step(id, &input.active().key, input.active().program.as_ref())?;
        if plan.capabilities.actions().next().is_some() {
            return Ok(Some(input));
        }
        self.counters.transfers += 1;
        self.counters.report(self.budget.used());
        let version = self.track_input(id)?;
        self.reserve(32)?;
        let outcomes = [
            OutcomeKind::Return,
            OutcomeKind::Revert,
            OutcomeKind::Failure,
        ]
        .into_iter()
        .filter(|kind| match kind {
            OutcomeKind::Return => plan.capabilities.returns,
            OutcomeKind::Revert => plan.capabilities.reverts,
            OutcomeKind::Failure => true,
        })
        .collect::<Vec<_>>();
        self.regional_compact_transfer(
            id,
            version,
            EvmProcessTransfer {
                control: plan.control.as_ref().map(Arc::clone),
                repeat: plan.location == ProcessLocation::Any,
                actions: Vec::new(),
                creations: Vec::new(),
                unresolved: Vec::new(),
            },
            Vec::new(),
        )?;
        self.producer = Some(super::super::Producer {
            kind: 0,
            id,
            case: 0,
            input: version,
            other: None,
            parent: self.consumed,
        });
        let frontiers = self.table.frontiers.len();
        if plan.capabilities.effects {
            let catalogue = Arc::clone(
                self.table
                    .process_catalogue
                    .as_ref()
                    .ok_or(MachineError::Shape)?,
            );
            let owner = input
                .active()
                .key
                .process
                .as_ref()
                .ok_or(MachineError::Shape)?
                .owner;
            let writable = match owner {
                ProcessOwner::Named(owner) => Some(owner),
                ProcessOwner::RootNamespace => {
                    catalogue.namespace().map(|namespace| namespace.coordinate)
                }
                ProcessOwner::Residual => None,
            };
            input.store = input.store.process_effects_with_payment(
                catalogue.coordinates(),
                writable,
                self.entry.environment.to.as_concrete().is_none(),
                input.active().key.is_static || plan.capabilities.readonly,
                self.domain,
                self.budget,
            )?;
        }
        let procedure = self.table.nodes[id].procedure;
        for kind in outcomes {
            let data = if kind == OutcomeKind::Failure {
                EvmByteArraySummary::empty_with_payment(false, self.domain, self.budget)?
            } else {
                EvmByteArraySummary::unknown_with_payment(self.domain, self.budget)?
            };
            let store = if kind == OutcomeKind::Return {
                input.store.clone_with_payment(self.budget)?
            } else {
                input
                    .store
                    .process_restore_with_payment(&input.active().saved_store, self.budget)?
            };
            let output = ProcedureOutput::from_summaries(
                data,
                store,
                &mut PolicyContext::new(self.domain, self.entry, self.budget),
            )?;
            self.reserve(2)?;
            self.table.nodes[id]
                .transfer
                .as_mut()
                .ok_or(MachineError::Shape)?
                .outcomes
                .push(kind);
            self.publish_summary(procedure, kind, output)?;
        }
        self.reserve(
            input
                .work_size()
                .saturating_add(input.store.process_work_size()),
        )?;
        input.work = input.measure_work();
        self.reserve(
            plan.control
                .as_ref()
                .map_or(2, |control| control.boundaries.len())
                .saturating_mul(8)
                .saturating_add(8),
        )?;
        let successors = match &plan.control {
            Some(control) => control
                .boundaries
                .iter()
                .map(|location| (*location, MachineEdgeKind::ProcessControl))
                .collect::<Vec<_>>(),
            None => plan.step.as_ref().map_or_else(Vec::new, |step| {
                step.successors.into_iter().flatten().collect()
            }),
        };
        for (location, kind) in successors {
            self.reserve(4)?;
            let mut next = input.clone_with_payment(self.budget)?;
            next.frames[0]
                .key
                .process
                .as_mut()
                .ok_or(MachineError::Shape)?
                .location = location;
            next.location.frames[0]
                .process
                .as_mut()
                .ok_or(MachineError::Shape)?
                .location = location;
            let to = self.enqueue_summary(procedure, next, None, Some(id))?;
            self.observe_body_edge(id, to, kind)?;
        }
        self.regional_transfer_complete(
            id,
            !self.deferred && self.table.frontiers.len() == frontiers,
        );
        Ok(None)
    }
}
