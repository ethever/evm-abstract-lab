//! Sound numeric evaluation of the shared foundation content graph.
//!
//! This is an upper-bound evaluator, not a second content DAG or alias solver.
//! Unknown aliasing joins both alternatives; reads retain their original terms.

use super::{conditional::Context, constant_with, zero_with};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue, Profile,
    symbolic::{ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind},
};
#[cfg(test)]
use crate::world::bytes::{constant, zero};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

mod full_image;
mod packing;

type Facts<N = crate::domain::NumericValue> = BTreeMap<ExprId, EvmValue<N>>;

pub(in crate::world) fn scalar<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    scalar_in(
        expression,
        &Context::<N, D>::new(facts, None, domain),
        domain,
    )
}

pub(super) fn scalar_in<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    if context.stopped() {
        // The checked demand retries a private cache refusal or propagates
        // cancellation before installing a result. This is only a sentinel.
        return EvmValue::<N>::top();
    }
    if let Some(v) = context.value(expression, domain) {
        if context.stopped() {
            return EvmValue::<N>::top();
        }
        if let Some(constant_with) = expression.as_constant() {
            // A constant-key source union is not a unique input occurrence.
            // Drop its must-alias identity while retaining every possible source.
            let joined = context.join_with(domain, &v, &EvmValue::<N>::constant(constant_with));
            return if context.stopped() {
                EvmValue::<N>::top()
            } else {
                joined.with_numeric(N::constant(constant_with))
            };
        }
        if let ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } = expression.kind()
        {
            // A retained minimum/copy-length bound can precede a later proof
            // selecting this ITE. Keep its sources, but do not let the older
            // bound hide a now-exact selected value of the same expression.
            let c = scalar_in(condition, context, domain);
            let selected = if !c.may_be_nonzero() {
                Some((else_value, false))
            } else if !c.may_be_zero() {
                Some((then_value, true))
            } else {
                None
            };
            if let Some(exact) = selected.and_then(|(value, nonzero)| {
                scalar_in(value, &context.branch(condition, nonzero), domain).singleton()
            }) && v.contains(exact)
            {
                return v.clone().with_numeric(N::constant(exact));
            }
        }
        return if context.stopped() {
            EvmValue::<N>::top()
        } else {
            v.clone()
        };
    }
    if context.stopped() {
        return EvmValue::<N>::top();
    }
    if let Some(v) = expression.as_constant() {
        return EvmValue::<N>::constant(v);
    }
    match expression.kind() {
        ExprKind::Operation { opcode, args } => context.apply(
            domain,
            *opcode,
            &args
                .iter()
                .map(|a| scalar_in(a, context, domain))
                .collect::<Vec<_>>(),
        ),
        ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } => {
            let c = scalar_in(condition, context, domain);
            if !c.may_be_nonzero() {
                scalar_in(else_value, &context.branch(condition, false), domain)
            } else if !c.may_be_zero() {
                scalar_in(then_value, &context.branch(condition, true), domain)
            } else {
                context.join_with(
                    domain,
                    &scalar_in(then_value, &context.branch(condition, true), domain),
                    &scalar_in(else_value, &context.branch(condition, false), domain),
                )
            }
        }
        _ => EvmValue::<N>::top(),
    }
}

pub(in crate::world) fn numeric_domain_with<'a, D: EvmNumericAlgebra>(
    domain: AlgebraRef<'a, D>,
) -> AlgebraRef<'a, D> {
    domain.without_relations()
}

pub(in crate::world) fn attach<N: WordElement>(
    value: EvmValue<N>,
    expression: ExprId,
) -> EvmValue<N> {
    if let Some(v) = expression.as_constant() {
        // Constant folding in the content model is a proof. Keep source metadata.
        value.with_numeric(N::constant(v))
    } else {
        value.with_expression(expression)
    }
}

// Only memoize pure evaluations in one immutable operand environment. These
// limits bound temporary retention independently of logical array length.
const WORD_CACHE_ENTRIES: usize = 32;
const WORD_CACHE_WORK: usize = 8 * 1024;

/// A batch of reads using exactly the same immutable facts and numeric policy.
/// Cached values keep all metadata and never escape this evaluator's lifetime.
pub(super) struct Reader<
    'a,
    N: WordElement = crate::domain::NumericValue,
    D: EvmNumericAlgebra<Element = N> = crate::domain::Domain,
> {
    context: Context<'a, N, D>,
    domain: AlgebraRef<'a, D>,
    word_bytes: BTreeMap<ExprId, EvmValue<N>>,
    retained_work: usize,
}

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Reader<'a, N, D> {
    pub(super) fn new(facts: &'a Facts<N>, domain: AlgebraRef<'a, D>) -> Self {
        Self::with_context(Context::<N, D>::new(facts, None, domain), domain)
    }

    pub(super) fn with_context(context: Context<'a, N, D>, domain: AlgebraRef<'a, D>) -> Self {
        Self {
            context,
            domain: numeric_domain_with(domain),
            word_bytes: BTreeMap::new(),
            retained_work: 0,
        }
    }

    /// Inspect this Reader's bounded cache without changing its evaluation.
    #[cfg(test)]
    pub(super) fn cache_stats(&self) -> (usize, usize) {
        (self.word_bytes.len(), self.retained_work)
    }

    fn word_byte_union(&mut self, expression: &ExprId) -> EvmValue<N> {
        if self.context.stopped() {
            return EvmValue::<N>::top();
        }
        if let Some(value) = self.word_bytes.get(expression) {
            return value.clone();
        }
        // The old evaluator repeated this scalar lookup for all 32 positions.
        // Evaluate it once; BYTE and the ordered joins retain their semantics.
        let scalar = scalar_in(expression, &self.context, self.domain);
        if self.context.stopped() {
            return EvmValue::<N>::top();
        }
        let value = if self.domain.byte_image_candidate(scalar.numeric())
            && scalar.numeric().is_top()
            && self.domain.spec().profile() == Profile::Product
            && !self.domain.spec().relations().enabled
        {
            // Every position has the same complete BYTE result for numeric Top.
            // Keep the ordinary transfer's finite facts, metadata and limits.
            let value = self
                .context
                .apply(self.domain, opcode::BYTE, &[constant_with(0), scalar]);
            #[cfg(test)]
            if !self.context.stopped() {
                tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.0 += 1);
            }
            value
        } else {
            // Enum/policy checks leave finite and congruence-constrained words
            // on their original path. Fund the fixed mask proof before running
            // it; a refused mandatory payment must not enter the old loop.
            let complete = if full_image::candidate_with(scalar.numeric(), self.domain) {
                if !self.context.pay_byte_image(self.domain) {
                    return EvmValue::<N>::top();
                }
                full_image::complete(scalar.numeric(), self.domain)
            } else {
                false
            };
            if complete {
                // Consume the owned scalar: its variable metadata is moved,
                // never cloned by the certificate. Only the temporary numeric
                // layer changes; no fact, expression or state is replaced.
                let value = self.context.apply(
                    self.domain,
                    opcode::BYTE,
                    &[constant_with(0), scalar.with_numeric(N::top())],
                );
                #[cfg(test)]
                if !self.context.stopped() {
                    tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.0 += 1);
                }
                value
            } else {
                (0..32)
                    .fold(None, |out, j| {
                        let byte = self.context.apply(
                            self.domain,
                            opcode::BYTE,
                            &[constant_with(j), scalar.clone()],
                        );
                        #[cfg(test)]
                        if !self.context.stopped() {
                            tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.0 += 1);
                        }
                        #[cfg(test)]
                        let joins = out.is_some();
                        let value = join(out, byte, self.domain, &self.context);
                        #[cfg(test)]
                        if joins && !self.context.stopped() {
                            tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.1 += 1);
                        }
                        Some(value)
                    })
                    .expect("32 bytes")
            }
        };
        if self.context.stopped() {
            return EvmValue::<N>::top();
        }
        let work = expression.work_size().saturating_add(value.work_size());
        if self.word_bytes.len() < WORD_CACHE_ENTRIES
            && work <= WORD_CACHE_WORK.saturating_sub(self.retained_work)
        {
            self.word_bytes.insert(expression.clone(), value.clone());
            self.retained_work += work;
        }
        value
    }

    pub(super) fn read(&mut self, state: &ByteStateId, index: &ExprId) -> EvmValue<N> {
        if self.context.stopped() {
            return EvmValue::<N>::top();
        }
        let domain = self.domain;
        let context = self.context.clone();
        let state = demanded_state(state, index, 1, &context, domain);
        let value = match state.kind() {
            ByteStateKind::Zero => scalar_in(&ExprId::constant(U256::ZERO), &context, domain),
            ByteStateKind::Exact(bytes) => {
                let index = scalar_in(index, &context, domain);
                if let Some(indices) = crate::world::candidates(&index) {
                    indices
                        .iter()
                        .fold(None, |out, i| {
                            Some(join(
                                out,
                                scalar_in(
                                    &ExprId::constant(U256::from(
                                        usize::try_from(*i)
                                            .ok()
                                            .and_then(|i| bytes.get(i))
                                            .copied()
                                            .unwrap_or(0),
                                    )),
                                    &context,
                                    domain,
                                ),
                                domain,
                                &context,
                            ))
                        })
                        .unwrap_or_else(zero_with)
                } else if crate::world::unsigned_bounds(&index).0 >= U256::from(bytes.len()) {
                    zero_with()
                } else {
                    EvmValue::<N>::unknown_byte()
                }
            }
            ByteStateKind::Input {
                length, bindings, ..
            } => {
                let i = scalar_in(index, &context, domain);
                let len = scalar_in(length, &context, domain);
                if context.stopped() {
                    return EvmValue::<N>::top();
                }
                let (_, hi) = crate::world::unsigned_bounds(&len);
                if crate::world::unsigned_bounds(&i).0 >= hi {
                    return zero_with();
                }
                let cell = if let Some(indices) = crate::world::candidates(&i) {
                    indices
                        .iter()
                        .fold(None, |out, i| {
                            let cell = bindings.iter().find(|b| b.index == *i).map_or_else(
                                EvmValue::<N>::unknown_byte,
                                |b| {
                                    let retained = context.has_binding_value(&b.value);
                                    let value = scalar_in(&b.value, &context, domain);
                                    if context.stopped()
                                        || (retained
                                            && !context.pay_binding(value.numeric(), &b.numeric))
                                    {
                                        return EvmValue::<N>::top();
                                    }
                                    bound(value, retained, &b.numeric)
                                },
                            );
                            Some(join(out, cell, domain, &context))
                        })
                        .unwrap_or_else(EvmValue::<N>::unknown_byte)
                } else {
                    EvmValue::<N>::unknown_byte()
                };
                if crate::world::unsigned_bounds(&i).1 < crate::world::unsigned_bounds(&len).0 {
                    cell
                } else {
                    context.join_with(domain, &cell, &zero_with())
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let val = context.observe_byte(
                    domain,
                    opcode::AND,
                    &[scalar_in(value, &context, domain), constant_with(255)],
                );
                if index == offset {
                    return val;
                }
                let i = scalar_in(index, &context, domain);
                let at = scalar_in(offset, &context, domain);
                if let (Some(i), Some(at)) = (i.singleton(), at.singleton()) {
                    return if i == at {
                        val
                    } else {
                        self.read(previous, index)
                    };
                }
                if disjoint(&i, &at) {
                    self.read(previous, index)
                } else {
                    context.join_with(domain, &self.read(previous, index), &val)
                }
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let i = scalar_in(index, &context, domain);
                let at = scalar_in(offset, &context, domain);
                if let (Some(indices), Some(offsets)) =
                    (crate::world::candidates(&i), crate::world::candidates(&at))
                {
                    let mut out = None;
                    for i in indices.iter() {
                        for at in offsets.iter() {
                            let val = if *i >= *at && *i - *at < U256::from(32) {
                                context.observe_byte(
                                    domain,
                                    opcode::BYTE,
                                    &[
                                        EvmValue::<N>::constant(*i - *at),
                                        scalar_in(value, &context, domain),
                                    ],
                                )
                            } else {
                                self.read(previous, &ExprId::constant(*i))
                            };
                            out = Some(join(out, val, domain, &context));
                        }
                    }
                    return out.unwrap_or_else(EvmValue::<N>::unknown_byte);
                }
                let (ilo, ihi) = crate::world::unsigned_bounds(&i);
                let (alo, ahi) = crate::world::unsigned_bounds(&at);
                if ihi < alo
                    || ahi
                        .checked_add(U256::from(32))
                        .is_some_and(|end| ilo >= end)
                {
                    return self.read(previous, index);
                }
                let val = self.word_byte_union(value);
                context.join_with(domain, &self.read(previous, index), &val)
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let i = scalar_in(index, &context, domain);
                let dst = scalar_in(destination, &context, domain);
                let len = scalar_in(length, &context, domain);
                if let (Some(indices), Some(destinations), Some(lengths)) = (
                    crate::world::candidates(&i),
                    crate::world::candidates(&dst),
                    crate::world::candidates(&len),
                ) {
                    let mut out = None;
                    for i in indices.iter() {
                        for dst in destinations.iter() {
                            for len in lengths.iter() {
                                let val = if *i >= *dst && *i - *dst < *len {
                                    self.view_read(source, &ExprId::constant(*i - *dst))
                                } else {
                                    self.read(previous, &ExprId::constant(*i))
                                };
                                out = Some(join(out, val, domain, &context));
                            }
                        }
                    }
                    return out.unwrap_or_else(EvmValue::<N>::unknown_byte);
                }
                if index == destination && !len.may_be_zero() {
                    return self.view_read(source, &ExprId::constant(U256::ZERO));
                }
                let (ilo, ihi) = crate::world::unsigned_bounds(&i);
                let (dlo, dhi) = crate::world::unsigned_bounds(&dst);
                let (llo, lhi) = crate::world::unsigned_bounds(&len);
                if ihi < dlo || dhi.checked_add(lhi).is_some_and(|end| ilo >= end) {
                    return self.read(previous, index);
                }
                if let (Some(i), Some(dst)) = (i.singleton(), dst.singleton())
                    && i >= dst
                {
                    let delta = i - dst;
                    let copied = self.view_read(source, &ExprId::constant(delta));
                    return if delta < llo {
                        copied
                    } else if delta >= lhi {
                        self.read(previous, index)
                    } else {
                        context.join_with(domain, &copied, &self.read(previous, index))
                    };
                }
                context.join_with(
                    domain,
                    &self.read(previous, index),
                    &EvmValue::<N>::unknown_byte(),
                )
            }
            ByteStateKind::Slice { source } => self.view_read(source, index),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let c = scalar_in(condition, &context, domain);
                if !c.may_be_zero() {
                    Self::with_context(context.branch(condition, true), domain)
                        .read(then_state, index)
                } else if !c.may_be_nonzero() {
                    Self::with_context(context.branch(condition, false), domain)
                        .read(else_state, index)
                } else {
                    context.join_with(
                        domain,
                        &Self::with_context(context.branch(condition, true), domain)
                            .read(then_state, index),
                        &Self::with_context(context.branch(condition, false), domain)
                            .read(else_state, index),
                    )
                }
            }
        };
        if context.stopped() {
            EvmValue::<N>::top()
        } else {
            value
        }
    }

    fn view_read(&mut self, view: &ByteView, relative: &ExprId) -> EvmValue<N> {
        let context = self.context.clone();
        let domain = self.domain;
        let rel = scalar_in(relative, &context, domain);
        let off = scalar_in(&view.offset, &context, domain);
        let len = scalar_in(&view.length, &context, domain);
        if context.stopped() {
            return EvmValue::<N>::top();
        }
        if let (Some(relatives), Some(offsets), Some(lengths)) = (
            crate::world::candidates(&rel),
            crate::world::candidates(&off),
            crate::world::candidates(&len),
        ) {
            let mut out = None;
            for rel in relatives.iter() {
                for off in offsets.iter() {
                    for len in lengths.iter() {
                        let v = if *rel >= *len {
                            zero_with()
                        } else {
                            off.checked_add(*rel).map_or_else(zero_with, |i| {
                                self.read(&view.state, &ExprId::constant(i))
                            })
                        };
                        out = Some(join(out, v, domain, &context));
                    }
                }
            }
            return out.unwrap_or_else(EvmValue::<N>::unknown_byte);
        }
        if crate::world::unsigned_bounds(&rel).0 >= crate::world::unsigned_bounds(&len).1 {
            return zero_with();
        }
        // Checked source addition: no BV256 wraparound can expose source byte zero.
        if let (Some(rel), Some(off)) = (rel.singleton(), off.singleton()) {
            return off.checked_add(rel).map_or_else(zero_with, |i| {
                let v = self.read(&view.state, &ExprId::constant(i));
                if rel < crate::world::unsigned_bounds(&len).0 {
                    v
                } else {
                    context.join_with(domain, &v, &zero_with())
                }
            });
        }
        EvmValue::<N>::unknown_byte()
    }
}

pub(super) fn read<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    state: &ByteStateId,
    index: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    Reader::<N, D>::new(facts, domain).read(state, index)
}

impl<N: WordElement, D: EvmNumericAlgebra<Element = N>> Reader<'_, N, D> {
    pub(super) fn word(&mut self, state: &ByteStateId, offset: &ExprId) -> EvmValue<N> {
        if self.context.stopped() {
            return EvmValue::<N>::top();
        }
        let domain = self.domain;
        let context = self.context.clone();
        let state = demanded_state(state, offset, 32, &context, domain);
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = state.kind()
        {
            // Joining bytes separately mixes alternatives from different snapshots.
            // The same selector chooses every byte of this word demand.
            let c = scalar_in(condition, &context, domain);
            if !c.may_be_zero() {
                return Self::with_context(context.branch(condition, true), domain)
                    .word(then_state, offset);
            }
            if !c.may_be_nonzero() {
                return Self::with_context(context.branch(condition, false), domain)
                    .word(else_state, offset);
            }
            return context.join_with(
                domain,
                &Self::with_context(context.branch(condition, true), domain)
                    .word(then_state, offset),
                &Self::with_context(context.branch(condition, false), domain)
                    .word(else_state, offset),
            );
        }
        if let ByteStateKind::WriteWord {
            offset: written,
            value,
            ..
        } = state.kind()
            && super::reads::same_offset(written, offset, &context, domain)
            && super::reads::nonwrapping_word(offset, &context, domain)
        {
            return scalar_in(value, &context, domain);
        }
        // A view/copy forwards a value only when every demanded byte belongs
        // to the same nonwrapping source window. Partial windows keep the
        // ordinary byte evaluator, including its padding and previous bytes.
        let forwarded = match state.kind() {
            ByteStateKind::Slice { source } => {
                super::reads::view_word_offset(source, offset, &context, domain)
                    .map(|at| (&source.state, at))
            }
            ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } => super::reads::copy_word_offset(
                destination,
                source,
                length,
                offset,
                &context,
                domain,
            )
            .map(|at| (&source.state, at)),
            _ => None,
        };
        if let Some((source, at)) = forwarded {
            return self.word(source, &at);
        }
        let offsets = scalar_in(offset, &context, domain);
        let Some(offsets) = crate::world::candidates(&offsets) else {
            return if matches!(state.kind(), ByteStateKind::Zero) {
                scalar_in(&ExprId::constant(U256::ZERO), &context, domain)
            } else {
                EvmValue::<N>::top()
            };
        };
        let mut out = None;
        for offset in offsets.iter() {
            let bytes = (0..32)
                .map(|j| {
                    offset
                        .checked_add(U256::from(j))
                        .map_or_else(zero_with, |i| self.read(state, &ExprId::constant(i)))
                })
                .collect::<Vec<_>>();
            if context.stopped() {
                return EvmValue::<N>::top();
            }
            if let Some(value) = packing::pack(&bytes, domain) {
                out = Some(join(out, value, domain, &context));
                continue;
            }
            let mut parts = bytes
                .into_iter()
                .enumerate()
                .map(|(j, b)| context.apply(domain, opcode::SHL, &[constant_with((31 - j) * 8), b]))
                .collect::<Vec<_>>();
            while parts.len() > 1 {
                if context.stopped() {
                    return EvmValue::<N>::top();
                }
                parts = parts
                    .chunks(2)
                    .map(|p| {
                        if p.len() == 2 {
                            context.apply(domain, opcode::OR, p)
                        } else {
                            p[0].clone()
                        }
                    })
                    .collect();
            }
            out = Some(join(out, parts.pop().expect("word"), domain, &context));
        }
        out.unwrap_or_else(EvmValue::<N>::top)
    }
}

// The symbolic constructor has already checked retention/cancellation. Numeric
// evaluation follows the same concrete proof iteratively, so a long disjoint
// prefix does not consume the host stack or reconstruct the word byte by byte.
fn demanded_state<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    state: &'a ByteStateId,
    index: &ExprId,
    width: usize,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> &'a ByteStateId {
    let Some(start) = index.as_constant() else {
        return state;
    };
    let end = start.saturating_add(U256::from(width - 1));
    let mut cursor = state;
    if context.stopped() {
        return cursor;
    }
    while let Some(previous) =
        cursor.prior_to_disjoint_write_with_bound(start, end, &|destination| {
            context.write_lower_bound(destination, domain)
        })
    {
        if context.stopped() {
            break;
        }
        cursor = previous;
    }
    cursor
}

fn join<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: Option<EvmValue<N>>,
    value: EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    context: &Context<'_, N, D>,
) -> EvmValue<N> {
    if context.stopped() {
        return EvmValue::<N>::top();
    }
    out.map_or_else(
        || value.clone(),
        |old| context.join_with(domain, &old, &value),
    )
}
fn disjoint<N: WordElement>(a: &EvmValue<N>, b: &EvmValue<N>) -> bool {
    let (alo, ahi) = crate::world::unsigned_bounds(a);
    let (blo, bhi) = crate::world::unsigned_bounds(b);
    ahi < blo
        || bhi < alo
        || match (a.constants(), b.constants()) {
            (Some(a), Some(b)) => a.is_disjoint(b),
            _ => false,
        }
}

#[cfg(test)]
mod payer_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod windows;

/// Import a binding onto its actual retained carrier for the same expression.
/// Budgeted callers pay import_binding_work before calling this helper.
pub(in crate::world) fn bound<N: WordElement>(
    value: EvmValue<N>,
    retained: bool,
    observation: &crate::domain::WordObservation,
) -> EvmValue<N> {
    if retained {
        let numeric = value.numeric().import_binding(observation);
        value.with_numeric(numeric)
    } else {
        value.with_numeric(N::from_observation(observation))
    }
}

#[cfg(test)]
pub(in crate::world) fn numeric_domain(domain: crate::domain::Domain) -> crate::domain::Domain {
    crate::domain::Domain::from_spec(domain.spec().with_relations(
        crate::domain::relational::RelationLimits {
            enabled: false,
            ..domain.spec().relations()
        },
    ))
}
