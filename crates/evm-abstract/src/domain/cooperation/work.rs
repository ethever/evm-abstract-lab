//! Bounded spending of one caller-prepaid closure, with captured cancellation.

use super::{KernelError, Meter, ReductionStatus, WorkExhausted};

/// A reservation carved from the caller's already-paid numeric operation.
/// Kernel overhead and all component rules share it; no root budget is reset.
pub struct Precharged {
    remaining: usize,
    exhausted: bool,
    cancellation: analysis_control::Cancellation,
}
impl Precharged {
    pub(super) fn new(work: usize) -> Self {
        Self {
            remaining: work,
            exhausted: false,
            cancellation: analysis_control::current_cancellation(),
        }
    }
    #[cfg(test)]
    pub(super) fn remaining(&self) -> usize {
        self.remaining
    }
    pub(super) fn failure_status(&self, error: &KernelError<WorkExhausted>) -> ReductionStatus {
        match error {
            KernelError::Stopped(_) if self.cancellation.is_cancelled() => {
                ReductionStatus::Cancelled
            }
            KernelError::Stopped(_) | KernelError::CostOverflow | KernelError::VersionExhausted => {
                ReductionStatus::WorkLimit
            }
            _ => ReductionStatus::KernelFailure,
        }
    }
}
impl Meter for Precharged {
    type Stop = WorkExhausted;
    fn reserve(&mut self, amount: usize) -> Result<(), WorkExhausted> {
        self.checkpoint()?;
        let Some(remaining) = self
            .remaining
            .checked_sub(amount)
            .filter(|_| amount != usize::MAX)
        else {
            // A rule cannot erase a refusal by catching its charge error.
            self.exhausted = true;
            return Err(WorkExhausted);
        };
        self.remaining = remaining;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), WorkExhausted> {
        if self.exhausted || self.cancellation.is_cancelled() {
            Err(WorkExhausted)
        } else {
            Ok(())
        }
    }
}
