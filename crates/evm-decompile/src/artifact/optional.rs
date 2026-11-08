//! Soft optional admission over the original cumulative ledger. A refused local
//! request never calls WorkBudget::charge, so the paid base transaction survives.
use super::Builder;
use crate::Progress;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum OptionalStop {
    #[error("optional work allowance refused (used {used}, requested {requested}, limit {limit})")]
    Work {
        used: usize,
        requested: usize,
        limit: usize,
    },
    #[error("optional publication history refused (required {required}, limit {limit})")]
    History { required: u64, limit: u64 },
    #[error("the mandatory owner stopped")]
    Stopped,
}

pub(super) struct Allowance {
    limit: usize,
    used: usize,
    refusal: Option<OptionalStop>,
}
impl Allowance {
    pub(super) fn admits(&mut self, work: usize) -> bool {
        if self.refusal.is_some() {
            return false;
        }
        if work > self.limit.saturating_sub(self.used) {
            self.refusal = Some(OptionalStop::Work {
                used: self.used,
                requested: work,
                limit: self.limit,
            });
            return false;
        }
        self.used += work;
        true
    }
    pub(super) fn refuse(&mut self, reason: OptionalStop) {
        self.refusal.get_or_insert(reason);
    }
}
impl<P: FnMut(Progress)> Builder<P> {
    /// Every admitted operation pays the existing ledger. The local allowance
    /// is bounded by its remaining balance and is not a second source of work.
    pub fn optional<T>(
        &mut self,
        allowance: usize,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, OptionalStop> {
        if !self.pay(0) {
            return Err(OptionalStop::Stopped);
        }
        assert!(
            self.optional.is_none(),
            "optional publication scopes cannot nest"
        );
        self.optional = Some(Allowance {
            limit: allowance.min(self.budget.remaining()),
            used: 0,
            refusal: None,
        });
        let value = operation(self);
        let local = self.optional.take().expect("owned optional scope");
        if !self.pay(0) {
            return Err(OptionalStop::Stopped);
        }
        if let Some(reason) = local.refusal {
            return Err(reason);
        }
        Ok(value)
    }
}
