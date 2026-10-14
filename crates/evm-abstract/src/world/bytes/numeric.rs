//! Sound numeric evaluation of the shared foundation content graph.
//!
//! This is an upper-bound evaluator, not a second content DAG or alias solver.
//! Unknown aliasing joins both alternatives; SMT consumes the original read term.

use super::{constant, zero};
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

type Facts = BTreeMap<ExprId, AbstractValue>;

pub(in crate::world) fn scalar(
    expression: &ExprId,
    facts: &Facts,
    domain: Domain,
) -> AbstractValue {
    if let Some(v) = facts.get(expression) {
        if let Some(constant) = expression.as_constant() {
            // A constant-key source union is not a unique input occurrence.
            // Drop its must-alias identity while retaining every possible source.
            return domain
                .join(v, &AbstractValue::constant(constant))
                .with_numeric(crate::domain::NumericValue::constant(constant));
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
            let condition = scalar(condition, facts, domain);
            let selected = if !condition.may_be_nonzero() {
                Some(else_value)
            } else if !condition.may_be_zero() {
                Some(then_value)
            } else {
                None
            };
            if let Some(exact) = selected.and_then(|value| scalar(value, facts, domain).singleton())
                && v.contains(exact)
            {
                return v
                    .clone()
                    .with_numeric(crate::domain::NumericValue::constant(exact));
            }
        }
        return v.clone();
    }
    if let Some(v) = expression.as_constant() {
        return AbstractValue::constant(v);
    }
    match expression.kind() {
        ExprKind::Operation { opcode, args } => domain.apply(
            *opcode,
            &args
                .iter()
                .map(|a| scalar(a, facts, domain))
                .collect::<Vec<_>>(),
        ),
        ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } => {
            let c = scalar(condition, facts, domain);
            if !c.may_be_nonzero() {
                scalar(else_value, facts, domain)
            } else if !c.may_be_zero() {
                scalar(then_value, facts, domain)
            } else {
                domain.join(
                    &scalar(then_value, facts, domain),
                    &scalar(else_value, facts, domain),
                )
            }
        }
        _ => AbstractValue::top(),
    }
}

pub(in crate::world) fn numeric_domain(domain: Domain) -> Domain {
    Domain::from_spec(
        domain
            .spec()
            .with_relations(crate::domain::relational::RelationLimits {
                enabled: false,
                ..domain.spec().relations()
            }),
    )
}

pub(in crate::world) fn attach(value: AbstractValue, expression: ExprId) -> AbstractValue {
    if let Some(v) = expression.as_constant() {
        // Constant folding in the content model is a proof. Keep source metadata.
        value.with_numeric(crate::domain::NumericValue::constant(v))
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
pub(super) struct Reader<'a> {
    facts: &'a Facts,
    domain: Domain,
    word_bytes: BTreeMap<ExprId, AbstractValue>,
    retained_work: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(facts: &'a Facts, domain: Domain) -> Self {
        Self {
            facts,
            domain: numeric_domain(domain),
            word_bytes: BTreeMap::new(),
            retained_work: 0,
        }
    }

    fn word_byte_union(&mut self, expression: &ExprId) -> AbstractValue {
        if let Some(value) = self.word_bytes.get(expression) {
            return value.clone();
        }
        // The old evaluator repeated this scalar lookup for all 32 positions.
        // Evaluate it once; BYTE and the ordered joins retain their semantics.
        let scalar = scalar(expression, self.facts, self.domain);
        let value = (0..32)
            .fold(None, |out, j| {
                Some(join(
                    out,
                    self.domain
                        .apply(opcode::BYTE, &[constant(j), scalar.clone()]),
                    self.domain,
                ))
            })
            .expect("32 bytes");
        let work = expression.work_size().saturating_add(value.work_size());
        if self.word_bytes.len() < WORD_CACHE_ENTRIES
            && work <= WORD_CACHE_WORK.saturating_sub(self.retained_work)
        {
            self.word_bytes.insert(expression.clone(), value.clone());
            self.retained_work += work;
        }
        value
    }

    pub(super) fn read(&mut self, state: &ByteStateId, index: &ExprId) -> AbstractValue {
        let domain = self.domain;
        let facts = self.facts;
        let state = demanded_state(state, index, 1, facts);
        match state.kind() {
            ByteStateKind::Zero => scalar(&ExprId::constant(U256::ZERO), facts, domain),
            ByteStateKind::Exact(bytes) => {
                let index = scalar(index, facts, domain);
                if let Some(indices) = index.constants() {
                    indices
                        .iter()
                        .fold(None, |out, i| {
                            Some(join(
                                out,
                                scalar(
                                    &ExprId::constant(U256::from(
                                        usize::try_from(*i)
                                            .ok()
                                            .and_then(|i| bytes.get(i))
                                            .copied()
                                            .unwrap_or(0),
                                    )),
                                    facts,
                                    domain,
                                ),
                                domain,
                            ))
                        })
                        .unwrap_or_else(zero)
                } else if index.interval().unsigned_bounds().0 >= U256::from(bytes.len()) {
                    zero()
                } else {
                    AbstractValue::unknown_byte()
                }
            }
            ByteStateKind::Input {
                length, bindings, ..
            } => {
                let i = scalar(index, facts, domain);
                let len = scalar(length, facts, domain);
                let (_, hi) = len.interval().unsigned_bounds();
                if i.interval().unsigned_bounds().0 >= hi {
                    return zero();
                }
                let cell = if let Some(indices) = i.constants() {
                    indices
                        .iter()
                        .fold(None, |out, i| {
                            let cell = bindings
                                .iter()
                                .find(|b| b.index == *i)
                                .map_or_else(AbstractValue::unknown_byte, |b| {
                                    scalar(&b.value, facts, domain).with_numeric(b.numeric.clone())
                                });
                            Some(join(out, cell, domain))
                        })
                        .unwrap_or_else(AbstractValue::unknown_byte)
                } else {
                    AbstractValue::unknown_byte()
                };
                if i.interval().unsigned_bounds().1 < len.interval().unsigned_bounds().0 {
                    cell
                } else {
                    domain.join(&cell, &zero())
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let val = domain.apply(opcode::AND, &[scalar(value, facts, domain), constant(255)]);
                if index == offset {
                    return val;
                }
                let i = scalar(index, facts, domain);
                let at = scalar(offset, facts, domain);
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
                    domain.join(&self.read(previous, index), &val)
                }
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let i = scalar(index, facts, domain);
                let at = scalar(offset, facts, domain);
                if let (Some(indices), Some(offsets)) = (i.constants(), at.constants()) {
                    let mut out = None;
                    for i in indices {
                        for at in offsets {
                            let val = if *i >= *at && *i - *at < U256::from(32) {
                                domain.apply(
                                    opcode::BYTE,
                                    &[
                                        AbstractValue::constant(*i - *at),
                                        scalar(value, facts, domain),
                                    ],
                                )
                            } else {
                                self.read(previous, &ExprId::constant(*i))
                            };
                            out = Some(join(out, val, domain));
                        }
                    }
                    return out.unwrap_or_else(AbstractValue::unknown_byte);
                }
                let (ilo, ihi) = i.interval().unsigned_bounds();
                let (alo, ahi) = at.interval().unsigned_bounds();
                if ihi < alo
                    || ahi
                        .checked_add(U256::from(32))
                        .is_some_and(|end| ilo >= end)
                {
                    return self.read(previous, index);
                }
                let val = self.word_byte_union(value);
                domain.join(&self.read(previous, index), &val)
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let i = scalar(index, facts, domain);
                let dst = scalar(destination, facts, domain);
                let len = scalar(length, facts, domain);
                if let (Some(indices), Some(destinations), Some(lengths)) =
                    (i.constants(), dst.constants(), len.constants())
                {
                    let mut out = None;
                    for i in indices {
                        for dst in destinations {
                            for len in lengths {
                                let val = if *i >= *dst && *i - *dst < *len {
                                    self.view_read(source, &ExprId::constant(*i - *dst))
                                } else {
                                    self.read(previous, &ExprId::constant(*i))
                                };
                                out = Some(join(out, val, domain));
                            }
                        }
                    }
                    return out.unwrap_or_else(AbstractValue::unknown_byte);
                }
                if index == destination && !len.may_be_zero() {
                    return self.view_read(source, &ExprId::constant(U256::ZERO));
                }
                let (ilo, ihi) = i.interval().unsigned_bounds();
                let (dlo, dhi) = dst.interval().unsigned_bounds();
                let (llo, lhi) = len.interval().unsigned_bounds();
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
                        domain.join(&copied, &self.read(previous, index))
                    };
                }
                domain.join(&self.read(previous, index), &AbstractValue::unknown_byte())
            }
            ByteStateKind::Slice { source } => self.view_read(source, index),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let c = scalar(condition, facts, domain);
                if !c.may_be_zero() {
                    self.read(then_state, index)
                } else if !c.may_be_nonzero() {
                    self.read(else_state, index)
                } else {
                    domain.join(&self.read(then_state, index), &self.read(else_state, index))
                }
            }
        }
    }

    fn view_read(&mut self, view: &ByteView, relative: &ExprId) -> AbstractValue {
        let facts = self.facts;
        let domain = self.domain;
        let rel = scalar(relative, facts, domain);
        let off = scalar(&view.offset, facts, domain);
        let len = scalar(&view.length, facts, domain);
        if let (Some(relatives), Some(offsets), Some(lengths)) =
            (rel.constants(), off.constants(), len.constants())
        {
            let mut out = None;
            for rel in relatives {
                for off in offsets {
                    for len in lengths {
                        let v = if *rel >= *len {
                            zero()
                        } else {
                            off.checked_add(*rel)
                                .map_or_else(zero, |i| self.read(&view.state, &ExprId::constant(i)))
                        };
                        out = Some(join(out, v, domain));
                    }
                }
            }
            return out.unwrap_or_else(AbstractValue::unknown_byte);
        }
        if rel.interval().unsigned_bounds().0 >= len.interval().unsigned_bounds().1 {
            return zero();
        }
        // Checked source addition: no BV256 wraparound can expose source byte zero.
        if let (Some(rel), Some(off)) = (rel.singleton(), off.singleton()) {
            return off.checked_add(rel).map_or_else(zero, |i| {
                let v = self.read(&view.state, &ExprId::constant(i));
                if rel < len.interval().unsigned_bounds().0 {
                    v
                } else {
                    domain.join(&v, &zero())
                }
            });
        }
        AbstractValue::unknown_byte()
    }
}

pub(super) fn read(
    state: &ByteStateId,
    index: &ExprId,
    facts: &Facts,
    domain: Domain,
) -> AbstractValue {
    Reader::new(facts, domain).read(state, index)
}

pub(super) fn word(
    state: &ByteStateId,
    offset: &ExprId,
    facts: &Facts,
    domain: Domain,
) -> AbstractValue {
    let domain = numeric_domain(domain);
    let state = demanded_state(state, offset, 32, facts);
    if offset.as_constant().is_some()
        && let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = state.kind()
    {
        // Joining bytes separately mixes alternatives from different snapshots.
        // The same selector chooses every byte of this concrete word demand.
        let condition = scalar(condition, facts, domain);
        if !condition.may_be_zero() {
            return word(then_state, offset, facts, domain);
        }
        if !condition.may_be_nonzero() {
            return word(else_state, offset, facts, domain);
        }
        return domain.join(
            &word(then_state, offset, facts, domain),
            &word(else_state, offset, facts, domain),
        );
    }
    if let ByteStateKind::WriteWord {
        offset: written,
        value,
        ..
    } = state.kind()
        && written == offset
        && scalar(offset, facts, domain).interval().unsigned_bounds().1
            <= U256::MAX - U256::from(31)
    {
        return scalar(value, facts, domain);
    }
    let offsets = scalar(offset, facts, domain);
    let Some(offsets) = offsets.constants() else {
        return if matches!(state.kind(), ByteStateKind::Zero) {
            scalar(&ExprId::constant(U256::ZERO), facts, domain)
        } else {
            AbstractValue::top()
        };
    };
    let mut out = None;
    for offset in offsets {
        let bytes = (0..32)
            .map(|j| {
                offset
                    .checked_add(U256::from(j))
                    .map_or_else(zero, |i| read(state, &ExprId::constant(i), facts, domain))
            })
            .collect::<Vec<_>>();
        let mut parts = bytes
            .into_iter()
            .enumerate()
            .map(|(j, b)| domain.apply(opcode::SHL, &[constant((31 - j) * 8), b]))
            .collect::<Vec<_>>();
        while parts.len() > 1 {
            parts = parts
                .chunks(2)
                .map(|p| {
                    if p.len() == 2 {
                        domain.apply(opcode::OR, p)
                    } else {
                        p[0].clone()
                    }
                })
                .collect();
        }
        out = Some(join(out, parts.pop().expect("word"), domain));
    }
    out.unwrap_or_else(AbstractValue::top)
}

// The symbolic constructor has already checked retention/cancellation. Numeric
// evaluation follows the same concrete proof iteratively, so a long disjoint
// prefix does not consume the host stack or reconstruct the word byte by byte.
fn demanded_state<'a>(
    state: &'a ByteStateId,
    index: &ExprId,
    width: usize,
    facts: &Facts,
) -> &'a ByteStateId {
    let Some(start) = index.as_constant() else {
        return state;
    };
    let end = start.saturating_add(U256::from(width - 1));
    let mut cursor = state;
    while let Some(previous) =
        cursor.prior_to_disjoint_write_with_bound(start, end, &|destination| {
            super::reads::write_lower_bound(destination, facts)
        })
    {
        cursor = previous;
    }
    cursor
}

fn join(out: Option<AbstractValue>, value: AbstractValue, domain: Domain) -> AbstractValue {
    out.map_or_else(|| value.clone(), |old| domain.join(&old, &value))
}
fn disjoint(a: &AbstractValue, b: &AbstractValue) -> bool {
    let (alo, ahi) = a.interval().unsigned_bounds();
    let (blo, bhi) = b.interval().unsigned_bounds();
    ahi < blo
        || bhi < alo
        || match (a.constants(), b.constants()) {
            (Some(a), Some(b)) => a.is_disjoint(b),
            _ => false,
        }
}

#[cfg(test)]
mod tests;
