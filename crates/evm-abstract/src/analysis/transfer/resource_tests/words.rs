//! Owned representation data for resource tests; this is not an alternate
//! numerical semantics or an independent-analyzer cooperation witness.

use crate::{
    U256,
    domain::{
        Domain, DomainSpec, EvmNumericAlgebra, NumericReduction, NumericValue, WordElement,
        WordObservation,
    },
};
use serde::Serialize;
use std::{cell::Cell, collections::BTreeSet, fmt};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::analysis::transfer) struct Copies {
    pub bytes: usize,
    pub quotes: usize,
    pub bytes_at_last_quote: usize,
}

thread_local! {
    static COPIES: Cell<Copies> = const { Cell::new(Copies { bytes: 0, quotes: 0, bytes_at_last_quote: 0 }) };
}

pub(in crate::analysis::transfer) fn reset() {
    COPIES.set(Copies::default());
}
pub(in crate::analysis::transfer) fn copies() -> Copies {
    COPIES.get()
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(in crate::analysis::transfer) struct Heavy {
    word: NumericValue,
    pub owned: Vec<u8>,
    clone_multiplier: usize,
}

impl Heavy {
    pub fn payload(word: NumericValue, bytes: usize) -> Self {
        Self {
            word,
            owned: vec![7; bytes],
            clone_multiplier: 1,
        }
    }

    fn costly_byte_bound() -> Self {
        Self {
            word: NumericValue::unknown_byte(),
            owned: vec![7; 16_384],
            clone_multiplier: 8,
        }
    }
}

impl Clone for Heavy {
    fn clone(&self) -> Self {
        COPIES.update(|mut copies| {
            copies.bytes += self.owned.len();
            copies
        });
        Self {
            word: self.word.clone(),
            owned: self.owned.clone(),
            clone_multiplier: self.clone_multiplier,
        }
    }
}

impl fmt::Display for Heavy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.word, f)
    }
}

impl WordElement for Heavy {
    fn top() -> Self {
        Self::payload(NumericValue::top(), 0)
    }
    fn constant(value: U256) -> Self {
        Self::payload(NumericValue::constant(value), 0)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self::payload(NumericValue::from_observation(observation), 0)
    }
    fn observation(&self) -> WordObservation {
        self.word.observation()
    }
    fn singleton(&self) -> Option<U256> {
        self.word.singleton()
    }
    fn contains(&self, word: U256) -> bool {
        self.word.contains(word)
    }
    fn is_top(&self) -> bool {
        self.word.is_top()
    }
    fn work_size(&self) -> usize {
        self.owned
            .len()
            .saturating_add(self.word.work_size())
            .saturating_add(16)
    }
    fn clone_work(&self) -> usize {
        if !self.owned.is_empty() {
            COPIES.update(|mut copies| {
                copies.quotes += 1;
                copies.bytes_at_last_quote = copies.bytes;
                copies
            });
        }
        self.work_size().saturating_mul(self.clone_multiplier)
    }
    fn singleton_work(&self) -> usize {
        self.word.singleton_work()
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.word.constants()
    }
    fn finite_constants(&self) -> &crate::domain::FiniteConstantSet {
        self.word.finite_constants()
    }
    fn known_bits(&self) -> &crate::domain::known_bits::KnownBits {
        self.word.known_bits()
    }
    fn interval(&self) -> &crate::domain::interval::Interval {
        self.word.interval()
    }
    fn congruence(&self) -> &crate::domain::congruence::Congruence {
        self.word.congruence()
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct Policy(pub Domain);

impl EvmNumericAlgebra for Policy {
    type Element = Heavy;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, value: &Heavy) -> Heavy {
        value.clone()
    }
    fn join_numeric(&self, left: &Heavy, right: &Heavy) -> Heavy {
        Heavy::payload(self.0.join_numeric(&left.word, &right.word), 0)
    }
    fn widen_numeric(&self, old: &Heavy, next: &Heavy) -> Heavy {
        Heavy::payload(self.0.widen_numeric(&old.word, &next.word), 0)
    }
    fn widen_procedure_numeric(&self, old: &Heavy, next: &Heavy) -> Heavy {
        Heavy::payload(self.0.widen_procedure_numeric(&old.word, &next.word), 0)
    }
    fn reduce_numeric(&self, value: &Heavy) -> NumericReduction<Heavy> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[Heavy]) -> NumericReduction<Heavy> {
        if op == revm_bytecode::opcode::AND
            && args.len() == 2
            && args
                .iter()
                .any(|value| value.singleton() == Some(U256::from(255)))
        {
            return NumericReduction::stable(Heavy::costly_byte_bound());
        }
        let words = args
            .iter()
            .map(|value| value.word.clone())
            .collect::<Vec<_>>();
        let result = self.0.apply_numeric(op, &words);
        NumericReduction {
            value: Heavy::payload(result.value, 0),
            status: result.status,
            rounds: result.rounds,
            strengthened: result.strengthened,
        }
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Heavy>) -> usize {
        args.fold(100_000usize, |work, value| {
            work.saturating_add(value.work_size().saturating_mul(100))
        })
    }
    fn projection_work(&self, value: &Heavy) -> usize {
        value.work_size().saturating_mul(4)
    }
    fn refinement_work(&self, value: &Heavy, facts: usize) -> usize {
        value.work_size().saturating_add(facts).saturating_mul(4096)
    }
}
