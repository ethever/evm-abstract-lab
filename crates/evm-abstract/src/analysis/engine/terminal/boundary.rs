//! Track actual detail stops as they occur; publication never scans an unpaid
//! finished graph or reconstructs finite failure identity from public reasons.

use crate::domain::WordElement;

use crate::analysis::{
    FrontierReason, Limit, RootTerminalRetention, RootTerminalTrigger,
    transfer::{EvmExecution, RetentionCause},
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default)]
pub(in crate::analysis::engine) struct Boundary {
    enabled: bool,
    trigger: Option<RootTerminalTrigger>,
    blocked: bool,
}

impl Boundary {
    pub(in crate::analysis::engine) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// The caller supplies actual ledger/counter observations at the failure.
    pub(in crate::analysis::engine) fn observe(
        &mut self,
        reason: &FrontierReason,
        work_refused: bool,
        states_reached: bool,
        transfers_reached: bool,
    ) {
        if !self.enabled {
            return;
        }
        // Acquisition failure and cancellation cannot be reclassified as
        // semantic terminal coverage even if another branch later meets Work.
        if matches!(
            reason,
            FrontierReason::RpcAcquisition { .. }
                | FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
        ) {
            self.blocked = true;
        }
        if self.trigger.is_some() {
            return;
        }
        self.trigger = match reason {
            FrontierReason::Work | FrontierReason::Budget(Limit::Work) if work_refused => {
                Some(RootTerminalTrigger::Work)
            }
            FrontierReason::SummaryWork if work_refused => Some(RootTerminalTrigger::SummaryWork),
            FrontierReason::Budget(Limit::States) if states_reached => {
                Some(RootTerminalTrigger::States)
            }
            FrontierReason::Budget(Limit::Transfers) if transfers_reached => {
                Some(RootTerminalTrigger::Transfers)
            }
            _ => None,
        };
    }

    /// Raw finite retention is already classified by checked transfer errors.
    /// execute() has completed its local retries before returning this record.
    pub(in crate::analysis::engine) fn retention<N: WordElement>(
        &mut self,
        state: usize,
        execution: &EvmExecution<N>,
    ) {
        if !self.enabled || self.trigger.is_some() {
            return;
        }
        if let Some((index, cause)) = execution.retention_failures.first()
            && let Some(frontier) = execution.frontiers.get(*index)
        {
            self.trigger = Some(RootTerminalTrigger::MandatoryRetention {
                state,
                pc: frontier.pc,
                kind: match cause {
                    RetentionCause::Expression => RootTerminalRetention::Expression,
                    RetentionCause::State => RootTerminalRetention::State,
                },
            });
        }
    }

    pub(in crate::analysis::engine) fn into_trigger(self) -> Option<RootTerminalTrigger> {
        if self.blocked { None } else { self.trigger }
    }
}
