//! Input contribution and body views for the shared production cooperation ports.
//!
//! Raw successor scopes preserve computed-expression relationships before
//! numerical template projection. Body scopes describe the retained ascending
//! bound. Neither view can replace a live anchor with a narrowed value.

use super::{
    Composition, ContextPolicy, DiagnosticKind, EvmMachinePayload, EvmSolver, MachineEdgeKind,
    MachineError, PartitionPolicy,
};
use crate::{analysis::worklist::kernel_failure, domain::EvmNumericAlgebra};
use analysis_kernel::Version;

#[derive(Clone, Copy)]
pub(super) struct Producer {
    pub(super) kind: u8,
    pub(super) id: usize,
    pub(super) case: u8,
    pub(super) input: Version,
    pub(super) other: Option<Version>,
}

impl<D: EvmNumericAlgebra, C: ContextPolicy, P: PartitionPolicy, X: Composition<D::Element, E>, E>
    EvmSolver<'_, D, C, P, X, E>
{
    pub(super) fn cooperative_input(
        &mut self,
        id: usize,
        payload: EvmMachinePayload<D::Element>,
        phase: u8,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        if !X::ENABLED {
            return Ok(Some(payload));
        }
        let slot = &self.table.nodes[id].input;
        let version = self.runtime.with_run(
            self.active.as_mut().unwrap_or(self.caller),
            self.budget,
            |run| run.slot_version(slot).map_err(kernel_failure),
        )?;
        let view = self
            .cooperation
            .input(
                crate::analysis::cooperation::Point {
                    namespace: 1,
                    kind: crate::analysis::cooperation::PointKind::Body,
                    index: id,
                },
                version,
                phase,
                None,
                payload,
                self.domain,
                self.runtime,
                self.active.as_mut().unwrap_or(self.caller),
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        match view {
            crate::analysis::cooperation::LocalInput::Unreachable => {
                self.table.nodes[id].excluded = true;
                Ok(None)
            }
            crate::analysis::cooperation::LocalInput::Reachable { payload, limited } => {
                self.reserve(limited.len().saturating_mul(16))?;
                self.table
                    .precision_losses
                    .extend(limited.into_iter().map(DiagnosticKind::FactExchangeLimited));
                self.table.nodes[id].excluded = false;
                Ok(Some(*payload))
            }
        }
    }

    pub(super) fn track_input(&mut self, id: usize) -> Result<Version, MachineError> {
        let node = &self.table.nodes[id];
        let answer = self.runtime.with_run(
            self.active.as_mut().unwrap_or(self.caller),
            self.budget,
            |run| {
                run.read_slot(&node.input, &node.payload, node.payload.work_size())
                    .map_err(kernel_failure)
            },
        )?;
        Ok(answer.version)
    }

    pub(super) fn incoming(
        &mut self,
        payload: EvmMachinePayload<D::Element>,
        kind: MachineEdgeKind,
        ordinal: usize,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        if !X::ENABLED {
            return Ok(Some(payload));
        }
        let source = self.producer.ok_or(MachineError::Shape)?;
        self.reserve(payload.work_size().saturating_mul(2))?;
        let key_work = payload.call_stack.iter().fold(1usize, |work, frame| {
            work.saturating_add(frame.key.jump_history.len().saturating_add(32))
        });
        self.reserve(
            key_work
                .saturating_mul((usize::BITS as usize) + 1)
                .saturating_mul(4),
        )?;
        let key = (
            source.kind,
            source.id,
            source.case,
            payload.key(),
            kind,
            ordinal,
        );
        let next = self.table.exchange_inputs.len();
        let index = *self.table.exchange_inputs.entry(key).or_insert(next);
        let view = self
            .cooperation
            .input(
                crate::analysis::cooperation::Point {
                    namespace: 2,
                    kind: crate::analysis::cooperation::PointKind::Body,
                    index,
                },
                source.input,
                0,
                source.other,
                payload,
                self.domain,
                self.runtime,
                self.active.as_mut().unwrap_or(self.caller),
                self.budget,
            )
            .map_err(|error| self.cooperation.stop(error, self.budget))?;
        match view {
            crate::analysis::cooperation::LocalInput::Unreachable => Ok(None),
            crate::analysis::cooperation::LocalInput::Reachable { payload, limited } => {
                self.reserve(limited.len().saturating_mul(16))?;
                self.table
                    .precision_losses
                    .extend(limited.into_iter().map(DiagnosticKind::FactExchangeLimited));
                Ok(Some(*payload))
            }
        }
    }
}
