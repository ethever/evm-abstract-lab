//! Kernel-owned regressions; external acceptance tests have separate ownership.

mod admission;
mod entry_lifecycle;
mod generations;
mod refinement;
mod reporting;
mod snapshot;
mod suspension;

use crate::{Kernel, Meter, Scope, ScopeKey};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
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
struct Budget {
    used: usize,
    calls: usize,
    fail_on: Option<usize>,
    cancelled: bool,
}
impl Meter for Budget {
    type Stop = Stop;
    fn reserve(&mut self, work: usize) -> Result<(), Stop> {
        self.checkpoint()?;
        self.calls += 1;
        if self.fail_on == Some(self.calls) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(work).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

fn scope<J>(kernel: &mut Kernel<J>, meter: &mut Budget, context: u64, generation: u64) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context,
                generation,
                policy: 1,
            },
            meter,
        )
        .unwrap()
}
