//! A private exclusion which validation must preserve without native work.

use crate::{
    U256,
    domain::{DomainSpec, EvmNumericAlgebra, NumericReduction, WordElement, WordObservation},
    test_instrumentation::Counter,
};
use std::fmt;

pub(super) type Receipt = usize;
pub(super) static RECEIPT: Counter<Receipt> = Counter::new(0);

fn hook() {
    RECEIPT.update(|count| *count += 1);
}

#[derive(Debug, serde::Serialize)]
pub(super) struct Word {
    lower: U256,
    upper: U256,
    pub excluded: Option<U256>,
}

impl Word {
    pub(super) fn window(lower: U256, upper: U256, excluded: Option<U256>) -> Self {
        Self {
            lower,
            upper,
            excluded,
        }
    }
}

impl Clone for Word {
    fn clone(&self) -> Self {
        hook();
        Self::window(self.lower, self.upper, self.excluded)
    }
}

impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        hook();
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
        hook();
        Self::window(U256::ZERO, U256::MAX, None)
    }
    fn constant(value: U256) -> Self {
        hook();
        Self::window(value, value, None)
    }
    fn from_observation(observation: &WordObservation) -> Self {
        hook();
        let (lower, upper) = observation.interval().unsigned_bounds();
        Self::window(lower, upper, None)
    }
    fn observation(&self) -> WordObservation {
        hook();
        WordObservation::unsigned_range(self.lower, self.upper).unwrap()
    }
    fn contains(&self, word: U256) -> bool {
        hook();
        self.lower <= word && word <= self.upper && self.excluded != Some(word)
    }
    fn is_top(&self) -> bool {
        hook();
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        hook();
        usize::MAX
    }
    fn clone_work(&self) -> usize {
        hook();
        usize::MAX
    }
    fn singleton_work(&self) -> usize {
        hook();
        usize::MAX
    }
}

#[derive(Debug, Default)]
pub(super) struct Policy {
    spec: DomainSpec,
}

impl EvmNumericAlgebra for Policy {
    type Element = Word;

    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, _value: &Word) -> Word {
        panic!("validation called native projection")
    }
    fn join_numeric(&self, _left: &Word, _right: &Word) -> Word {
        panic!("validation called native join")
    }
    fn widen_numeric(&self, _old: &Word, _next: &Word) -> Word {
        panic!("validation called native widening")
    }
    fn widen_procedure_numeric(&self, _old: &Word, _next: &Word) -> Word {
        panic!("validation called native procedure widening")
    }
    fn reduce_numeric(&self, _value: &Word) -> NumericReduction<Word> {
        panic!("validation called native reduction")
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        panic!("validation called native execution")
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        hook();
        usize::MAX
    }
    fn projection_work(&self, _value: &Word) -> usize {
        hook();
        usize::MAX
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        hook();
        usize::MAX
    }
}
