//! Shared numeric transfers and path consequences for private recovery values.
//!
//! This adapter owns identity qualification and charges the recovery root ledger.
//! A refused optional consequence preserves an upper approximation; only proved
//! branch infeasibility removes an edge. Wire publication never feeds back here.

use super::state::Address;
use super::{
    Recovery,
    state::{AbstractValue, Cell, State},
};
use crate::{Completion, DiagnosticCode, Progress, U256, ValueFact};
use evm_domain::{
    EvmNumericAlgebra, SummaryConstruction,
    identity::Symbol,
    relational::{CheckResult, EvmValueQuery, intersect_numeric},
};

impl<P: FnMut(Progress)> Recovery<'_, P> {
    /// Export only complete finite covers; a product singleton can exist even
    /// when its finite component has already reached top.
    pub(super) fn value_fact(&mut self, value: &AbstractValue) -> Option<ValueFact> {
        if !self.owner.pay(
            value
                .singleton_work()
                .saturating_add(value.work_size().saturating_mul(8)),
        ) {
            return None;
        }
        if let Some(value) = value.singleton() {
            return Some(ValueFact::Constants(vec![value]));
        }
        // The finite component is already a complete cover of the product.
        // Keeping extra candidates avoids one membership query per candidate.
        Some(value.constants().map_or(ValueFact::Unknown, |constants| {
            ValueFact::Constants(constants.iter().copied().collect())
        }))
    }
    pub(super) fn join_value(
        &mut self,
        left: &AbstractValue,
        right: &AbstractValue,
        widen: bool,
    ) -> Option<AbstractValue> {
        let work = self
            .algebra
            .numeric_algebra()
            .summary_join_work(left.numeric(), right.numeric(), false)
            .saturating_add(
                left.work_size()
                    .saturating_add(right.work_size())
                    .saturating_mul(8),
            );
        if !self.owner.pay(work) {
            return None;
        }
        let joined = self.algebra.join(left, right);
        if !widen {
            return Some(joined);
        }
        if let Some(candidates) = joined.constants() {
            // A complete finite cover enumerates this product's entire concrete
            // denotation after membership filtering. Rebuild its numeric fields
            // through shared joins of exact literals, preserving all metadata.
            // The canonical sets ascend only up to capacity; their other fields
            // depend only on that set, so redundant interval chains cannot grow.
            if !self.owner.pay(
                candidates
                    .len()
                    .saturating_mul(joined.numeric().work_size()),
            ) {
                return None;
            }
            let mut canonical = None;
            for candidate in candidates {
                if !self.owner.pay(0) {
                    return None;
                }
                if !joined.numeric().contains(*candidate) {
                    continue;
                }
                if !self.owner.pay(self.algebra.literal_quote_work(*candidate)) {
                    return None;
                }
                let literal = evm_domain::NumericValue::constant(*candidate);
                canonical = Some(if let Some(previous) = canonical {
                    if !self.owner.pay(
                        self.algebra
                            .numeric_algebra()
                            .summary_join_work(&previous, &literal, false),
                    ) {
                        return None;
                    }
                    self.algebra
                        .numeric_algebra()
                        .join_numeric(&previous, &literal)
                } else {
                    literal
                });
            }
            // An empty optional projection never invents a bottom state.
            return Some(match canonical {
                Some(numeric) => joined.with_numeric(numeric),
                None => joined,
            });
        }
        if !self.owner.pay(
            self.algebra
                .numeric_algebra()
                .summary_join_work(left.numeric(), joined.numeric(), true)
                .saturating_add(
                    left.work_size()
                        .saturating_add(joined.work_size())
                        .saturating_mul(8),
                ),
        ) {
            return None;
        }
        // Once the finite cover exceeds capacity, its Top is absorbing at
        // later entry joins. The shared procedure policy also prevents
        // interval-implied bits from undoing acceleration of moving loops.
        Some(self.algebra.widen_procedure(left, &joined))
    }
    /// Shared helpers charge this run's root ledger directly. Its refusal bit
    /// must be surfaced even when a subsequent zero-sized charge would fit.
    fn shared_complete(&mut self) -> bool {
        if !self.owner.pay(0) {
            return false;
        }
        if self.owner.budget.exhausted() {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "shared abstract semantics exhausted logical work",
            );
            return false;
        }
        true
    }
    pub(super) fn refine_value(
        &mut self,
        value: &mut AbstractValue,
        relations: &evm_domain::relational::RelationState,
    ) -> bool {
        if relations.is_empty() || value.expression().is_none() {
            return true;
        }
        if !self.owner.pay(value.singleton_work()) {
            return false;
        }
        if value.singleton().is_some() {
            return true;
        }
        let expression = value.expression().expect("checked expression").clone();
        let limits = self.algebra.spec().relations();
        let query = relations.local_value_budgeted(
            &expression,
            &limits,
            &mut self.owner.budget,
            self.algebra.borrowed(),
        );
        if !self.shared_complete() {
            return false;
        }
        let bounds = match query {
            EvmValueQuery::Exact(word) => {
                if !self.owner.pay(self.algebra.literal_quote_work(word)) {
                    return false;
                }
                evm_domain::NumericValue::constant(word)
            }
            EvmValueQuery::Bounds(bounds) => *bounds,
            // Only branch feasibility checks eliminate a path. Refusal and
            // optional projection failures retain the original upper bound.
            EvmValueQuery::ProvenImpossible | EvmValueQuery::Undetermined(_) => return true,
        };
        if !self.owner.pay(
            self.algebra
                .numeric_algebra()
                .refinement_work(value.numeric(), bounds.work_size())
                .saturating_add(bounds.work_size().saturating_mul(8)),
        ) {
            return false;
        }
        if let Some(numeric) = intersect_numeric(value.numeric(), &bounds, self.algebra.borrowed())
        {
            *value = value.clone().with_numeric(numeric);
        }
        true
    }
    fn refine_state(&mut self, state: &mut State) -> bool {
        for value in state
            .stack
            .iter_mut()
            .map(|cell| &mut cell.abstract_value)
            .chain(state.memory.words.values_mut())
            .chain(state.storage.slots.values_mut())
            .chain(state.transient.slots.values_mut())
        {
            if !self.refine_value(value, &state.relations) {
                return false;
            }
        }
        true
    }
    /// `None` is a resource stop; `Some(None)` is a proved infeasible edge.
    pub(super) fn assume_state(
        &mut self,
        state: &State,
        condition: &AbstractValue,
        nonzero: bool,
    ) -> Option<Option<State>> {
        if !self.owner.pay(
            state
                .work_size()
                .saturating_mul(4)
                .saturating_add(condition.singleton_work())
                .saturating_add(condition.work_size()),
        ) {
            return None;
        }
        let possible = if nonzero {
            condition.may_be_nonzero()
        } else {
            condition.may_be_zero()
        };
        if !possible {
            return Some(None);
        }
        let mut state = state.clone();
        if let Some(expression) = condition.expression() {
            let result = state.relations.assume_checked_budgeted(
                expression,
                nonzero,
                &self.algebra.spec().relations(),
                &mut self.owner.budget,
                self.algebra.borrowed(),
            );
            if !self.shared_complete() {
                return None;
            }
            if result == Ok(CheckResult::ProvenImpossible) {
                return Some(None);
            }
            if !self.refine_state(&mut state) {
                return None;
            }
        }
        Some(Some(state))
    }
    pub(super) fn operation_value(
        &mut self,
        code: u8,
        immediate: Option<U256>,
        pc: usize,
        args: &[Cell],
        state: &State,
    ) -> Option<AbstractValue> {
        if matches!(code, 0x35 | 0x40 | 0x49 | 0x51 | 0x54 | 0x5c)
            && !self.owner.pay(args[0].abstract_value.singleton_work())
        {
            return None;
        }
        let value = match code {
            0x5f..=0x7f | 0x58 | 0x38 => {
                let literal = match code {
                    0x58 => U256::from(pc),
                    0x38 => U256::from(self.program.byte_len()),
                    _ => immediate.unwrap_or(U256::ZERO),
                };
                if !self.owner.pay(self.algebra.literal_quote_work(literal)) {
                    return None;
                }
                AbstractValue::constant(literal)
            }
            0x51 | 0x54 | 0x5c => {
                // Map hits clone an existing value under instruction_work.
                // Missing observations construct Top or an implicit zero word;
                // reserve both possible native factories before the lookup.
                let factories = self
                    .algebra
                    .numeric_algebra()
                    .summary_construction_work(SummaryConstruction::Top)
                    .saturating_add(if code == 0x51 {
                        self.algebra.literal_quote_work(U256::ZERO)
                    } else {
                        0
                    });
                if !self.owner.pay(factories) {
                    return None;
                }
                let address = Address::of(&args[0].abstract_value);
                match code {
                    0x51 => state.memory.read(address.as_ref()),
                    0x54 => state.storage.read(address.as_ref()),
                    _ => state.transient.read(address.as_ref()),
                }
            }
            _ if evm_bytecode::word::arity(code).is_some() => {
                let values: Vec<_> = args
                    .iter()
                    .map(|cell| cell.abstract_value.clone())
                    .collect();
                if !self.owner.pay(self.algebra.operation_work(code, &values)) {
                    return None;
                }
                self.algebra.apply(code, &values)
            }
            _ => {
                let symbol = match code {
                    0x30 => Some(Symbol::To),
                    0x32 => Some(Symbol::Origin),
                    0x33 => Some(Symbol::Caller),
                    0x34 => Some(Symbol::CallValue),
                    0x35 => args[0].abstract_value.singleton().map(Symbol::CalldataWord),
                    0x36 => Some(Symbol::CalldataLength),
                    0x3a => Some(Symbol::GasPrice),
                    0x40 => args[0].abstract_value.singleton().map(Symbol::BlockHash),
                    0x41 => Some(Symbol::Coinbase),
                    0x42 => Some(Symbol::Timestamp),
                    0x43 => Some(Symbol::Number),
                    0x44 => Some(Symbol::Prevrandao),
                    0x45 => Some(Symbol::GasLimit),
                    0x46 => Some(Symbol::ChainId),
                    0x48 => Some(Symbol::BaseFee),
                    0x49 => args[0].abstract_value.singleton().map(Symbol::BlobHash),
                    0x4a => Some(Symbol::BlobBaseFee),
                    _ => None,
                };
                let construction = match code {
                    0x30 | 0x32 | 0x33 | 0x41 | 0xf0 | 0xf5 => SummaryConstruction::UnknownAddress,
                    0xf1 | 0xf2 | 0xf4 | 0xfa => SummaryConstruction::UnsignedRange {
                        lower: U256::ZERO,
                        upper: U256::from(1),
                    },
                    _ => SummaryConstruction::Top,
                };
                if !self.owner.pay(
                    self.algebra
                        .numeric_algebra()
                        .summary_construction_work(construction)
                        .saturating_add(32),
                ) {
                    return None;
                }
                let value = match code {
                    0x30 | 0x32 | 0x33 | 0x41 | 0xf0 | 0xf5 => AbstractValue::unknown_address(),
                    0xf1 | 0xf2 | 0xf4 | 0xfa => {
                        AbstractValue::unsigned_range(U256::ZERO, U256::from(1))
                            .expect("boolean range")
                    }
                    _ => AbstractValue::top(),
                };
                // These are immutable values in this one runtime frame. Dynamic
                // observations (GAS/MSIZE/return data/storage/calls) get no term.
                match symbol {
                    Some(symbol) => value.with_symbol(symbol, Some(0)),
                    None => value,
                }
            }
        };
        Some(value)
    }
}
