//! Frozen native test carrier with value-specific returned-Top hook controls.
#![allow(dead_code)]
use std::cell::{Cell, RefCell};
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Targets {
    pub factories: usize,
    pub copy_quotes: usize,
    pub copies: usize,
    pub query_quotes: usize,
    pub queries: usize,
}
thread_local! {
    pub(super) static TOP_FEES: Cell<(usize, usize)> = const { Cell::new((64,32)) };
    pub(super) static TARGETS: RefCell<Targets> = const { RefCell::new(Targets {factories:0,copy_quotes:0,copies:0,query_quotes:0,queries:0}) };
}
pub(super) fn configure(copy: usize, query: usize) {
    TOP_FEES.set((copy, query));
    TARGETS.set(Targets::default());
}
pub(super) fn targets() -> Targets {
    TARGETS.with_borrow(|v| *v)
}

use crate::domain::{
    DomainSpec, EvmNumericAlgebra, NumericReduction, SummaryConstruction, WordElement,
    WordObservation,
};
use alloy_primitives::U256;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Calls {
    pub clones: usize,
    pub singletons: usize,
    pub equal: usize,
    pub is_top: usize,
    pub constructions: usize,
    pub joins: usize,
    pub widens: usize,
    pub projects: usize,
    pub canonical: usize,
    pub exports: usize,
    pub imports: usize,
    pub compactions: usize,
    pub restorations: usize,
}

pub(super) static CALLS: crate::test_instrumentation::Counter<Calls> =
    crate::test_instrumentation::Counter::new(Calls {
        clones: 0,
        singletons: 0,
        equal: 0,
        is_top: 0,
        constructions: 0,
        joins: 0,
        widens: 0,
        projects: 0,
        canonical: 0,
        exports: 0,
        imports: 0,
        compactions: 0,
        restorations: 0,
    });

#[derive(Debug, Serialize)]
pub(super) struct Native {
    pub exact: Option<U256>,
    pub excludes_seven: bool,
    pub copy_fee: usize,
    pub singleton_fee: usize,
}

impl Native {
    pub fn private() -> Self {
        Self {
            exact: None,
            excludes_seven: true,
            copy_fee: 64,
            singleton_fee: 32,
        }
    }
    fn new(exact: Option<U256>, excludes_seven: bool) -> Self {
        Self {
            exact,
            excludes_seven,
            copy_fee: 64,
            singleton_fee: 32,
        }
    }
}

impl Clone for Native {
    fn clone(&self) -> Self {
        CALLS.update(|calls| calls.clones += 1);
        if self.exact.is_none() && !self.excludes_seven {
            TARGETS.with_borrow_mut(|v| v.copies += 1);
        }
        Self {
            exact: self.exact,
            excludes_seven: self.excludes_seven,
            copy_fee: self.copy_fee,
            singleton_fee: self.singleton_fee,
        }
    }
}
impl PartialEq for Native {
    fn eq(&self, other: &Self) -> bool {
        CALLS.update(|calls| calls.equal += 1);
        self.exact == other.exact
            && self.excludes_seven == other.excludes_seven
            && self.copy_fee == other.copy_fee
            && self.singleton_fee == other.singleton_fee
    }
}
impl Eq for Native {}
impl fmt::Display for Native {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{:?}/exclude7={}", self.exact, self.excludes_seven)
    }
}
impl WordElement for Native {
    fn top() -> Self {
        CALLS.update(|calls| calls.constructions += 1);
        let mut out = Self::new(None, false);
        let (copy, query) = TOP_FEES.get();
        out.copy_fee = copy;
        out.singleton_fee = query;
        TARGETS.with_borrow_mut(|v| v.factories += 1);
        out
    }
    fn constant(value: U256) -> Self {
        CALLS.update(|calls| calls.constructions += 1);
        Self::new(Some(value), false)
    }
    fn from_observation(value: &WordObservation) -> Self {
        CALLS.update(|calls| calls.constructions += 1);
        CALLS.update(|calls| calls.imports += 1);
        let exact = value.constants().and_then(|values| {
            (values.len() == 1).then(|| *values.first().expect("one finite value"))
        });
        Self::new(exact, false)
    }
    fn unknown_byte() -> Self {
        // This carrier declines unsupported width predicates conservatively.
        Self::top()
    }
    fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        if lower > upper {
            return None;
        }
        CALLS.update(|calls| calls.constructions += 1);
        Some(Self::new((lower == upper).then_some(lower), false))
    }
    fn observation(&self) -> WordObservation {
        CALLS.update(|calls| calls.exports += 1);
        self.exact
            .map_or_else(WordObservation::top, WordObservation::constant)
    }
    fn singleton(&self) -> Option<U256> {
        CALLS.update(|calls| calls.singletons += 1);
        if self.exact.is_none() && !self.excludes_seven {
            TARGETS.with_borrow_mut(|v| v.queries += 1);
        }
        self.exact
    }
    fn singleton_work(&self) -> usize {
        if self.exact.is_none() && !self.excludes_seven {
            TARGETS.with_borrow_mut(|v| v.query_quotes += 1);
        }
        self.singleton_fee
    }
    fn contains(&self, value: U256) -> bool {
        self.exact.is_none_or(|exact| exact == value)
            && !(self.excludes_seven && value == U256::from(7))
    }
    fn is_top(&self) -> bool {
        CALLS.update(|calls| calls.is_top += 1);
        self.exact.is_none() && !self.excludes_seven
    }
    fn work_size(&self) -> usize {
        // Inline payload plus one observation export (including the singleton
        // finite-allocation handle), as required by WordElement's quote law.
        64
    }
    fn clone_work(&self) -> usize {
        if self.exact.is_none() && !self.excludes_seven {
            TARGETS.with_borrow_mut(|v| v.copy_quotes += 1);
        }
        self.copy_fee
    }
    fn requires_retained_binding(&self) -> bool {
        self.excludes_seven
    }
    fn canonical_for_summary(&self) -> Self {
        CALLS.update(|calls| calls.canonical += 1);
        self.clone()
    }
    fn compact_for_retention(&self) -> Self {
        CALLS.update(|calls| calls.compactions += 1);
        self.clone()
    }
    fn restore_redundant_finite_from(&self, _previous: &Self) -> Self {
        CALLS.update(|calls| calls.restorations += 1);
        self.clone()
    }
}

#[derive(Debug)]
pub(super) struct Policy {
    pub join_fee: usize,
    pub widen_fee: usize,
    pub equality_fee: usize,
    pub is_top_fee: usize,
    pub constructor_fee: usize,
    pub projection_fee: usize,
    pub projected_copy_fee: Option<usize>,
    pub joined_singleton_fee: Option<usize>,
    pub capacity: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            join_fee: 256,
            widen_fee: 256,
            equality_fee: 64,
            is_top_fee: 32,
            constructor_fee: 128,
            projection_fee: 16,
            projected_copy_fee: None,
            joined_singleton_fee: None,
            capacity: 8,
        }
    }
}

impl Policy {
    fn upper(&self, left: &Native, right: &Native) -> Native {
        let exact = match (left.exact, right.exact) {
            (Some(left), Some(right)) if left == right => Some(left),
            _ => None,
        };
        let mut out = Native::new(exact, left.excludes_seven && right.excludes_seven);
        if let Some(fee) = self.joined_singleton_fee {
            out.singleton_fee = fee;
        }
        out
    }
}

impl EvmNumericAlgebra for Policy {
    type Element = Native;
    fn spec(&self) -> DomainSpec {
        DomainSpec::new(
            std::num::NonZeroUsize::new(self.capacity).unwrap(),
            std::num::NonZeroUsize::new(4).unwrap(),
            std::num::NonZeroUsize::new(256).unwrap(),
        )
    }
    fn project_numeric(&self, value: &Native) -> Native {
        CALLS.update(|calls| calls.projects += 1);
        let mut out = value.clone();
        if let Some(fee) = self.projected_copy_fee {
            out.copy_fee = fee;
        }
        out
    }
    fn join_numeric(&self, left: &Native, right: &Native) -> Native {
        CALLS.update(|calls| calls.joins += 1);
        self.upper(left, right)
    }
    fn widen_numeric(&self, left: &Native, right: &Native) -> Native {
        CALLS.update(|calls| calls.widens += 1);
        self.upper(left, right)
    }
    fn widen_procedure_numeric(&self, left: &Native, right: &Native) -> Native {
        self.widen_numeric(left, right)
    }
    fn reduce_numeric(&self, value: &Native) -> NumericReduction<Native> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Native>,
    ) -> NumericReduction<Native> {
        NumericReduction::stable(Native::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Native>) -> usize {
        256
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        128
    }
    fn projection_work(&self, _value: &Native) -> usize {
        self.projection_fee
    }
    fn refinement_work(&self, value: &Native, _facts: usize) -> usize {
        value.copy_fee
    }
    fn summary_join_work(&self, _left: &Native, _right: &Native, widen: bool) -> usize {
        if widen { self.widen_fee } else { self.join_fee }
    }
    fn summary_canonical_work(&self, value: &Native) -> usize {
        value.copy_fee.saturating_add(16)
    }
    fn summary_construction_work(&self, _kind: SummaryConstruction) -> usize {
        self.constructor_fee
    }
    fn summary_equal_work(&self, _left: &Native, _right: &Native) -> usize {
        self.equality_fee
    }
    fn summary_is_top_work(&self, _value: &Native) -> usize {
        self.is_top_fee
    }
    fn summary_compact_work(&self, value: &Native) -> usize {
        value.copy_fee.saturating_add(16)
    }
    fn summary_restore_work(&self, value: &Native, _previous: &Native) -> usize {
        value.copy_fee.saturating_add(16)
    }
    fn summary_observation_work(&self, observation: &WordObservation) -> usize {
        observation
            .work_size()
            .saturating_mul(4)
            .saturating_add(128)
    }
}
