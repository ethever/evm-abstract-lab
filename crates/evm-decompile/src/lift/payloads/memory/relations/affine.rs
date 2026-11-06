//! Modular identity and ordinary unsigned ranges are deliberately separate.
use crate::{EntityId, U256};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Atom {
    Source(EntityId),
    Iteration(EntityId),
    Local(EntityId, EntityId),
}
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Expr {
    pub terms: BTreeMap<Atom, U256>,
    pub constant: U256,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Range {
    pub low: U256,
    pub high: U256,
}
impl Range {
    pub const FULL: Self = Self {
        low: U256::ZERO,
        high: U256::MAX,
    };
    pub fn exact(value: U256) -> Self {
        Self {
            low: value,
            high: value,
        }
    }
    pub fn hull(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }
    pub fn intersect(self, other: Self) -> Option<Self> {
        let result = Self {
            low: self.low.max(other.low),
            high: self.high.min(other.high),
        };
        (result.low <= result.high).then_some(result)
    }
}
impl Expr {
    pub fn constant(constant: U256) -> Self {
        Self {
            terms: BTreeMap::new(),
            constant,
        }
    }
    pub fn atom(atom: Atom) -> Self {
        Self {
            terms: BTreeMap::from([(atom, U256::from(1))]),
            constant: U256::ZERO,
        }
    }
    pub fn exact(&self) -> Option<U256> {
        self.terms.is_empty().then_some(self.constant)
    }
    pub fn single(&self) -> Option<Atom> {
        if self.constant != U256::ZERO || self.terms.len() != 1 {
            return None;
        }
        let (&atom, &coefficient) = self.terms.first_key_value()?;
        (coefficient == U256::from(1)).then_some(atom)
    }
    pub fn add(&self, other: &Self) -> Self {
        let mut result = self.clone();
        result.constant = result.constant.wrapping_add(other.constant);
        for (&atom, &coefficient) in &other.terms {
            let value = result.terms.entry(atom).or_default();
            *value = value.wrapping_add(coefficient);
            if *value == U256::ZERO {
                result.terms.remove(&atom);
            }
        }
        result
    }
    pub fn scale(&self, factor: U256) -> Self {
        Self {
            constant: self.constant.wrapping_mul(factor),
            terms: self
                .terms
                .iter()
                .filter_map(|(&a, &c)| {
                    let c = c.wrapping_mul(factor);
                    (c != U256::ZERO).then_some((a, c))
                })
                .collect(),
        }
    }
    pub fn sub(&self, other: &Self) -> Self {
        self.add(&other.scale(U256::MAX))
    }
    /// None means that the modular expression has no proved ordinary range.
    /// A signed presentation is used only if both aggregate endpoints fit.
    /// Check that signed normalization does not discard an integer carry.
    /// Modular identity alone is insufficient for an ordinary range claim.
    pub fn integral_add(&self, other: &Self) -> bool {
        let mid = U256::MAX >> 1;
        let magnitude = |v: U256| {
            if v > mid {
                U256::ZERO.wrapping_sub(v)
            } else {
                v
            }
        };
        let compatible = |a: U256, b: U256| {
            if magnitude(a) > mid || magnitude(b) > mid {
                return false;
            }
            if (a > mid) == (b > mid) {
                magnitude(a)
                    .checked_add(magnitude(b))
                    .is_some_and(|v| v <= mid)
            } else {
                true
            }
        };
        compatible(self.constant, other.constant)
            && other.terms.values().all(|&v| magnitude(v) <= mid)
            && self.terms.iter().all(|(atom, &c)| {
                compatible(c, other.terms.get(atom).copied().unwrap_or(U256::ZERO))
            })
    }
    pub fn integral_scale(&self, factor: U256) -> bool {
        let mid = U256::MAX >> 1;
        if factor > mid {
            return false;
        }
        self.terms
            .values()
            .chain(std::iter::once(&self.constant))
            .all(|&v| {
                let magnitude = if v > mid {
                    U256::ZERO.wrapping_sub(v)
                } else {
                    v
                };
                magnitude.checked_mul(factor).is_some_and(|v| v <= mid)
            })
    }
    pub fn difference(&self, other: &Self) -> Option<Self> {
        // Do not turn a modular negative difference into an unsigned ordering.
        // Retain the signed constants before cancellation removes all symbols.
        let signed = |expr: &Self| {
            if !expr.terms.is_empty() && expr.constant > (U256::MAX >> 1) {
                (true, U256::ZERO.wrapping_sub(expr.constant))
            } else {
                (false, expr.constant)
            }
        };
        let (an, a) = signed(self);
        let (bn, b) = signed(other);
        let delta = self.sub(other);
        if delta.terms.is_empty() {
            let value = match (an, bn) {
                (false, false) => a.checked_sub(b),
                (true, true) => b.checked_sub(a),
                (false, true) => a.checked_add(b),
                (true, false) => None,
            }?;
            return Some(Self::constant(value));
        }
        // A signed normal form cannot represent a large literal offset without
        // changing its ordinary meaning. Such ranges keep the raw fallback.
        if self.terms.is_empty() && self.constant > (U256::MAX >> 1)
            || other.terms.is_empty() && other.constant > (U256::MAX >> 1)
        {
            return None;
        }
        self.integral_add(&other.scale(U256::MAX)).then_some(delta)
    }
    pub fn range(&self, atoms: &BTreeMap<Atom, Range>) -> Option<Range> {
        if let Some(value) = self.exact() {
            return Some(Range::exact(value));
        }
        let negative = |value: U256| value > (U256::MAX >> 1);
        let mut positive = Range::exact(U256::ZERO);
        let mut minus = Range::exact(U256::ZERO);
        if negative(self.constant) {
            minus = Range::exact(U256::ZERO.wrapping_sub(self.constant));
        } else {
            positive = Range::exact(self.constant);
        }
        for (&atom, &coefficient) in &self.terms {
            let range = atoms.get(&atom).copied().unwrap_or(Range::FULL);
            let (sum, factor) = if negative(coefficient) {
                (&mut minus, U256::ZERO.wrapping_sub(coefficient))
            } else {
                (&mut positive, coefficient)
            };
            sum.low = sum.low.checked_add(range.low.checked_mul(factor)?)?;
            sum.high = sum.high.checked_add(range.high.checked_mul(factor)?)?;
        }
        Some(Range {
            low: positive.low.checked_sub(minus.high)?,
            high: positive.high.checked_sub(minus.low)?,
        })
    }
}
