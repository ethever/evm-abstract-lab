//! Extra custom work on the same caller ledger as a prepaid word read.

use crate::{domain::symbolic::ExprError, resource::WorkBudget};
use std::cell::{Cell, RefCell};

/// The original complete-read fee already funds ordinary actions. A stack-owned
/// payer funds extra custom binding, copy and proof work, without another ledger
/// or cache.
pub(in crate::world::bytes) struct BindingBudget<'a> {
    budget: RefCell<&'a mut WorkBudget>,
    exhausted: Cell<bool>,
    cancelled: Cell<bool>,
}

impl<'a> BindingBudget<'a> {
    pub(in crate::world::bytes) fn new(budget: &'a mut WorkBudget) -> Self {
        let exhausted = budget.exhausted();
        Self {
            budget: RefCell::new(budget),
            exhausted: Cell::new(exhausted),
            cancelled: Cell::new(false),
        }
    }

    pub(super) fn check(&self) -> Result<(), ExprError> {
        if analysis_control::current_cancellation().is_cancelled() {
            self.cancelled.set(true);
        }
        if self.cancelled.get() {
            Err(ExprError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub(super) fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    pub(super) fn pay(&self, work: usize) -> bool {
        if self.check().is_err() || self.exhausted() {
            return false;
        }
        // Pack quotes use MAX for an unrepresentable complete fee. Even an
        // otherwise empty unlimited ledger cannot authorize that opaque hook.
        let paid = {
            let mut budget = self.budget.borrow_mut();
            !(work == usize::MAX && budget.used() == 0) && budget.charge(work)
        };
        if !paid {
            self.exhausted.set(true);
        }
        paid && self.check().is_ok()
    }
}
