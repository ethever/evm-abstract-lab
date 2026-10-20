//! Private default numeric primitives. These never call the metadata facade.

use super::{
    Domain, EvmValue, FiniteConstantSet, NumericReduction, NumericValue, Profile, WordElement,
    known_bits::KnownBits, query,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

pub(super) fn finite(finite: FiniteConstantSet) -> NumericValue {
    let Some(values) = finite.as_values() else {
        return NumericValue::top();
    };
    NumericValue {
        bits: KnownBits::from_values(values),
        interval: super::interval::Interval::from_values(values),
        congruence: super::congruence::Congruence::from_values(values),
        nonzero: !values.contains(&U256::ZERO),
        finite,
    }
}

impl Domain {
    pub(super) fn numeric_memory_write_passes(
        self,
        offset: &EvmValue<NumericValue>,
        value: &EvmValue<NumericValue>,
        width: usize,
    ) -> usize {
        let numeric = value.numeric();
        // These are exactly singleton()'s candidate tags, without its possibly
        // expensive membership/modulo validation. No candidate means the writer
        // cannot enter its singleton-provenance loop.
        let candidate = numeric.finite.cardinality() == Some(1)
            || numeric.bits.singleton().is_some()
            || numeric.interval.singleton().is_some()
            || numeric.congruence.singleton().is_some();
        let constant_origin = value
            .provenance()
            .origins()
            .sources()
            .is_some_and(|origins| {
                origins.len() == 1 && origins.contains(&super::provenance::Origin::Constant)
            })
            && !value.provenance().is_code_address();
        let source_pass = usize::from(candidate && !constant_origin);
        // NumericValue cloning/normalized offset retain this raw complete set.
        // A single last-key lookup bounds every host conversion and width add.
        let cached = offset.constants().is_some_and(|values| {
            values.last().is_none_or(|last| {
                usize::try_from(*last)
                    .ok()
                    .and_then(|start| start.checked_add(width.saturating_sub(1)))
                    .is_some()
            })
        });
        source_pass + usize::from(!cached)
    }
    pub(super) fn numeric_project(self, value: &NumericValue) -> NumericValue {
        if self.spec().profile() == Profile::Product {
            let mut out = value.clone();
            out.finite = out.finite.into_limited(self.spec().constant_capacity());
            return out;
        }
        query::candidates(value, self.capacity())
            .filter(|values| !values.is_empty() && values.len() <= self.capacity())
            .map_or_else(NumericValue::top, |values| {
                finite(FiniteConstantSet::try_from_values(values).expect("nonempty candidates"))
            })
    }

    pub(super) fn numeric_join(self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        let candidates = left
            .finite
            .join(&right.finite, self.spec().constant_capacity());
        if self.spec().profile() == Profile::ConstantsOnly {
            return finite(candidates);
        }
        NumericValue {
            finite: candidates,
            bits: left.bits.join(&right.bits),
            interval: left.interval.join(&right.interval),
            congruence: left.congruence.join(&right.congruence),
            nonzero: left.nonzero && right.nonzero,
        }
    }

    pub(super) fn numeric_widen(self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        let mut out = next.clone();
        if self.spec().profile() == Profile::Product {
            out.interval = old.interval.widen(&next.interval);
        }
        out
    }

    pub(super) fn numeric_widen_procedure(
        self,
        old: &NumericValue,
        next: &NumericValue,
    ) -> NumericValue {
        if old == next
            || next.constants().is_some_and(|values| {
                values.len() <= self.spec().widening_after_updates().saturating_add(1)
            })
        {
            return next.clone();
        }
        if self.spec().profile() == Profile::ConstantsOnly {
            return NumericValue::top();
        }
        let previous = old.interval.unsigned_bounds();
        let joined = next.interval.unsigned_bounds();
        if previous == joined {
            return next.clone();
        }
        let mut out = self.numeric_widen(old, next);
        let widened = out.interval.unsigned_bounds();
        if joined != widened {
            let implied = KnownBits::from_unsigned_bounds(joined.0, joined.1);
            let retained = KnownBits::from_unsigned_bounds(widened.0, widened.1);
            let discard = (implied.zero() | implied.one()) & !(retained.zero() | retained.one());
            out.bits = KnownBits::new(next.bits.zero() & !discard, next.bits.one() & !discard)
                .expect("removing guarantees preserves disjoint masks");
            out.finite = FiniteConstantSet::top();
        }
        out
    }

    pub(super) fn numeric_finite_apply(self, op: u8, args: &[NumericValue]) -> NumericValue {
        self.numeric_finite_apply_borrowed(op, args.iter())
    }

    fn numeric_finite_apply_borrowed<'a>(
        self,
        op: u8,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> NumericValue {
        let collect = |values| {
            finite(
                FiniteConstantSet::collect_bounded(values, self.spec().constant_capacity())
                    .expect("reachable finite values are nonempty"),
            )
        };
        let Some(sets) = args
            .map(NumericValue::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                finite(
                    FiniteConstantSet::collect_bounded(
                        (0u64..=256).map(U256::from),
                        self.spec().constant_capacity(),
                    )
                    .unwrap(),
                )
            } else if matches!(op, 0x10..=0x15) {
                finite(
                    FiniteConstantSet::collect_bounded(
                        [U256::ZERO, U256::ONE],
                        self.spec().constant_capacity(),
                    )
                    .unwrap(),
                )
            } else {
                NumericValue::top()
            };
        };
        let singleton = BTreeSet::from([U256::ZERO]);
        let second = sets.get(1).copied().unwrap_or(&singleton);
        let third = sets.get(2).copied().unwrap_or(&singleton);
        collect(sets[0].iter().flat_map(|a| {
            second
                .iter()
                .flat_map(move |b| third.iter().map(move |c| super::evaluate(op, *a, *b, *c)))
        }))
    }

    pub(super) fn numeric_apply(
        self,
        op: u8,
        args: &[NumericValue],
    ) -> NumericReduction<NumericValue> {
        self.numeric_apply_with(op, args, |value| super::reduce::numeric(self, value))
    }

    pub(super) fn numeric_borrowed_copy_work<'a>(
        self,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        // The legacy constants-only tuple envelope already covers scalar reads
        // and result metadata. Borrowing adds no operand materialization there.
        if self.spec().profile() == Profile::ConstantsOnly {
            return 0;
        }
        let (tuples, finite, copies) =
            args.fold((1usize, true, 0usize), |(tuples, finite, copies), value| {
                (
                    tuples.saturating_mul(value.constants().map_or(1, BTreeSet::len)),
                    finite && value.constants().is_some(),
                    copies.saturating_add(value.clone_work()),
                )
            });
        if finite && tuples <= self.capacity() {
            0
        } else {
            copies
        }
    }

    pub(super) fn numeric_apply_borrowed<'a>(
        self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericReduction<NumericValue> {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return NumericReduction::stable(NumericValue::top());
        }
        let first = args.next().expect("validated nonempty scalar operands");
        let operands = [
            first,
            args.next().unwrap_or(first),
            args.next().unwrap_or(first),
        ];
        let operands = &operands[..expected];
        if self.numeric_borrowed_copy_work(operands.iter().copied()) == 0 {
            if self.spec().profile() == Profile::Product {
                if op == opcode::ISZERO && (!first.may_be_zero() || !first.may_be_nonzero()) {
                    return NumericReduction::stable(NumericValue::constant(U256::from(u8::from(
                        !first.may_be_nonzero(),
                    ))));
                }
                // A raw finite cover can carry a stricter singleton in another
                // component. Preserve transfer's exact-scalar check before using
                // its otherwise equivalent complete finite enumeration shortcut.
                let mut exact = [U256::ZERO; 3];
                if operands.iter().enumerate().all(|(i, value)| {
                    value.singleton().is_some_and(|word| {
                        exact[i] = word;
                        true
                    })
                }) {
                    return NumericReduction::stable(NumericValue::constant(super::evaluate(
                        op, exact[0], exact[1], exact[2],
                    )));
                }
            }
            return NumericReduction::stable(
                self.numeric_finite_apply_borrowed(op, operands.iter().copied()),
            );
        }
        // This remaining path and custom implementations retain their complete
        // prepaid materialization cost; no default-product rule is bypassed.
        self.numeric_apply(
            op,
            &super::algebra::clone_numeric_operands(operands.iter().copied()),
        )
    }

    pub(super) fn numeric_observe_byte_borrowed<'a>(
        self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericValue {
        if !matches!(op, opcode::BYTE | opcode::AND) || args.len() != 2 {
            return self.numeric_apply_borrowed(op, args).value;
        }
        let operands = [args.next().unwrap(), args.next().unwrap()];
        if self.numeric_borrowed_copy_work(operands.into_iter()) == 0 {
            return self.numeric_apply_borrowed(op, operands.into_iter()).value;
        }
        super::byte_projection::observe(
            self,
            op,
            &super::algebra::clone_numeric_operands(operands.into_iter()),
        )
    }
    pub(super) fn numeric_apply_with(
        self,
        op: u8,
        args: &[NumericValue],
        reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
    ) -> NumericReduction<NumericValue> {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return NumericReduction::stable(NumericValue::top());
        }
        if self.spec().profile() == Profile::ConstantsOnly {
            return NumericReduction::stable(self.numeric_finite_apply(op, args));
        }
        super::transfer::apply(self, op, args, reduce)
    }

    pub(super) fn numeric_projection_work(self, value: &NumericValue) -> usize {
        if self.spec().profile() == Profile::Product {
            return 0;
        }
        query::candidate_visits(value, self.capacity()).saturating_mul(256)
    }

    pub(super) fn numeric_refinement_work(self, value: &NumericValue, facts: usize) -> usize {
        self.numeric_refinement_work_with(value, facts, None)
    }

    pub(super) fn numeric_refinement_work_with(
        self,
        value: &NumericValue,
        facts: usize,
        closure_work: Option<usize>,
    ) -> usize {
        let atoms = self.capacity().saturating_add(32);
        // Refinement cannot grow an existing finite cover. After the first
        // scalar pass its remaining candidates imply every exported summary;
        // the second pass only confirms those publications. A canonical literal
        // already has all summaries. Top is unchanged only without new facts.
        let rounds = if canonical_literal(value) || (facts == 0 && stable_scalar(self, value)) {
            Some(1)
        } else if value.constants().is_some() {
            Some(2)
        } else if facts == 0
            && value.congruence.is_top()
            && (value.bits == KnownBits::top()
                || value.interval == super::interval::Interval::top())
        {
            // A sole mask or interval has no competing endpoint constraint.
            // Its complete cover is either available immediately or stays over
            // capacity; the second cycle confirms any summaries/nonzero flag.
            Some(2)
        } else if facts == 0 && dyadic_congruence(value) {
            Some(3)
        } else {
            None
        };
        let finite = if value.constants().is_some() || (facts == 0 && stable_scalar(self, value)) {
            value.finite.work_size()
        } else {
            self.capacity().max(value.finite.work_size())
        };
        // A custom closure owns its complete allowance; built-in convergence
        // shortcuts cannot clip it. Import/copy work remains outside that cap.
        let closure = closure_work.unwrap_or_else(|| self.numeric_closure_work(finite, rounds));
        let facts = facts
            .saturating_add(1)
            .min(self.spec().fact_limit().saturating_add(1));
        4096usize
            .saturating_add(closure)
            .saturating_add(
                facts
                    .saturating_mul(facts.saturating_add(atoms))
                    .saturating_mul(32),
            )
            .saturating_add(value.work_size())
    }

    pub(super) fn numeric_import_work(
        self,
        value: &NumericValue,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> usize {
        // Keep the old fixed-word import and quadratic fact-normalization
        // envelope. Payload visits are the actual borrowed input/member count,
        // rather than the unrelated future scalar capacity/round multiplier.
        if self.numeric_closure_work(self.capacity(), Some(1)) == usize::MAX {
            return usize::MAX;
        }
        let finite = finite_values.max(value.finite.work_size());
        4096usize
            .saturating_add(256usize.saturating_mul(finite))
            .saturating_add(
                facts
                    .saturating_mul(facts.saturating_add(finite).saturating_add(32))
                    .saturating_mul(32),
            )
            .saturating_add(expression_work)
            .saturating_add(value.work_size())
    }

    fn numeric_closure_work(self, finite: usize, rounds: Option<usize>) -> usize {
        if self
            .spec()
            .reduction_rounds()
            .checked_mul(3)
            .and_then(|steps| steps.checked_add(1))
            .is_none()
        {
            return usize::MAX;
        }
        // One scope, three singleton channels, four jobs, ten live read edges.
        // Each query costs at most 9 (intern) + 28 (two tracked reads) + value.
        // A changed publication costs 9 + policy + value + 38: at most three
        // readers, each reserving a 12-unit enqueue, plus value/version writes.
        // Selection/handle/begin/finish costs at most 7 per non-seed evaluation.
        let mask = 256usize + 7 + (9 + 28 + 8) + (9 + 16 + 8 + 38);
        let bounds =
            (4096usize + 7 + 3 * (9 + 28) + 3 * 8 + 9 + 128 + 8 + 38).saturating_add(finite);
        // ScalarRule reserves the configured capacity even for a singleton;
        // channel copying/meeting uses the actual retained cover bound instead.
        // Public oversized covers also need their full arithmetic/fact envelope.
        let scalar_capacity = self.capacity().max(finite);
        let scalar = 4096usize
            .saturating_add(256usize.saturating_mul(scalar_capacity))
            .saturating_add(32usize.saturating_mul(scalar_capacity.saturating_add(32)))
            // Three queries and three publications, including ScalarMeet's
            // 64 * (old finite + new finite + 16) reservation exactly once.
            .saturating_add(1475)
            .saturating_add(130usize.saturating_mul(finite));
        // Setup/seed, its first publications and the fixed-size numeric copies.
        let seed = 256usize.saturating_add(finite);
        if let Some(rounds) = rounds {
            return seed.saturating_add(
                mask.saturating_add(bounds)
                    .saturating_add(scalar)
                    .saturating_mul(rounds.min(self.spec().reduction_rounds())),
            );
        }
        // solve permits 1 + 3*rounds evaluations. The seed runs once. Every
        // scalar rerun is preceded by a bounds evaluation: all scalar reads are
        // also bounds reads, and a shared wake queues bounds before scalar.
        // Scalar is the most expensive job, bounds next, mask cheapest. Reserve
        // the maximum weighted schedule, retaining the initial mask evaluation.
        let steps = self.spec().reduction_rounds().saturating_mul(3);
        let scalars = steps.saturating_sub(1) / 2;
        let bounds_runs = steps.saturating_sub(1).saturating_sub(scalars);
        seed.saturating_add(mask)
            .saturating_add(scalar.saturating_mul(scalars))
            .saturating_add(bounds.saturating_mul(bounds_runs))
    }

    pub(super) fn numeric_operation_work_for<'a>(
        self,
        op: u8,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        let mut args = args.fuse();
        let a = args.next();
        let b = args.next();
        let c = args.next();
        let quote = self.numeric_operation_work(a.into_iter().chain(b).chain(c).chain(args));
        if self.spec().profile() != Profile::Product || quote == usize::MAX {
            return quote;
        }
        let closed = match (a, b, c) {
            (Some(a), None, None) if a.is_top() && matches!(op, opcode::NOT | opcode::ISZERO) => {
                // NOT yields Top; ISZERO yields the already aligned boolean
                // product (finite two or finite Top at capacity one).
                Some((
                    3,
                    if op == opcode::ISZERO {
                        self.capacity().min(2)
                    } else {
                        1
                    },
                ))
            }
            (Some(a), Some(b), None) => {
                let literal_top =
                    (canonical_literal(a) && b.is_top()) || (a.is_top() && canonical_literal(b));
                if literal_top && matches!(op, opcode::ADD | opcode::SUB) {
                    // Translating all Word256 values by a literal is a bijection;
                    // all three component transfers are Top, with no facts to add.
                    Some((2, 1))
                } else if self.capacity() < 256
                    && ((op == opcode::BYTE
                        && canonical_literal(a)
                        && a.finite
                            .singleton()
                            .is_some_and(|index| index < U256::from(32))
                        && b.is_top())
                        || (op == opcode::AND
                            && a.is_top()
                            && canonical_literal(b)
                            && b.finite.singleton() == Some(U256::from(255))))
                {
                    // The unconstrained-byte cube is already aligned, and its
                    // 256 values cannot be reconstructed at this capacity.
                    Some((2, 1))
                } else if op == opcode::EQ && (address_literal_eq(a, b) || address_literal_eq(b, a))
                {
                    // Each component admits the literal in the complete address
                    // cube, so EQ's finite/mask/range outputs are the same
                    // already aligned boolean. No comparison rule is removed.
                    Some((3, self.capacity().min(2)))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((old_rounds, finite)) = closed {
            quote
                .saturating_sub(self.numeric_closure_work(self.capacity(), Some(old_rounds)))
                .saturating_add(self.numeric_closure_work(finite, Some(1)))
        } else {
            quote
        }
    }

    pub(super) fn numeric_operation_work<'a>(
        self,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        self.numeric_operation_work_with(args, None)
    }

    pub(super) fn numeric_operation_work_with<'a>(
        self,
        args: impl Iterator<Item = &'a NumericValue>,
        closure_work: Option<usize>,
    ) -> usize {
        let (tuples, all_finite, size, len, shapes, dyadic) = args.fold(
            (1usize, true, 0usize, 0usize, 0u8, true),
            |(tuples, all, size, len, shapes, dyadic), v| {
                let shape = if v.is_top() {
                    1
                } else if canonical_literal(v) {
                    2
                } else {
                    4
                };
                (
                    tuples.saturating_mul(v.constants().map_or(1, BTreeSet::len)),
                    all && v.constants().is_some(),
                    size.saturating_add(v.work_size()),
                    len.saturating_add(1),
                    shapes | shape,
                    dyadic && dyadic_congruence(v),
                )
            },
        );
        if self.spec().profile() == Profile::ConstantsOnly {
            return tuples;
        }
        if len > 0 && all_finite && tuples <= self.capacity() {
            return tuples.saturating_mul(256).saturating_add(size);
        }
        // For {canonical literal, Top}, binary transfer yields a range, mask
        // cube, boolean or exact word. Multiplication's congruence is a power
        // of two (wrapped modulo Word256), already implied by the mask. The
        // other non-exact results have unconstrained congruence. Bounds and
        // low-bit congruence inferred from these shapes are compatible with
        // their endpoints: one scalar pass aligns their summaries/optional
        // complete cover; a second confirms it, in either operand order.
        let rounds = if len == 2 && shapes == 3 {
            Some(2)
        } else if len > 0 && dyadic {
            // Transfer of dyadic/Top/exact inputs stays dyadic unless complete
            // finite enumeration has already returned. Projecting congruence
            // endpoints then masks needs at most two bounds passes: the second
            // dyadic adjustment changes only compatible low bits. A newly
            // reconstructed finite cover can require one final confirmation.
            Some(3)
        } else {
            None
        };
        tuples
            .saturating_add(4096)
            .saturating_add(
                closure_work.unwrap_or_else(|| self.numeric_closure_work(self.capacity(), rounds)),
            )
            .saturating_add(size)
    }
}

// Inspect only the singleton cover and inline summaries; no candidate scan,
// normalization, constraint import or operand clone belongs in a quotation.
fn canonical_literal(value: &NumericValue) -> bool {
    value.finite.singleton().is_some_and(|word| {
        value.bits == KnownBits::exact(word)
            && value.interval == super::interval::Interval::exact(word)
            && value.congruence.singleton() == Some(word)
            && value.nonzero == (word != U256::ZERO)
    })
}

fn address_literal_eq(cube: &NumericValue, literal: &NumericValue) -> bool {
    let max = U256::MAX >> 96usize;
    let sign = U256::ONE << 255usize;
    cube.finite.is_top()
        && cube.congruence.is_top()
        && !cube.nonzero
        && cube.bits.zero() == !max
        && cube.bits.one() == U256::ZERO
        && cube.interval.unsigned_bounds() == (U256::ZERO, max)
        && cube.interval.signed_bounds() == (sign, sign + max)
        && canonical_literal(literal)
        && literal.finite.singleton().is_some_and(|word| word <= max)
}

fn stable_scalar(domain: Domain, value: &NumericValue) -> bool {
    if !value.finite.is_top() || !value.congruence.is_top() {
        return false;
    }
    if value.bits == KnownBits::top() && value.interval == super::interval::Interval::top() {
        // The nonzero flag alone is already closed; no rule clips zero away
        // from an otherwise unconstrained interval.
        return true;
    }
    let (lo, hi) = value.bits.unsigned_bounds();
    let unknown = (!(value.bits.zero() | value.bits.one())).count_ones();
    value.interval == super::interval::Interval::new_unsigned(lo, hi).unwrap()
        && !(value.bits.zero() | value.bits.one()).bit(0)
        && value.nonzero == (value.bits.one() != U256::ZERO)
        && (unknown >= usize::BITS as usize
            || (1usize << unknown) > domain.capacity().min(domain.spec().fact_limit()))
}

fn dyadic_congruence(value: &NumericValue) -> bool {
    value.congruence.is_top()
        || value.congruence.singleton().is_some()
        || value
            .congruence
            .modulus_residue()
            .is_some_and(|(modulus, _)| modulus.is_power_of_two())
}

#[cfg(test)]
mod cost_tests;
