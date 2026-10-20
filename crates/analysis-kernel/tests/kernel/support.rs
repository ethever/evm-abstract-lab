use analysis_kernel::{Capability, JoinPolicy, Kernel, Meter, Scope, ScopeKey};
use std::{cell::Cell, convert::Infallible, fmt, rc::Rc};

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

pub struct Budget {
    pub used: usize,
    pub calls: usize,
    pub fail_on: Option<usize>,
    pub cancelled: Rc<Cell<bool>>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            used: 0,
            calls: 0,
            fail_on: None,
            cancelled: Rc::new(Cell::new(false)),
        }
    }
}
impl Meter for Budget {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        self.checkpoint()?;
        self.calls += 1;
        if self.fail_on == Some(self.calls) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(cost).ok_or(Stop::Work)?;
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), Stop> {
        if self.cancelled.get() {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub fn scope<J>(kernel: &mut Kernel<J>, budget: &mut Budget, generation: u64) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context: 3,
                generation,
                policy: 1,
            },
            budget,
        )
        .unwrap()
}

pub struct Numbers;
impl Capability for Numbers {
    type Key = u8;
    type Value = u32;
    fn key_work(_: &u8) -> usize {
        1
    }
    fn value_work(_: &u32) -> usize {
        1
    }
}
#[derive(Clone, Copy)]
pub struct Maximum;
impl JoinPolicy for Maximum {
    type Value = u32;
    type Error = Infallible;
    fn work(&self, _: &u32, _: &u32) -> usize {
        3
    }
    fn join(&self, a: &u32, b: &u32) -> Result<u32, Infallible> {
        Ok((*a).max(*b))
    }
    fn equivalent(&self, a: &u32, b: &u32) -> bool {
        a == b
    }
}
