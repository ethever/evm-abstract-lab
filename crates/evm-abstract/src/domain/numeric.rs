//! Numeric Word256 constraints only; metadata and value identity are separate.

use super::{FiniteConstantSet, congruence::Congruence, interval::Interval, known_bits::KnownBits};
use alloy_primitives::U256;
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{collections::BTreeSet, fmt};

/// Intersection of finite candidates, masks, intervals and congruences.
/// Component Top never means that all other numeric constraints are absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumericValue {
    pub(super) finite: FiniteConstantSet,
    pub(super) bits: KnownBits,
    pub(super) interval: Interval,
    pub(super) congruence: Congruence,
    pub(super) nonzero: bool,
}
impl NumericValue {
    /// Exact unsigned word.
    pub fn constant(value: U256) -> Self {
        Self {
            finite: FiniteConstantSet::constant(value),
            bits: KnownBits::exact(value),
            interval: Interval::exact(value),
            congruence: Congruence::exact(value),
            nonzero: value != U256::ZERO,
        }
    }
    /// No numeric constraints.
    pub fn top() -> Self {
        Self {
            finite: FiniteConstantSet::top(),
            bits: KnownBits::top(),
            interval: Interval::top(),
            congruence: Congruence::top(),
            nonzero: false,
        }
    }
    /// Unknown byte, constrained to 0..=255.
    pub fn unknown_byte() -> Self {
        let mut value = Self::top();
        value.bits = KnownBits::new(U256::MAX << 8usize, U256::ZERO).unwrap();
        value.interval = Interval::new_unsigned(U256::ZERO, U256::from(255)).unwrap();
        value
    }
    /// Unsigned range; reversed bounds do not manufacture a value.
    pub fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        let mut value = Self::top();
        value.interval = Interval::new_unsigned(lower, upper)?;
        Some(value)
    }
    /// Unknown address word, with the high 96 bits zero.
    pub fn unknown_address() -> Self {
        let mut value = Self::top();
        value.bits = KnownBits::new(U256::MAX << 160usize, U256::ZERO).unwrap();
        value.interval = Interval::new_unsigned(U256::ZERO, U256::MAX >> 96usize).unwrap();
        value
    }
    /// 完整有限候选集合；None 仅表示此组件不可枚举，不表示 whole-product Top。
    pub fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.finite.as_values()
    }
    /// 有限常量集合组件；它的 Top 不会抹去其他数值约束。
    pub fn finite_constants(&self) -> &FiniteConstantSet {
        &self.finite
    }
    /// 已知位约束。
    pub fn known_bits(&self) -> &KnownBits {
        &self.bits
    }
    /// 无符号和有符号界的交。
    pub fn interval(&self) -> &Interval {
        &self.interval
    }
    /// 一般模数同余约束。
    pub fn congruence(&self) -> &Congruence {
        &self.congruence
    }
    /// 排除必须由至少一个组件证明；true 是“尚不能排除”，不是可达见证。
    pub fn contains(&self, value: U256) -> bool {
        (!self.nonzero || value != U256::ZERO)
            && self.finite.contains(value)
            && self.bits.contains(value)
            && self.interval.contains(value)
            && self.congruence.contains(value)
    }
    /// 单点查询使用全部组件，而不依赖有限集合是否可用。
    pub fn singleton(&self) -> Option<U256> {
        let value = self
            .finite
            .singleton()
            .or_else(|| self.bits.singleton())
            .or_else(|| self.interval.singleton())
            .or_else(|| self.congruence.singleton())?;
        self.contains(value).then_some(value)
    }
    /// 零未被任何组件排除。
    pub fn may_be_zero(&self) -> bool {
        self.contains(U256::ZERO)
    }
    /// 未证明这个值只能是零。
    pub fn may_be_nonzero(&self) -> bool {
        self.singleton() != Some(U256::ZERO)
    }
    /// Logical cost of inline numeric data and retained candidates.
    pub fn work_size(&self) -> usize {
        self.finite.work_size().saturating_add(12)
    }
    /// All numeric components are unconstrained.
    pub fn is_top(&self) -> bool {
        self.finite.is_top()
            && self.bits == KnownBits::top()
            && self.interval == Interval::top()
            && self.congruence == Congruence::top()
            && !self.nonzero
    }
}
impl Serialize for NumericValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.is_top() {
            return serializer.serialize_str("Top");
        }
        let mut map = serializer.serialize_map(None)?;
        if let Some(constants) = self.finite.as_values() {
            map.serialize_entry("Constants", constants)?;
        }
        map.serialize_entry("known_bits", &self.bits)?;
        map.serialize_entry("interval", &self.interval)?;
        map.serialize_entry("congruence", &self.congruence)?;
        map.serialize_entry("nonzero", &self.nonzero)?;
        map.end()
    }
}
impl fmt::Display for NumericValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.finite.is_top() {
            return self.finite.fmt(f);
        }
        if self.is_top() {
            return f.write_str("⊤");
        }
        let (lo, hi) = self.interval.unsigned_bounds();
        write!(f, "u[0x{lo:x},0x{hi:x}] bits={}", self.bits)?;
        if let Some((m, r)) = self.congruence.modulus_residue() {
            write!(f, " mod(0x{m:x})=0x{r:x}")?;
        }
        if self.nonzero {
            f.write_str(" ≠0")?;
        }
        Ok(())
    }
}
