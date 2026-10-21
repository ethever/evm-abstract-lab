//! Opaque numeric facts and query instruments shared by transfer witnesses.
//! No default numeric product is used to answer private exclusions.

use crate::{
    U256,
    domain::{
        DomainSpec, EvmNumericAlgebra, NumericReduction, ReductionStatus, WordElement,
        WordObservation,
        facts::{FactError, Symbol},
    },
    test_instrumentation::Counter,
};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy)]
pub(in crate::analysis::transfer) struct Receipt {
    pub watched: Option<U256>,
    pub fee: usize,
    pub quotes: usize,
    pub queries: usize,
    pub cancel_on_quote: bool,
    pub cancel_on_query: bool,
    pub cancel_on_constrain: bool,
    pub cancel_on_reduce: bool,
    pub reduction_status: ReductionStatus,
    pub literal_fee: usize,
    pub cancel_on_apply: bool,
    pub query_tag: Option<u64>,
}

pub(in crate::analysis::transfer) static RECEIPT: Counter<Receipt> = Counter::new(Receipt {
    watched: Some(U256::from_limbs([7, 0, 0, 0])),
    fee: 61,
    quotes: 0,
    queries: 0,
    cancel_on_quote: false,
    cancel_on_query: false,
    cancel_on_constrain: false,
    cancel_on_reduce: false,
    reduction_status: ReductionStatus::Stable,
    literal_fee: 64,
    cancel_on_apply: false,
    query_tag: None,
});

pub(in crate::analysis::transfer) static LITERALS: Counter<usize> = Counter::new(0);

/// A range with an unexported exclusion and no finite/bits/range trait views.
#[derive(Clone, Debug, Serialize)]
pub(in crate::analysis::transfer) struct Word {
    pub lower: U256,
    pub upper: U256,
    pub excluded: Option<U256>,
    #[serde(skip)]
    pub cancel_refinement_owner: Option<analysis_control::Cancellation>,
    #[serde(skip)]
    pub cancel_query_owner: Option<analysis_control::Cancellation>,
    #[serde(skip)]
    pub cancel_quote_owner: Option<analysis_control::Cancellation>,
    #[serde(skip)]
    pub query_tag: Option<u64>,
}

// Instrumentation does not change the represented concrete set.
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        (self.lower, self.upper, self.excluded) == (other.lower, other.upper, other.excluded)
    }
}
impl Eq for Word {}

impl Word {
    fn exact(&self) -> Option<U256> {
        (self.lower == self.upper && self.excluded != Some(self.lower)).then_some(self.lower)
    }
}

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
            cancel_refinement_owner: None,
            cancel_query_owner: None,
            cancel_quote_owner: None,
            query_tag: None,
        }
    }
    fn constant(word: U256) -> Self {
        LITERALS.update(|count| *count += 1);
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
        // Explicit export loses the interior exclusion without querying Self.
        self.exact().map_or_else(
            || WordObservation::unsigned_range(self.lower, self.upper).unwrap(),
            WordObservation::constant,
        )
    }
    fn constrain(&self, facts: &WordObservation) -> Result<Self, FactError> {
        RECEIPT.update(|receipt| {
            if receipt.cancel_on_constrain {
                self.cancel_refinement_owner
                    .as_ref()
                    .unwrap_or(&analysis_control::current_cancellation())
                    .cancel();
            }
        });
        let (lower, upper) = facts.interval().unsigned_bounds();
        let mut next = Self {
            lower: self.lower.max(lower),
            upper: self.upper.min(upper),
            excluded: self.excluded,
            cancel_refinement_owner: self.cancel_refinement_owner.clone(),
            cancel_query_owner: self.cancel_query_owner.clone(),
            cancel_quote_owner: self.cancel_quote_owner.clone(),
            query_tag: self.query_tag,
        };
        if let Some(values) = facts.constants() {
            let mut retained = values.iter().copied().filter(|word| self.contains(*word));
            let Some(first) = retained.next() else {
                return Err(FactError::Contradiction {
                    subject: Symbol::THIS,
                });
            };
            next.lower = next.lower.max(first);
            next.upper = next.upper.min(retained.next_back().unwrap_or(first));
        }
        if next.lower > next.upper || next.exact().is_none() && next.lower == next.upper {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        Ok(next)
    }
    fn contains(&self, word: U256) -> bool {
        self.lower <= word && word <= self.upper && self.excluded != Some(word)
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|receipt| {
            if receipt.watched == self.exact()
                && receipt
                    .query_tag
                    .is_none_or(|tag| self.query_tag == Some(tag))
            {
                receipt.queries += 1;
                if receipt.cancel_on_query {
                    analysis_control::current_cancellation().cancel();
                }
            }
        });
        if let Some(owner) = &self.cancel_query_owner {
            owner.cancel();
        }
        self.exact()
    }
    fn singleton_work(&self) -> usize {
        let mut fee = 64;
        RECEIPT.update(|receipt| {
            if receipt.watched == self.exact()
                && receipt
                    .query_tag
                    .is_none_or(|tag| self.query_tag == Some(tag))
            {
                receipt.quotes += 1;
                fee = receipt.fee;
                if receipt.cancel_on_quote {
                    analysis_control::current_cancellation().cancel();
                }
            }
        });
        if let Some(owner) = &self.cancel_quote_owner {
            owner.cancel();
        }
        fee
    }
    fn is_top(&self) -> bool {
        self.lower == U256::ZERO && self.upper == U256::MAX && self.excluded.is_none()
    }
    fn work_size(&self) -> usize {
        64
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct Policy;

impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        crate::domain::Domain::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, a: &Word, b: &Word) -> Word {
        Word {
            lower: a.lower.min(b.lower),
            upper: a.upper.max(b.upper),
            excluded: a.excluded.filter(|word| !b.contains(*word)),
            ..Word::top()
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        Word {
            lower: if next.lower < old.lower {
                U256::ZERO
            } else {
                old.lower
            },
            upper: if next.upper > old.upper {
                U256::MAX
            } else {
                old.upper
            },
            excluded: old.excluded.filter(|word| !next.contains(*word)),
            ..Word::top()
        }
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.widen_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        let mut status = ReductionStatus::Stable;
        RECEIPT.update(|receipt| {
            status = receipt.reduction_status;
            if receipt.cancel_on_reduce {
                value
                    .cancel_refinement_owner
                    .as_ref()
                    .unwrap_or(&analysis_control::current_cancellation())
                    .cancel();
            }
        });
        NumericReduction {
            status,
            ..NumericReduction::stable(value.clone())
        }
    }
    fn apply_numeric(&self, _op: u8, _args: &[Word]) -> NumericReduction<Word> {
        RECEIPT.update(|receipt| {
            if receipt.cancel_on_apply {
                analysis_control::current_cancellation().cancel();
            }
        });
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        // This test policy's whole-operation envelope covers its own internals;
        // it supplies no credit for the transfer's separate result queries.
        args.fold(1024usize, |work, word| {
            work.saturating_add(word.work_size() * 8)
        })
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        let mut fee = 64;
        RECEIPT.update(|receipt| fee = receipt.literal_fee);
        fee
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        value
            .work_size()
            .saturating_add(facts.saturating_add(1).saturating_mul(1024))
    }
}
