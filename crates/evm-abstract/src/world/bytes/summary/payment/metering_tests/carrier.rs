//! Small native interval with a private exclusion absent from its observation.

use crate::{
    U256,
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, SummaryConstruction, WordElement,
        WordObservation, interval::Interval,
    },
    test_instrumentation::Counter,
};
use serde::Serialize;
use std::fmt;

pub(super) const FEE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Stage {
    Input,
    Literal,
    Constructed,
    Copied,
    Joined,
    Widened,
    Projected,
    Canonical,
    Compacted,
    Restored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hook {
    Copy,
    Singleton,
    Equal,
    IsTop,
    Literal,
    UnknownByte,
    Range,
    Top,
    Join,
    Widen,
    Project,
    Canonical,
    Compact,
    Restore,
    Observation,
    FromObservation,
    Import,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Receipt {
    pub hooks: [usize; 17],
    pub quotes: [usize; 17],
}

pub(super) static RECEIPT: Counter<Receipt> = Counter::new(Receipt {
    hooks: [0; 17],
    quotes: [0; 17],
});

fn hook(kind: Hook) {
    RECEIPT.update(|receipt| receipt.hooks[kind as usize] += 1);
}

fn quote(kind: Hook, work: usize) -> usize {
    RECEIPT.update(|receipt| receipt.quotes[kind as usize] += 1);
    work
}

#[derive(Debug, Serialize)]
pub(super) struct Word {
    pub lower: U256,
    pub upper: U256,
    pub excluded: Option<U256>,
    interval: Interval,
    pub stage: Stage,
    pub copy_fee: usize,
    pub query_fee: usize,
    pub copied_query_fee: usize,
    pub compacted_copy_fee: usize,
}

impl Word {
    pub fn window(lower: U256, upper: U256, excluded: Option<U256>) -> Self {
        Self {
            lower,
            upper,
            excluded,
            interval: Interval::new_unsigned(lower, upper).unwrap(),
            stage: Stage::Input,
            copy_fee: FEE,
            query_fee: FEE,
            copied_query_fee: FEE,
            compacted_copy_fee: FEE,
        }
    }
}

impl Clone for Word {
    fn clone(&self) -> Self {
        hook(Hook::Copy);
        Self {
            lower: self.lower,
            upper: self.upper,
            excluded: self.excluded,
            interval: self.interval,
            stage: Stage::Copied,
            copy_fee: self.copy_fee,
            query_fee: self.copied_query_fee,
            copied_query_fee: self.copied_query_fee,
            compacted_copy_fee: self.compacted_copy_fee,
        }
    }
}

impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        hook(Hook::Equal);
        self.lower == other.lower && self.upper == other.upper && self.excluded == other.excluded
    }
}
impl Eq for Word {}

impl fmt::Display for Word {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "[{},{}] except {:?}",
            self.lower, self.upper, self.excluded
        )
    }
}

impl WordElement for Word {
    fn top() -> Self {
        hook(Hook::Top);
        Self::window(U256::ZERO, U256::MAX, None)
    }
    fn constant(value: U256) -> Self {
        hook(Hook::Literal);
        Self {
            stage: Stage::Literal,
            ..Self::window(value, value, None)
        }
    }
    fn unknown_byte() -> Self {
        hook(Hook::UnknownByte);
        Self {
            stage: Stage::Constructed,
            ..Self::window(U256::ZERO, U256::from(255), None)
        }
    }
    fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        hook(Hook::Range);
        (lower <= upper).then(|| Self {
            stage: Stage::Constructed,
            ..Self::window(lower, upper, None)
        })
    }
    fn from_observation(observation: &WordObservation) -> Self {
        hook(Hook::FromObservation);
        let (lower, upper) = observation.interval().unsigned_bounds();
        Self::window(lower, upper, None)
    }
    fn observation(&self) -> WordObservation {
        hook(Hook::Observation);
        WordObservation::unsigned_range(self.lower, self.upper).unwrap()
    }
    fn interval(&self) -> &Interval {
        &self.interval
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        hook(Hook::Singleton);
        (self.lower == self.upper && self.excluded != Some(self.lower)).then_some(self.lower)
    }
    fn is_top(&self) -> bool {
        hook(Hook::IsTop);
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        32
    }
    fn clone_work(&self) -> usize {
        quote(Hook::Copy, self.copy_fee)
    }
    fn singleton_work(&self) -> usize {
        quote(Hook::Singleton, self.query_fee)
    }
    fn canonical_for_summary(&self) -> Self {
        hook(Hook::Canonical);
        Self {
            stage: Stage::Canonical,
            ..self.clone()
        }
    }
    fn requires_retained_binding(&self) -> bool {
        self.excluded.is_some()
    }
    fn compact_for_retention(&self) -> Self {
        hook(Hook::Compact);
        Self {
            stage: Stage::Compacted,
            copy_fee: self.compacted_copy_fee,
            ..self.clone()
        }
    }
    fn restore_redundant_finite_from(&self, _previous: &Self) -> Self {
        hook(Hook::Restore);
        Self {
            stage: Stage::Restored,
            ..self.clone()
        }
    }
    fn import_binding(&self, _observation: &WordObservation) -> Self {
        hook(Hook::Import);
        self.clone()
    }
    fn import_binding_work(&self, _observation: &WordObservation) -> usize {
        quote(Hook::Import, self.copy_fee.saturating_add(16))
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Policy {
    pub refusal: Option<(Hook, Stage)>,
    pub refusal_fee: usize,
    pub joined_copy_fee: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            refusal: None,
            refusal_fee: usize::MAX,
            joined_copy_fee: FEE,
        }
    }
}

impl Policy {
    fn work(&self, kind: Hook, stage: Stage) -> usize {
        quote(
            kind,
            if self.refusal == Some((kind, stage)) {
                self.refusal_fee
            } else {
                FEE
            },
        )
    }
    fn union(&self, left: &Word, right: &Word, stage: Stage) -> Word {
        Word {
            stage,
            copy_fee: self.joined_copy_fee,
            ..Word::window(
                left.lower.min(right.lower),
                left.upper.max(right.upper),
                (left.excluded == right.excluded)
                    .then_some(left.excluded)
                    .flatten(),
            )
        }
    }
}

impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        DomainSpec::default()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        hook(Hook::Project);
        Word {
            stage: Stage::Projected,
            ..value.clone()
        }
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        hook(Hook::Join);
        self.union(left, right, Stage::Joined)
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.union(old, next, Stage::Widened)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        hook(Hook::Widen);
        self.union(old, next, Stage::Widened)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        usize::MAX
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        self.work(Hook::Literal, Stage::Literal)
    }
    fn projection_work(&self, value: &Word) -> usize {
        self.work(Hook::Project, value.stage)
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        usize::MAX
    }
    fn summary_join_work(&self, left: &Word, right: &Word, widen: bool) -> usize {
        self.work(
            if widen { Hook::Widen } else { Hook::Join },
            if widen { right.stage } else { left.stage },
        )
    }
    fn summary_canonical_work(&self, value: &Word) -> usize {
        self.work(Hook::Canonical, value.stage)
            .saturating_add(value.copy_fee)
    }
    fn summary_construction_work(&self, construction: SummaryConstruction) -> usize {
        self.work(
            match construction {
                SummaryConstruction::Top => Hook::Top,
                SummaryConstruction::UnknownByte => Hook::UnknownByte,
                SummaryConstruction::UnsignedRange { .. } | SummaryConstruction::UnknownAddress => {
                    Hook::Range
                }
            },
            Stage::Constructed,
        )
    }
    fn summary_equal_work(&self, left: &Word, _right: &Word) -> usize {
        self.work(Hook::Equal, left.stage)
    }
    fn summary_is_top_work(&self, value: &Word) -> usize {
        self.work(Hook::IsTop, value.stage)
    }
    fn summary_compact_work(&self, value: &Word) -> usize {
        self.work(Hook::Compact, value.stage)
            .saturating_add(value.copy_fee)
    }
    fn summary_restore_work(&self, value: &Word, _previous: &Word) -> usize {
        self.work(Hook::Restore, value.stage)
            .saturating_add(value.copy_fee)
    }
    fn summary_observation_work(&self, _observation: &WordObservation) -> usize {
        self.work(Hook::FromObservation, Stage::Constructed)
    }
}
