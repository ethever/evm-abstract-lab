//! Covering summary admission through the language-independent kernel protocol.

use super::{EvmPayloadSummary, MachineError, join_with, semantic_eq};
use crate::domain::{AlgebraRef, EvmNumericAlgebra};
use analysis_kernel::{
    Change,
    algebra::{AdmissionPolicy, Prepared},
};

pub(super) struct SummaryAdmission<'a, D: EvmNumericAlgebra> {
    pub domain: AlgebraRef<'a, D>,
    pub widen: bool,
}

impl<D: EvmNumericAlgebra> AdmissionPolicy for SummaryAdmission<'_, D> {
    type Value = EvmPayloadSummary<D::Element>;
    type Metadata = usize;
    type Error = MachineError;

    fn work(&self, current: &Self::Value, incoming: &Self::Value, _: &usize) -> usize {
        current
            .work_size()
            .saturating_add(incoming.work_size())
            .saturating_mul(4)
    }

    fn prepare(
        &self,
        current: &Self::Value,
        incoming: &Self::Value,
        updates: &usize,
    ) -> Result<Prepared<Self::Value, usize>, MachineError> {
        let value = join_with(current, incoming, self.domain, self.widen)?;
        let change = if semantic_eq(current, &value) {
            Change::Unchanged
        } else {
            Change::Changed
        };
        let metadata = if change == Change::Changed {
            updates
                .checked_add(1)
                .ok_or(MachineError::IdentityExhausted)?
        } else {
            *updates
        };
        let value = if change == Change::Unchanged {
            current.clone()
        } else {
            value
        };
        let install_work = value.work_size().saturating_add(1);
        Ok(Prepared {
            value,
            metadata,
            change,
            install_work,
        })
    }
}
