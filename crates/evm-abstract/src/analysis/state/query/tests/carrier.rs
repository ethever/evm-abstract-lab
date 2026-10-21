//! A small carrier whose independent proof tariff exceeds its retained size.

use crate::domain::EvmNumericAlgebra as _;
use crate::{
    U256,
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, ProductAlgebra, WordElement,
        WordObservation,
    },
};
use analysis_control::Cancellation;
use serde::Serialize;
use std::{cell::Cell, fmt, rc::Rc};

#[derive(Debug)]
pub(in crate::analysis) struct Probe {
    pub fee: usize,
    pub quotes: Cell<usize>,
    pub queries: Cell<usize>,
    pub cancellations: Cell<usize>,
    pub cancel_quote: Option<Cancellation>,
    pub cancel_query: Option<Cancellation>,
}

impl Probe {
    pub fn new(fee: usize) -> Self {
        Self {
            fee,
            quotes: Cell::new(0),
            queries: Cell::new(0),
            cancellations: Cell::new(0),
            cancel_quote: None,
            cancel_query: None,
        }
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.quotes.get(),
            self.queries.get(),
            self.cancellations.get(),
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::analysis) struct Word {
    pub lower: U256,
    pub upper: U256,
    pub excluded: Option<U256>,
    #[serde(skip)]
    probe: Option<Rc<Probe>>,
}

impl Word {
    pub fn watched(mut self, probe: &Rc<Probe>) -> Self {
        self.probe = Some(Rc::clone(probe));
        self
    }
}

impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        (self.lower, self.upper, self.excluded) == (other.lower, other.upper, other.excluded)
    }
}
impl Eq for Word {}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}..={} except {:?}",
            self.lower, self.upper, self.excluded
        )
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self {
            lower: U256::ZERO,
            upper: U256::MAX,
            excluded: None,
            probe: None,
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            lower: word,
            upper: word,
            ..Self::top()
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        let (lower, upper) = facts.interval().unsigned_bounds();
        Self {
            lower,
            upper,
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        // Export deliberately cannot express the interior excluded word.
        WordObservation::unsigned_range(self.lower, self.upper).unwrap()
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        if let Some(probe) = &self.probe {
            probe.queries.set(probe.queries.get() + 1);
            if let Some(token) = &probe.cancel_query {
                probe.cancellations.set(probe.cancellations.get() + 1);
                token.cancel();
            }
        }
        (self.lower == self.upper && self.contains(self.lower)).then_some(self.lower)
    }
    fn singleton_work(&self) -> usize {
        let Some(probe) = &self.probe else {
            return 1;
        };
        probe.quotes.set(probe.quotes.get() + 1);
        if let Some(token) = &probe.cancel_quote {
            probe.cancellations.set(probe.cancellations.get() + 1);
            token.cancel();
        }
        probe.fee
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        1
    }
}

#[derive(Debug)]
pub(in crate::analysis) struct Algebra;

impl EvmNumericAlgebra for Algebra {
    type Element = Word;

    fn spec(&self) -> DomainSpec {
        ProductAlgebra::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        if left == right {
            left.clone()
        } else {
            Word {
                lower: left.lower.min(right.lower),
                upper: left.upper.max(right.upper),
                excluded: left.excluded.filter(|_| left.excluded == right.excluded),
                ..Word::top()
            }
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        if old == next {
            old.clone()
        } else {
            Word::top()
        }
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.widen_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        let _args: Vec<&Word> = _args.collect();
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        // No test below requests a numeric operation; refuse rather than claim
        // that a small retained-size quote funds this opaque carrier's queries.
        usize::MAX
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        64
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        usize::MAX
    }
}
