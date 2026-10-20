use analysis_kernel::{
    Capability, JoinPolicy, Kernel, KernelError, Meter, PublishError, Scope, ScopeKey,
};
use std::{error::Error, fmt};

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
impl Error for Stop {}

#[derive(Default)]
pub struct Ledger {
    pub used: usize,
    pub calls: usize,
    pub deny_next: bool,
    pub deny_call: Option<usize>,
    pub cancelled: bool,
}
impl Meter for Ledger {
    type Stop = Stop;
    fn reserve(&mut self, cost: usize) -> Result<(), Stop> {
        self.calls += 1;
        if std::mem::take(&mut self.deny_next) || self.deny_call == Some(self.calls) {
            return Err(Stop::Work);
        }
        self.used = self.used.checked_add(cost).expect("test ledger overflow");
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conflict;
impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("incompatible finite guarantees")
    }
}
impl Error for Conflict {}

#[derive(Debug)]
pub enum RuleError {
    Kernel(KernelError<Stop>),
    Publication(PublishError<Conflict, Stop>),
}
impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(error) => error.fmt(f),
            Self::Publication(error) => error.fmt(f),
        }
    }
}
impl Error for RuleError {}
impl From<KernelError<Stop>> for RuleError {
    fn from(error: KernelError<Stop>) -> Self {
        Self::Kernel(error)
    }
}
impl From<PublishError<Conflict, Stop>> for RuleError {
    fn from(error: PublishError<Conflict, Stop>) -> Self {
        Self::Publication(error)
    }
}

pub fn scope<J>(
    kernel: &mut Kernel<J>,
    ledger: &mut Ledger,
    context: u64,
    generation: u64,
) -> Scope {
    kernel
        .scope(
            ScopeKey {
                context,
                generation,
                policy: 1,
            },
            ledger,
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
pub struct Maximum;
impl JoinPolicy for Maximum {
    type Value = u32;
    type Error = Conflict;
    fn work(&self, _: &u32, _: &u32) -> usize {
        3
    }
    fn join(&self, a: &u32, b: &u32) -> Result<u32, Conflict> {
        Ok((*a).max(*b))
    }
    fn equivalent(&self, a: &u32, b: &u32) -> bool {
        a == b
    }
}
