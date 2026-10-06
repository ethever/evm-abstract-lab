//! 同一个字的约束取交集。有限集合组件未知，不能被误读成整个值未知。
use super::{
    FiniteConstantSet, congruence::Congruence, interval::Interval, known_bits::KnownBits,
    provenance::Provenance,
};
use alloy_primitives::U256;
use serde::{Serialize, Serializer, ser::SerializeMap};
use std::{collections::BTreeSet, fmt};

/// EVM Word256 的笛卡尔组合。私有构造保证单个组件合法；未求解的联合
/// 约束不承诺存在具体见证。不可达路径由工作表缺少状态表示。
///
/// ```compile_fail
/// use evm_abstract::domain::Value;
/// let malformed = Value(()); // 字段私有，不能绕过受控构造。
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub(super) finite: FiniteConstantSet,
    pub(super) bits: KnownBits,
    pub(super) interval: Interval,
    pub(super) congruence: Congruence,
    pub(super) provenance: Provenance,
    pub(super) nonzero: bool,
}
impl Value {
    /// 精确的单点；其他数值组件均为此单点的等价表示。
    pub fn constant(value: U256) -> Self {
        Self {
            finite: FiniteConstantSet::constant(value),
            bits: KnownBits::exact(value),
            interval: Interval::exact(value),
            congruence: Congruence::exact(value),
            provenance: Provenance::constant(),
            nonzero: value != U256::ZERO,
        }
    }
    /// 对数值和来源都没有约束。
    pub fn top() -> Self {
        Self {
            finite: FiniteConstantSet::top(),
            bits: KnownBits::top(),
            interval: Interval::top(),
            congruence: Congruence::top(),
            provenance: Provenance::top(),
            nonzero: false,
        }
    }
    /// 未知字节仍只可能为 0..=255；高 248 位是格式保证。
    pub fn unknown_byte() -> Self {
        let mut value = Self::top();
        value.bits = KnownBits::new(U256::MAX << 8usize, U256::ZERO).unwrap();
        value.interval = Interval::new_unsigned(U256::ZERO, U256::from(255)).unwrap();
        value
    }
    /// An unsigned interval without assumptions about the source.
    pub fn unsigned_range(lower: U256, upper: U256) -> Option<Self> {
        let mut value = Self::top();
        value.interval = Interval::new_unsigned(lower, upper)?;
        Some(value)
    }

    /// 地址字段高 96 位为零；不证明账户存在或有代码。
    pub fn unknown_address() -> Self {
        let mut value = Self::top();
        value.bits = KnownBits::new(U256::MAX << 160usize, U256::ZERO).unwrap();
        value.interval = Interval::new_unsigned(U256::ZERO, U256::MAX >> 96usize).unwrap();
        value.provenance = Provenance::source(super::provenance::Origin::Address);
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
    /// 可能来源及局部可信复制身份；来源标签本身不证明相等。
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
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
    /// 复制和比较时的逻辑成本，包含内联数值组件及来源。
    pub fn work_size(&self) -> usize {
        self.finite
            .work_size()
            .saturating_add(12)
            .saturating_add(self.provenance.origins().work_size())
    }
    /// 为来源域增加一个明确的输入来源；不制造数值相等事实。
    pub fn with_origin(mut self, origin: super::provenance::Origin) -> Self {
        self.provenance = self.provenance.join(&Provenance::source(origin));
        self
    }
    pub(crate) fn set_origin(&mut self, origin: super::provenance::Origin) {
        self.provenance.set_origin(origin);
    }
    pub(crate) fn with_symbol(
        mut self,
        symbol: super::provenance::Symbol,
        scope: Option<u64>,
    ) -> Self {
        if self.singleton().is_none()
            && let Some(scope) = scope
        {
            self.provenance = self.provenance.with_symbol(symbol, scope);
        }
        self
    }
    pub(crate) fn with_identity(mut self, scope: u64, definition: u32) -> Self {
        self.provenance = self
            .provenance
            .with_identity(super::provenance::RuntimeIdentity::new(scope, definition));
        self
    }
    pub(crate) fn forget_identity(&mut self) {
        self.provenance.forget_identity();
    }
    pub(super) fn numeric_top(&self) -> bool {
        self.finite.is_top()
            && self.bits == KnownBits::top()
            && self.interval == Interval::top()
            && self.congruence == Congruence::top()
            && !self.nonzero
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // 保留旧 JSON 的 Constants 键；非有限的组合值显式包含全部约束。
        if self.numeric_top() && self.provenance == Provenance::top() {
            return serializer.serialize_str("Top");
        }
        let mut map = serializer.serialize_map(None)?;
        if let Some(constants) = self.finite.as_values() {
            map.serialize_entry("Constants", constants)?;
        }
        map.serialize_entry("known_bits", &self.bits)?;
        map.serialize_entry("interval", &self.interval)?;
        map.serialize_entry("congruence", &self.congruence)?;
        map.serialize_entry("provenance", &self.provenance)?;
        map.serialize_entry("nonzero", &self.nonzero)?;
        map.end()
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.finite.is_top() {
            return self.finite.fmt(f);
        }
        if self.numeric_top() {
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
