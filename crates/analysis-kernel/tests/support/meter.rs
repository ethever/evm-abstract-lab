//! Reservation counting shared by kernel-owned and public-API regressions.
//! A work refusal rejects the selected payment; cancellation is visible only
//! after that payment succeeds, including at the final commit checkpoint.
use crate::Meter;
use std::{cell::Cell, fmt, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Work,
    Cancelled,
}
impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Stop {}

#[derive(Default)]
pub struct Budget {
    pub used: usize,
    pub calls: usize,
    pub fail_on: Option<usize>,
    pub cancelled: Rc<Cell<bool>>,
    cancel_after: Option<usize>,
}
impl Budget {
    pub fn stop_after(&mut self, reservations: usize, stop: Stop) {
        assert!(reservations > 0);
        self.clear_stop();
        let at = self.calls.checked_add(reservations).unwrap();
        match stop {
            Stop::Work => self.fail_on = Some(at),
            Stop::Cancelled => self.cancel_after = Some(at),
        }
    }
    pub fn clear_stop(&mut self) {
        self.fail_on = None;
        self.cancel_after = None;
    }
}
impl Meter for Budget {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        // Cancellation belongs to the caller's checkpoints. Doing it here
        // would let a missing kernel checkpoint pass the cancellation tests.
        self.calls += 1;
        if self.fail_on == Some(self.calls) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(work).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled.get() || self.cancel_after.is_some_and(|at| self.calls >= at) {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}
