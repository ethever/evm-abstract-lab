//! Only semantic query vocabulary is shared by the two independent rules.
use super::support::Conflict;
use analysis_kernel::{Capability, RefinePolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Variable {
    X,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub lo: u8,
    pub hi: u8,
}
impl Interval {
    pub const fn new(lo: u8, hi: u8) -> Self {
        Self { lo, hi }
    }
    pub fn contains(self, word: u8) -> bool {
        self.lo <= word && word <= self.hi
    }
}

pub struct Bounds;
impl Capability for Bounds {
    type Key = Variable;
    type Value = Interval;
    fn key_work(_: &Variable) -> usize {
        1
    }
    fn value_work(_: &Interval) -> usize {
        2
    }
}

pub struct Candidates;
impl Capability for Candidates {
    type Key = Variable;
    type Value = u16;
    fn key_work(_: &Variable) -> usize {
        1
    }
    fn value_work(_: &u16) -> usize {
        1
    }
}

pub struct IntersectIntervals;
impl RefinePolicy for IntersectIntervals {
    type Value = Interval;
    type Error = Conflict;
    fn work(&self, _: &Interval, _: &Interval) -> usize {
        4
    }
    fn refine(&self, a: &Interval, b: &Interval) -> Result<Interval, Conflict> {
        let value = Interval::new(a.lo.max(b.lo), a.hi.min(b.hi));
        if value.lo > value.hi {
            Err(Conflict)
        } else {
            Ok(value)
        }
    }
    fn equivalent(&self, a: &Interval, b: &Interval) -> bool {
        a == b
    }
}

pub struct IntersectCandidates;
impl RefinePolicy for IntersectCandidates {
    type Value = u16;
    type Error = Conflict;
    fn work(&self, _: &u16, _: &u16) -> usize {
        2
    }
    fn refine(&self, a: &u16, b: &u16) -> Result<u16, Conflict> {
        let value = a & b;
        if value == 0 { Err(Conflict) } else { Ok(value) }
    }
    fn equivalent(&self, a: &u16, b: &u16) -> bool {
        a == b
    }
}
