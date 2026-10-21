//! All language, domain and capability choices belong to this client.
use analysis_kernel::{Capability, Either, JoinPolicy, PublishError, Reachable, Scope, Scoped};
use std::convert::Infallible;

pub const MAX: u8 = 7;
pub const BOTH: u8 = 0b11;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pc {
    Entry,
    Head,
    Step,
    Return,
    Fault,
}
pub const POINTS: [Pc; 5] = [Pc::Entry, Pc::Head, Pc::Step, Pc::Return, Pc::Fault];

#[derive(Clone, Copy)]
pub enum Instruction {
    Equal { value: u8, yes: Pc, no: Pc },
    Less { limit: u8, yes: Pc, no: Pc },
    Add { amount: u8, next: Pc },
    Return,
    Fault,
}
#[derive(Clone, Copy)]
pub struct Program(pub [Instruction; 5]);
impl Program {
    pub fn at(self, pc: Pc) -> Instruction {
        self.0[pc as usize]
    }
}

// if x == 1 { fault }; while x < 6 { x = min(x + 2, 7) }; return x
pub const PROGRAM: Program = Program([
    Instruction::Equal {
        value: 1,
        yes: Pc::Fault,
        no: Pc::Head,
    },
    Instruction::Less {
        limit: 6,
        yes: Pc::Step,
        no: Pc::Return,
    },
    Instruction::Add {
        amount: 2,
        next: Pc::Head,
    },
    Instruction::Return,
    Instruction::Fault,
]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub lo: u8,
    pub hi: u8,
}
pub type State = Reachable<Interval>;
impl Interval {
    pub const TOP: Self = Self { lo: 0, hi: MAX };
    pub fn contains(self, word: u8) -> bool {
        self.lo <= word && word <= self.hi
    }
    pub fn cover(words: impl IntoIterator<Item = u8>) -> State {
        let mut words = words.into_iter();
        let Some(first) = words.next() else {
            return Reachable::Bottom;
        };
        Reachable::Value(words.fold(
            Self {
                lo: first,
                hi: first,
            },
            |range, word| Self {
                lo: range.lo.min(word),
                hi: range.hi.max(word),
            },
        ))
    }
}
pub fn residue(word: u8) -> u8 {
    1 << (word % 2)
}

pub struct States;
impl Capability for States {
    type Key = Pc;
    type Value = State;
    fn key_work(_: &Pc) -> usize {
        1
    }
    fn value_work(_: &State) -> usize {
        3
    }
}
pub struct Parities;
impl Capability for Parities {
    type Key = Pc;
    // 0 is Bottom, BOTH is Top. This is a may-set, never a growing must-fact.
    type Value = u8;
    fn key_work(_: &Pc) -> usize {
        1
    }
    fn value_work(_: &u8) -> usize {
        1
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Returned,
    Faulted,
}
pub struct Outcomes;
impl Capability for Outcomes {
    type Key = Outcome;
    type Value = State;
    fn key_work(_: &Outcome) -> usize {
        1
    }
    fn value_work(_: &State) -> usize {
        3
    }
}
pub struct Hull;
impl JoinPolicy for Hull {
    type Value = Interval;
    type Error = Infallible;
    fn work(&self, _: &Interval, _: &Interval) -> usize {
        8
    }
    fn join(&self, a: &Interval, b: &Interval) -> Result<Interval, Infallible> {
        Ok(Interval {
            lo: a.lo.min(b.lo),
            hi: a.hi.max(b.hi),
        })
    }
    fn equivalent(&self, a: &Interval, b: &Interval) -> bool {
        a == b
    }
}
pub struct Union;
impl JoinPolicy for Union {
    type Value = u8;
    type Error = Infallible;
    fn work(&self, _: &u8, _: &u8) -> usize {
        3
    }
    fn join(&self, a: &u8, b: &u8) -> Result<u8, Infallible> {
        Ok(a | b)
    }
    fn equivalent(&self, a: &u8, b: &u8) -> bool {
        a == b
    }
}

#[derive(Clone, Copy)]
pub struct At {
    pub scope: Scope,
    pub pc: Pc,
}
impl At {
    pub fn key(self) -> Scoped<Pc> {
        Scoped::new(self.scope, self.pc)
    }
}
pub struct Input {
    pub scope: Scope,
    // A fixed concrete input universe, with one bit per word, not per residue.
    pub words: u8,
}
pub type Task = Either<Input, Either<At, At>>;
#[derive(Clone, Copy)]
pub struct ContextId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    Work,
}
pub type Error = PublishError<Infallible, Stop>;
