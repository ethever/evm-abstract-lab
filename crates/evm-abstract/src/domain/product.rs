//! Private default numeric primitives. These never call the metadata facade.

use super::{
    DomainSpec, EvmValue, FiniteConstantSet, NumericReduction, NumericValue, known_bits::KnownBits,
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

impl DomainSpec {
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
        let mut out = value.clone();
        out.finite = out.finite.into_limited(self.constant_capacity());
        out
    }

    pub(super) fn numeric_join(self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        let candidates = left.finite.join(&right.finite, self.constant_capacity());
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
        out.interval = old.interval.widen(&next.interval);
        out
    }

    pub(super) fn numeric_widen_procedure(
        self,
        old: &NumericValue,
        next: &NumericValue,
    ) -> NumericValue {
        if old == next
            || next.constants().is_some_and(|values| {
                values.len() <= self.widening_after_updates().saturating_add(1)
            })
        {
            return next.clone();
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

    pub(super) fn numeric_finite_apply(self, op: u8, args: &[&NumericValue]) -> NumericValue {
        let collect = |values| {
            finite(
                FiniteConstantSet::collect_bounded(values, self.constant_capacity())
                    .expect("reachable finite values are nonempty"),
            )
        };
        let Some(sets) = args
            .iter()
            .copied()
            .map(NumericValue::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                finite(
                    FiniteConstantSet::collect_bounded(
                        (0u64..=256).map(U256::from),
                        self.constant_capacity(),
                    )
                    .unwrap(),
                )
            } else if matches!(op, 0x10..=0x15) {
                finite(
                    FiniteConstantSet::collect_bounded(
                        [U256::ZERO, U256::ONE],
                        self.constant_capacity(),
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

    pub(super) fn numeric_apply_with<'a>(
        self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a NumericValue>,
        reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
    ) -> NumericReduction<NumericValue> {
        let Some(expected) = super::concrete::arity(op).filter(|count| *count == args.len()) else {
            return NumericReduction::stable(NumericValue::top());
        };
        // EVM pure operations have at most three borrowed operands; the fixed
        // reference array retains the original numeric values without cloning.
        let first = args.next().expect("validated nonempty scalar operands");
        let operands = [
            first,
            args.next().unwrap_or(first),
            args.next().unwrap_or(first),
        ];
        let operands = &operands[..expected];
        super::transfer::apply(self, op, operands, reduce)
    }

    pub(super) fn numeric_observe_byte_with<'a>(
        self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a NumericValue>,
        reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
    ) -> NumericValue {
        if !matches!(op, opcode::BYTE | opcode::AND) || args.len() != 2 {
            return self.numeric_apply_with(op, args, reduce).value;
        }
        let operands = [args.next().unwrap(), args.next().unwrap()];
        super::byte_projection::observe_with(op, &operands, || {
            self.numeric_apply_with(op, operands.into_iter(), reduce)
                .value
        })
    }

    // A complete closed certificate replaces a cooperation kernel only when the
    // original policy admits it. Constrained fact limits and overflow retain the
    // original status/refusal path. This also funds the certificate, up to three
    // import/reduction/project copies and their retained/released native payloads.
    pub(super) fn native_closed_scalar_work(self, finite: usize) -> Option<usize> {
        if self.fact_limit() < 8
            || self.numeric_closure_work(self.capacity().max(finite), Some(1)) == usize::MAX
        {
            return None;
        }
        Some(576usize.saturating_add(finite.saturating_add(24).saturating_mul(3)))
    }

    pub(super) fn native_scalar_is_closed(self, value: &NumericValue) -> bool {
        self.native_closed_scalar_work(value.finite.work_size())
            .is_some()
            && (canonical_literal(value)
                || (stable_scalar(self, value)
                    && (value.bits == KnownBits::top()
                        || (value.bits.zero() | value.bits.one()).bit(255)))
                || (self.capacity() >= 2
                    && value.finite.as_values().is_some_and(|set| {
                        set.len() == 2
                            && set.first() == Some(&U256::ZERO)
                            && set.last() == Some(&U256::ONE)
                    })
                    && value.bits == KnownBits::new(!U256::ONE, U256::ZERO).unwrap()
                    && value.interval
                        == super::interval::Interval::new_unsigned(U256::ZERO, U256::ONE).unwrap()
                    && value.congruence.is_top()
                    && !value.nonzero))
    }

    // These complete input shapes have one feasible word. A full builtin
    // kernel publishes its canonical fields and confirms them in two cycles.
    // Keep smaller schedules/fact policies and every unproved component shape
    // on the original path. No candidate search or modular query is needed.
    pub(super) fn native_scalar_literal(self, value: &NumericValue) -> Option<U256> {
        if self.reduction_rounds() < 2 || self.native_closed_scalar_work(1).is_none() {
            return None;
        }
        let word = if value.finite.is_top() && value.congruence.is_top() {
            value
                .bits
                .singleton()
                .or_else(|| value.interval.singleton())
                .or_else(|| {
                    (self.capacity() >= 2
                        && value.nonzero
                        && value.interval.unsigned_bounds() == (U256::ZERO, U256::ONE))
                        .then_some(U256::ONE)
                })?
        } else {
            let word = value.finite.singleton()?;
            if value.bits != KnownBits::exact(word)
                || value.interval != super::interval::Interval::exact(word)
                || value.congruence.singleton() != Some(word)
            {
                return None;
            }
            word
        };
        (value.bits.contains(word)
            && value.interval.contains(word)
            && (!value.nonzero || word != U256::ZERO))
            .then_some(word)
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
            .min(self.fact_limit().saturating_add(1));
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
        // Fixed-word import retains the quadratic fact-normalization
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

    pub(super) fn numeric_closure_work(self, finite: usize, rounds: Option<usize>) -> usize {
        use super::cooperation::{BoundsToMask, MaskToBounds, RuleWork, cost};

        if self
            .reduction_rounds()
            .checked_mul(3)
            .and_then(|steps| steps.checked_add(1))
            .is_none()
        {
            return usize::MAX;
        }
        // Share protocol/query/publication/report tariffs with ProductAlgebra.
        // Only the scheduling proof differs: these exact built-in rules permit
        // the input-sensitive cycle bounds and weighted schedule below.
        let mask = BoundsToMask
            .evaluation_work(self, finite)
            .saturating_add(cost::STEP_WORK);
        let bounds = MaskToBounds
            .evaluation_work(self, finite)
            .saturating_add(cost::STEP_WORK);
        let scalar = cost::scalar_work(self, finite).saturating_add(cost::STEP_WORK);
        let fixed = cost::fixed_work(finite);
        if let Some(rounds) = rounds {
            return fixed.saturating_add(
                mask.saturating_add(bounds)
                    .saturating_add(scalar)
                    .saturating_mul(rounds.min(self.reduction_rounds())),
            );
        }
        // solve permits 1 + 3*rounds evaluations. The seed runs once. Every
        // scalar rerun is preceded by a bounds evaluation: all scalar reads are
        // also bounds reads, and a shared wake queues bounds before scalar.
        // Scalar is the most expensive job, bounds next, mask cheapest. Reserve
        // the maximum weighted schedule, retaining the initial mask evaluation.
        let steps = self.reduction_rounds().saturating_mul(3);
        let scalars = steps.saturating_sub(1) / 2;
        let bounds_runs = steps.saturating_sub(1).saturating_sub(scalars);
        fixed
            .saturating_add(mask)
            .saturating_add(scalar.saturating_mul(scalars))
            .saturating_add(bounds.saturating_mul(bounds_runs))
    }

    pub(super) fn numeric_operation_work<'a>(
        self,
        op: u8,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        let mut args = args.fuse();
        let a = args.next();
        let b = args.next();
        let c = args.next();
        let quote =
            self.numeric_transfer_work(op, a.into_iter().chain(b).chain(c).chain(args), None);
        if quote == usize::MAX {
            return quote;
        }
        let closed = match (a, b, c) {
            (Some(a), None, None) if a.is_top() && matches!(op, opcode::NOT | opcode::ISZERO) => {
                // NOT yields Top; ISZERO yields the already aligned boolean
                // product (finite two or finite Top at capacity one).
                Some((
                    if op == opcode::ISZERO { 2 } else { 3 },
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
                if op == opcode::AND
                    && self.reduction_rounds() >= 2
                    && self.native_closed_scalar_work(1).is_some()
                    && [(a, b), (b, a)].into_iter().any(|(value, mask)| {
                        let Some(mask_word) =
                            mask.finite.singleton().filter(|_| canonical_literal(mask))
                        else {
                            return false;
                        };
                        let zero = value.bits.zero() | !mask_word;
                        let one = value.bits.one() & mask_word;
                        value.finite.is_top()
                            && value.congruence.is_top()
                            // Exact interval operands take Interval::transfer's
                            // concrete path, not its [0, min_hi] AND bound. Keep
                            // that path's original invoice even if mask fields
                            // alone imply a different singleton.
                            && value.interval.singleton().is_none()
                            && mask_word != U256::ZERO
                            && mask_word != U256::MAX
                            && zero | one == U256::MAX
                            && one <= value.interval.unsigned_bounds().1.min(mask_word)
                    })
                {
                    // Raw AND has an exact mask, finite/congruence Top and a
                    // containing interval. native_scalar_literal constructs its
                    // canonical word, replacing all actual cooperation jobs.
                    return quote
                        .saturating_sub(self.numeric_closure_work(self.capacity(), Some(3)))
                        .saturating_add(self.native_closed_scalar_work(1).unwrap());
                } else if literal_top && matches!(op, opcode::ADD | opcode::SUB) {
                    // Translating all Word256 values by a literal is a bijection;
                    // all three component transfers are Top, with no facts to add.
                    Some((2, 1))
                } else if literal_top
                    && (op == opcode::EQ
                        || (matches!(op, opcode::LT | opcode::GT)
                            && [a, b].into_iter().any(|value| {
                                canonical_literal(value)
                                    && value.finite.singleton().is_some_and(|literal| {
                                        literal != U256::ZERO && literal != U256::MAX
                                    })
                            })))
                {
                    // Every component already describes the same boolean:
                    // finite {0,1} (or finite Top below capacity two), one-bit
                    // masks/ranges and Top congruence. The one builtin pass
                    // confirms it; endpoint comparisons keep the ordinary path.
                    Some((2, self.capacity().min(2)))
                } else if (self.capacity() < 256
                    && op == opcode::BYTE
                    && canonical_literal(a)
                    && a.finite
                        .singleton()
                        .is_some_and(|index| index < U256::from(32))
                    && b.is_top())
                    || (op == opcode::AND
                        && [(a, b), (b, a)].into_iter().any(|(value, mask)| {
                            value.is_top()
                                && canonical_literal(mask)
                                && mask.finite.singleton().is_some_and(|mask| {
                                    let width = mask.count_ones();
                                    mask != U256::ZERO
                                        && mask & mask.wrapping_add(U256::ONE) == U256::ZERO
                                        && (width >= usize::BITS as usize
                                            || (1usize << width) > self.capacity())
                                })
                        }))
                {
                    // These complete byte/low-mask cubes are already aligned;
                    // their finite images cannot be rebuilt at this capacity.
                    Some((2, 1))
                } else if op == opcode::EQ && (address_literal_eq(a, b) || address_literal_eq(b, a))
                {
                    // Each component admits the literal in the complete address
                    // cube, so EQ's finite/mask/range outputs are the same
                    // already aligned boolean. No comparison rule is removed.
                    Some((2, self.capacity().min(2)))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((old_rounds, finite)) = closed {
            let boolean = matches!(op, 0x10..=0x15)
                && ((op == opcode::ISZERO && a.is_some() && b.is_none())
                    || (op != opcode::ISZERO && a.is_some() && b.is_some() && c.is_none()));
            let input_finite = if boolean {
                self.capacity().min(2)
            } else {
                self.capacity()
            };
            // Each positive raw-output case above is the complete Top, boolean
            // or low-bit cube recognized by native_scalar_is_closed. Its actual
            // reducer returns a paid native copy without constructing a kernel.
            // Keep tuple visits, raw component evaluation, facade fold and input
            // work; replace only the closure that is actually omitted.
            let closure = self
                .native_closed_scalar_work(finite)
                .unwrap_or_else(|| self.numeric_closure_work(finite, Some(1)));
            quote
                .saturating_sub(self.numeric_closure_work(input_finite, Some(old_rounds)))
                .saturating_add(closure)
        } else {
            quote
        }
    }

    pub(super) fn numeric_transfer_work<'a>(
        self,
        op: u8,
        args: impl Iterator<Item = &'a NumericValue>,
        closure_work: Option<usize>,
    ) -> usize {
        let (tuples, all_finite, size, len, shapes, dyadic, exact_components) = args.fold(
            (1usize, true, 0usize, 0usize, 0u8, true, 0b111u8),
            |(tuples, all, size, len, shapes, dyadic, exact_components), v| {
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
                    exact_components
                        & (u8::from(v.bits.singleton().is_some())
                            | (u8::from(v.interval.singleton().is_some()) << 1)
                            | (u8::from(v.congruence.singleton().is_some()) << 2)),
                )
            },
        );
        let supported = super::concrete::arity(op) == Some(len);
        let evaluation = super::concrete::evaluation_work(op);
        // Keep the existing 256-unit cheap tuple envelope, replacing its
        // 64-unit concrete evaluation with the selected opcode's real bound.
        let tuple_work = 256usize.saturating_add(if supported {
            evaluation.saturating_sub(64)
        } else {
            0
        });
        if len > 0 && all_finite && tuples <= self.capacity() {
            // A single tuple always produces a singleton, so the facade skips
            // expression evaluation. A complete larger image may still fold
            // one retained expression after all numeric evaluations finish.
            let fold = if supported && tuples > 1 {
                evaluation
            } else {
                0
            };
            return tuples
                .saturating_mul(tuple_work)
                .saturating_add(size)
                .saturating_add(fold);
        }
        // For {canonical literal, Top}, binary transfer yields a range, mask
        // cube, boolean or exact word. Multiplication's congruence is a power
        // of two (wrapped modulo Word256), already implied by the mask. The
        // other non-exact results have unconstrained congruence. Bounds and
        // low-bit congruence inferred from these shapes are compatible with
        // their endpoints: one scalar pass aligns their summaries/optional
        // complete cover; a second confirms it, in either operand order.
        let boolean = supported && matches!(op, 0x10..=0x15);
        let rounds = if boolean {
            // Each raw component admits only zero/one. One alternative can be
            // removed and propagated once; a second builtin cycle confirms it.
            Some(2)
        } else if supported && op == opcode::BYTE {
            // Raw BYTE has a byte cube and a full-byte/zero interval, without
            // an independent modular constraint. One alignment/cover pass and
            // its confirmation suffice; enumeration above retains all tuples.
            Some(2)
        } else if len == 2 && shapes == 3 {
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
        // Output capacity limits distinct retained results, not tuple visits.
        // Colliding images can require the entire Cartesian product even when
        // tuples exceed capacity. Unknown operands admit no finite enumeration.
        let visits = if supported && all_finite {
            tuples.saturating_mul(tuple_work)
        } else {
            tuples
        };
        // Raw exact transfer returns before component transfer and facade fold.
        // Otherwise each independently exact component can evaluate once, then
        // a nonsingleton result may fold one expression. Cached component tags
        // also cover inconsistent products without assuming they are reachable.
        let concrete = if supported {
            evaluation.saturating_mul(exact_components.count_ones() as usize + 1)
        } else {
            0
        };
        visits
            .saturating_add(4096)
            .saturating_add(closure_work.unwrap_or_else(|| {
                self.numeric_closure_work(
                    if boolean {
                        self.capacity().min(2)
                    } else if supported && op == opcode::BYTE {
                        self.capacity().min(256)
                    } else {
                        self.capacity()
                    },
                    rounds,
                )
            }))
            .saturating_add(size)
            .saturating_add(concrete)
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

fn stable_scalar(domain: DomainSpec, value: &NumericValue) -> bool {
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
            || (1usize << unknown) > domain.capacity().min(domain.fact_limit()))
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
