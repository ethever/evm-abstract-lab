//! Native bounded finite-set semantics over the actual finite carrier.
//!
//! Every retained set is a complete disjunction. Capacity overflow and refused
//! enumeration lose precision to Top; they never keep a truncated sample. The
//! finite lattice needs no component scheduler: its local reduction only applies
//! the frozen capacity. Shared relation/fact limits still come from DomainSpec.

use super::{
    DomainSpec, EvmNumericAlgebra, FiniteConstantSet, NumericReduction, WordElement,
    WordObservation, concrete,
    facts::{FactError, Symbol},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

// A large complete input can produce very few distinct outputs. Bound raw
// Cartesian visits before evaluation rather than relying on output overflow to
// stop duplicate-heavy products. The same admission is used by the work quote.
const MAX_CARTESIAN_VISITS: usize = 4096;
const COPY_WORK: usize = 2;
const LITERAL_WORK: usize = 128;
const SINGLETON_WORK: usize = 16;

/// Standalone finite EVM analysis, with no product carrier or runtime selector.
#[derive(Clone, Copy, Debug)]
pub struct FiniteSetAlgebra {
    spec: DomainSpec,
}

impl FiniteSetAlgebra {
    /// Freeze numeric and shared relation limits without allocating candidates.
    pub fn from_spec(spec: DomainSpec) -> Self {
        Self { spec }
    }
}

impl Default for FiniteSetAlgebra {
    fn default() -> Self {
        Self::from_spec(DomainSpec::default())
    }
}

// Covers fixed-width comparisons, B-tree node searches/splits and word copies.
// Sizes are cached; quoting never walks a tree or copies a candidate.
fn tree_entry_work(size: usize) -> usize {
    let levels = usize::BITS as usize - size.max(1).leading_zeros() as usize;
    levels.saturating_add(1).saturating_mul(64)
}

fn cartesian_visits(args: &[&FiniteConstantSet]) -> Option<usize> {
    args.iter().try_fold(1usize, |visits, value| {
        visits
            .checked_mul(value.cardinality()?)
            .filter(|total| *total <= MAX_CARTESIAN_VISITS)
    })
}

fn result_range(op: u8) -> Option<u64> {
    match op {
        opcode::LT | opcode::GT | opcode::SLT | opcode::SGT | opcode::EQ | opcode::ISZERO => {
            Some(1)
        }
        opcode::BYTE => Some(255),
        opcode::CLZ => Some(256),
        _ => None,
    }
}

fn evaluation_work(op: u8) -> usize {
    match op {
        // Square-and-multiply uses at most 512 fixed-width multiplications;
        // reserve their limb work and the exponent/loop bookkeeping.
        opcode::EXP => 16384,
        // Include signed normalization and widened modular intermediates.
        opcode::ADDMOD | opcode::MULMOD => 4096,
        opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD => 1024,
        _ => 64,
    }
}

impl FiniteSetAlgebra {
    fn unconstrained_result(&self, op: u8) -> FiniteConstantSet {
        let Some(last) = result_range(op) else {
            return FiniteConstantSet::top();
        };
        // A known small image is still complete even when operands cannot be
        // enumerated. Insufficient capacity discards the entire image.
        if last as usize >= self.spec.capacity() {
            return FiniteConstantSet::top();
        }
        FiniteConstantSet::collect_bounded(
            (0..=last).map(U256::from),
            self.spec.constant_capacity(),
        )
        .expect("an instruction image is nonempty")
    }

    fn transfer(&self, op: u8, args: &[&FiniteConstantSet]) -> FiniteConstantSet {
        if cartesian_visits(args).is_none() {
            return self.unconstrained_result(op);
        }
        let first = args[0]
            .as_values()
            .expect("admitted finite Cartesian operand");
        let mut result = std::collections::BTreeSet::new();
        let mut insert = |a, b, c| {
            result.insert(concrete::evaluate(op, a, b, c));
            result.len() <= self.spec.capacity()
        };
        // Match arity directly: no synthetic singleton sets or operand copies
        // are needed for missing second/third operands.
        match args {
            [_] => {
                for a in first {
                    if !insert(*a, U256::ZERO, U256::ZERO) {
                        return FiniteConstantSet::top();
                    }
                }
            }
            [_, second] => {
                for a in first {
                    for b in second.as_values().expect("admitted finite operand") {
                        if !insert(*a, *b, U256::ZERO) {
                            return FiniteConstantSet::top();
                        }
                    }
                }
            }
            [_, second, third] => {
                for a in first {
                    for b in second.as_values().expect("admitted finite operand") {
                        for c in third.as_values().expect("admitted finite operand") {
                            if !insert(*a, *b, *c) {
                                return FiniteConstantSet::top();
                            }
                        }
                    }
                }
            }
            _ => unreachable!("pure instruction arity was validated"),
        }
        FiniteConstantSet::try_from_values(result).expect("finite operands are nonempty")
    }

    fn import_work(
        &self,
        value: &FiniteConstantSet,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> usize {
        let finite = finite_values.max(value.work_size());
        // The common fact importer pays table normalization and fixed-word
        // predicates, then the native importer filters one complete finite
        // cover. Four passes cover validation, export, filtering and rebuilding;
        // no product closure or capacity-sized future allocation is charged.
        4096usize
            .saturating_add(
                facts
                    .saturating_mul(facts.saturating_add(finite).saturating_add(32))
                    .saturating_mul(256),
            )
            .saturating_add(
                finite
                    .saturating_mul(256usize.saturating_add(tree_entry_work(finite)))
                    .saturating_mul(4),
            )
            .saturating_add(expression_work)
            .saturating_add(WordElement::work_size(value))
    }
}

impl EvmNumericAlgebra for FiniteSetAlgebra {
    type Element = FiniteConstantSet;

    fn spec(&self) -> DomainSpec {
        self.spec
    }

    fn project_numeric(&self, value: &Self::Element) -> Self::Element {
        value.limit(self.spec.constant_capacity())
    }

    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element {
        left.join(right, self.spec.constant_capacity())
    }

    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        // Bounded finite union already has finite ascending-chain height.
        self.join_numeric(old, next)
    }

    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        let joined = self.join_numeric(old, next);
        if old == &joined
            || joined
                .cardinality()
                .is_some_and(|len| len <= self.spec.widening_after_updates().saturating_add(1))
        {
            joined
        } else {
            FiniteConstantSet::top()
        }
    }

    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element> {
        NumericReduction::stable(self.project_numeric(value))
    }

    fn apply_numeric<'a>(
        &self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element> {
        let Some(arity) = concrete::arity(op).filter(|arity| *arity == args.len()) else {
            return NumericReduction::stable(FiniteConstantSet::top());
        };
        let first = args
            .next()
            .expect("supported pure instructions have operands");
        let operands = [
            first,
            args.next().unwrap_or(first),
            args.next().unwrap_or(first),
        ];
        NumericReduction::stable(self.transfer(op, &operands[..arity]))
    }

    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Self::Element>) -> usize {
        let mut count = 0usize;
        let mut visits = Some(1usize);
        let mut queries = 0usize;
        for value in args {
            count = count.saturating_add(1);
            visits = visits.and_then(|total| {
                total
                    .checked_mul(value.cardinality()?)
                    .filter(|total| *total <= MAX_CARTESIAN_VISITS)
            });
            // One symbolic-expression query per operand; reserve a second for
            // metadata decisions without copying any numeric operand.
            queries = queries.saturating_add(value.singleton_work().saturating_mul(2));
        }
        // Result queries, fixed dispatch and a facade-produced exact literal.
        let fixed = 64usize
            .saturating_add(queries)
            .saturating_add(SINGLETON_WORK.saturating_mul(2))
            .saturating_add(LITERAL_WORK);
        if concrete::arity(op) != Some(count) {
            return fixed;
        }
        let candidates = visits.unwrap_or_else(|| {
            result_range(op)
                .filter(|last| (*last as usize) < self.spec.capacity())
                .map_or(0, |last| last as usize + 1)
        });
        let output = candidates.min(self.spec.capacity().saturating_add(1));
        fixed.saturating_add(
            candidates.saturating_mul(evaluation_work(op).saturating_add(tree_entry_work(output))),
        )
    }

    fn literal_quote_work(&self, _literal: U256) -> usize {
        LITERAL_WORK
    }

    fn byte_quotes_uniform(&self) -> bool {
        // Numeric quotes depend on cached operand cardinalities, never on the
        // literal index. Facade metadata likewise sees an exact one-word index.
        true
    }

    fn projection_work(&self, _value: &Self::Element) -> usize {
        // A cached cardinality check and at most one shared-handle clone.
        2 + COPY_WORK
    }

    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize {
        if facts == 0 {
            return self.projection_work(value);
        }
        // Without an actual payload size, reserve the entire admitted fact
        // capacity. Oversized MemberOf facts are rejected before traversal by
        // the common lattice; staged callers use the actual input size below.
        self.import_work(value, facts, self.spec.fact_limit(), 0)
    }

    fn refinement_import_work(
        &self,
        value: &Self::Element,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> Option<usize> {
        Some(self.import_work(value, facts, finite_values, expression_work))
    }
}

impl WordElement for FiniteConstantSet {
    fn top() -> Self {
        Self::top()
    }

    fn constant(value: U256) -> Self {
        Self::constant(value)
    }

    fn from_observation(observation: &WordObservation) -> Self {
        if !observation.finite_constants().is_top() {
            // The observation's finite membership is complete, but its other
            // guarantees can exclude candidates from that cover.
            return observation
                .finite_constants()
                .filter(|word| observation.contains(word))
                .unwrap_or_else(|_| Self::top());
        }
        observation
            .singleton()
            .map_or_else(Self::top, Self::constant)
    }

    fn observation(&self) -> WordObservation {
        WordObservation {
            finite: self.clone(),
            ..WordObservation::top()
        }
    }

    fn requires_retained_binding(&self) -> bool {
        // Unlike arbitrary custom predicates, this exact finite disjunction is
        // fully expressible in WordObservation without a product conversion.
        false
    }

    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        let candidates = if self.is_top() {
            observation.finite_constants()
        } else {
            self
        };
        if candidates.is_top() {
            // Unsupported nonsingleton masks/ranges/congruences cannot be
            // represented by this carrier. Keep Top rather than pruning them.
            return Ok(observation
                .singleton()
                .map_or_else(Self::top, Self::constant));
        }
        candidates
            .filter(|word| observation.contains(word))
            .map_err(|_| FactError::Contradiction {
                subject: Symbol::THIS,
            })
    }

    fn constrain_work(&self, observation: &WordObservation) -> usize {
        let candidates = self
            .cardinality()
            .or_else(|| observation.finite_constants().cardinality())
            .unwrap_or(1);
        let membership = tree_entry_work(observation.finite_constants().work_size());
        64usize.saturating_add(
            candidates.saturating_mul(
                256usize
                    .saturating_add(membership)
                    .saturating_add(tree_entry_work(candidates)),
            ),
        )
    }

    fn finite_constants(&self) -> &FiniteConstantSet {
        self
    }

    fn contains(&self, word: U256) -> bool {
        self.contains(word)
    }

    fn singleton(&self) -> Option<U256> {
        self.singleton()
    }

    fn is_top(&self) -> bool {
        self.is_top()
    }

    fn work_size(&self) -> usize {
        // Four word limbs plus iteration/serialization and tree-search work.
        self.work_size().saturating_mul(16).saturating_add(1)
    }

    fn clone_work(&self) -> usize {
        COPY_WORK
    }

    fn singleton_work(&self) -> usize {
        if self.cardinality() == Some(1) {
            SINGLETON_WORK
        } else {
            // Top and nonsingletons are decided from their cached shape only.
            2
        }
    }
}

#[cfg(test)]
mod tests;
