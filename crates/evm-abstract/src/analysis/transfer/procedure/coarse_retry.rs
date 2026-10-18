//! One persistent whole-entry retention promotion per real Runtime node.
//!
//! The bound lives in Node.payload. This adapter owns only its frozen layout;
//! every later incoming join uses the same erased guarantees and coarse mode.

use super::{
    Domain, Execution, FrameCode, MachineError, MachinePayload, PayloadSummary, WorkBudget,
    instantiate, join, loop_bound, stack_alias::StackAliases,
};

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) enum RuntimeRetry {
    #[default]
    Fresh,
    Attempted,
    Coarse(CoarseRuntime),
}

/// Publication uses one phase's receipts. A failed coarse transfer cannot
/// invalidate legal alternatives or obligations from the precise attempt.
pub(super) fn reconcile(mut first: Execution, mut retry: Execution) -> Execution {
    if retry.frontiers.is_empty() {
        let superseded = first
            .retention_failures
            .iter()
            .map(|(index, _)| *index)
            .collect::<std::collections::BTreeSet<_>>();
        retry.frontiers.extend(
            first
                .frontiers
                .into_iter()
                .enumerate()
                .filter_map(|(index, frontier)| (!superseded.contains(&index)).then_some(frontier)),
        );
        retry.diagnostics.extend(first.diagnostics);
        retry
    } else {
        // Keep the original input/receipts/effects as one complete phase. Only
        // an actual interruption needs an additional original-entry boundary;
        // the original retention frontier already witnesses incompleteness.
        for frontier in retry.frontiers {
            if matches!(
                frontier.reason,
                crate::analysis::FrontierReason::Work
                    | crate::analysis::FrontierReason::Budget(_)
                    | crate::analysis::FrontierReason::Relations(
                        crate::domain::relational::QueryReason::Cancelled
                    )
            ) {
                super::super::boundary(&mut first, frontier.pc, frontier.reason);
            }
        }
        first
    }
}

pub(super) struct CoarseRuntime {
    layout: PayloadSummary,
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), MachineError> {
    MachineError::check_cancelled()?;
    if work == usize::MAX || !budget.charge(work) {
        Err(MachineError::work_failure())
    } else {
        Ok(())
    }
}

fn erase_guarantees(mut input: PayloadSummary) -> Result<PayloadSummary, MachineError> {
    input.storage_observations = Default::default();
    for frame in &mut input.frames {
        frame.stack_aliases = StackAliases::form(&frame.stack, false)?;
    }
    input.work = input.measure_work();
    Ok(input)
}

impl CoarseRuntime {
    pub(super) fn prepare(
        original: &PayloadSummary,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<(Self, PayloadSummary, MachinePayload), MachineError> {
        if original.active().code != FrameCode::Runtime
            || original
                .frames
                .iter()
                .any(|frame| frame.code == FrameCode::Unknown)
        {
            return Err(MachineError::Shape);
        }
        // Pay for cloning, guarantee erasure, layout projection and the full
        // coarse pass before construction. No partial bound escapes refusal.
        reserve(budget, original.work_size().saturating_mul(8))?;
        let projected = erase_guarantees(original.clone())?;
        let layout = loop_bound::restrict(&projected, &projected, domain);
        let bound = loop_bound::coarse(&loop_bound::restrict(&projected, &layout, domain));
        reserve(budget, bound.work_size().saturating_mul(4))?;
        let payload = instantiate(&bound, domain)?;
        MachineError::check_cancelled()?;
        Ok((Self { layout }, bound, payload))
    }

    pub(super) fn join(
        &self,
        old: &PayloadSummary,
        incoming: &PayloadSummary,
        domain: Domain,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<PayloadSummary, MachineError> {
        // The shared summary traversal bound covers both normalization passes,
        // joins, fixed-coordinate projection and the resulting Eq traversal.
        reserve(
            budget,
            self.layout
                .work_size()
                .saturating_add(old.work_size())
                .saturating_add(incoming.work_size())
                .saturating_mul(8),
        )?;
        let incoming = self.normalize(erase_guarantees(incoming.clone())?, domain);
        let joined = join(old, &incoming, domain, widen)?;
        let result = self.normalize(joined, domain);
        MachineError::check_cancelled()?;
        Ok(result)
    }

    fn normalize(&self, input: PayloadSummary, domain: Domain) -> PayloadSummary {
        loop_bound::coarse(&loop_bound::restrict(&input, &self.layout, domain))
    }
}
