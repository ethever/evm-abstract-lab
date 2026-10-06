//! 有界的有限常量集合组件：Top 或非空的完整候选集合。
//!
//! Top 只表示本组件没有约束，不表示整个组合域的值未知。容量由调用方
//! 的域配置传入；候选超过容量时退化为 Top，空交集则返回矛盾错误。
//! 本组件不保存 Bottom，不可达状态由组合域的规约结果负责表示。

use super::known_bits::KnownBits;
use alloy_primitives::U256;
use serde::{Serialize, Serializer};
use std::{collections::BTreeSet, fmt, num::NonZeroUsize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Top,
    Finite(BTreeSet<U256>),
}

/// 没有满足约束的候选；不能作为一个可达值的有限常量组件保存。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("finite constant set must be nonempty")]
pub struct EmptyFiniteConstantSet;

/// Top 或非空的有限常量集合，内部表示不可由调用方直接修改。
///
/// ```compile_fail
/// use evm_abstract::domain::FiniteConstantSet;
///
/// // 私有字段阻止调用方绕过构造函数的不变量。
/// let invalid = FiniteConstantSet { 0: todo!() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteConstantSet(Kind);

impl FiniteConstantSet {
    /// 本组件不约束具体 word。
    pub fn top() -> Self {
        Self(Kind::Top)
    }

    /// 只有一个具体 word 的集合。
    pub fn constant(value: U256) -> Self {
        Self(Kind::Finite(BTreeSet::from([value])))
    }

    /// 取得已有候选集合的所有权，拒绝空集合。
    ///
    /// 此构造函数不施加容量限制；保存到值时仍须遵守调用方的域配置。
    pub fn try_from_values(values: BTreeSet<U256>) -> Result<Self, EmptyFiniteConstantSet> {
        if values.is_empty() {
            return Err(EmptyFiniteConstantSet);
        }
        Ok(Self(Kind::Finite(values)))
    }

    /// 去重后收集完整候选；超过容量时退化为 Top，空输入返回矛盾错误。
    ///
    /// 发现第一个超出容量的不同候选后即可停止消耗输入。
    pub fn collect_bounded(
        values: impl IntoIterator<Item = U256>,
        capacity: NonZeroUsize,
    ) -> Result<Self, EmptyFiniteConstantSet> {
        let mut result = BTreeSet::new();
        for value in values {
            result.insert(value);
            if result.len() > capacity.get() {
                return Ok(Self::top());
            }
        }
        Self::try_from_values(result)
    }

    /// 借用完整的有限候选；Top 返回 `None`。
    pub fn as_values(&self) -> Option<&BTreeSet<U256>> {
        match &self.0 {
            Kind::Top => None,
            Kind::Finite(values) => Some(values),
        }
    }

    /// 是否没有有限候选约束。
    pub fn is_top(&self) -> bool {
        matches!(self.0, Kind::Top)
    }

    /// 集合只有一个 word 时返回该值。
    pub fn singleton(&self) -> Option<U256> {
        self.as_values()
            .filter(|values| values.len() == 1)
            .and_then(|values| values.first().copied())
    }

    /// 有限候选数量；Top 返回 `None`。
    pub fn cardinality(&self) -> Option<usize> {
        self.as_values().map(BTreeSet::len)
    }

    /// 具体 word 是否满足本组件约束；Top 允许所有 word。
    pub fn contains(&self, value: U256) -> bool {
        self.as_values()
            .is_none_or(|values| values.contains(&value))
    }

    /// 计量组件操作的工作量：Top 为 1，有限集合为候选数量。
    pub fn work_size(&self) -> usize {
        self.cardinality().unwrap_or(1)
    }

    /// 对已有组件施加容量限制；超出容量时退化为 Top。
    pub fn limit(&self, capacity: NonZeroUsize) -> Self {
        if self.cardinality().is_some_and(|size| size > capacity.get()) {
            Self::top()
        } else {
            self.clone()
        }
    }

    /// 取得组件所有权后施加容量限制，未超限时复用已有集合。
    pub fn into_limited(self, capacity: NonZeroUsize) -> Self {
        if self.cardinality().is_some_and(|size| size > capacity.get()) {
            Self::top()
        } else {
            self
        }
    }

    /// 覆盖两个组件的并集；Top 吸收，超过容量时退化为 Top。
    pub fn join(&self, other: &Self, capacity: NonZeroUsize) -> Self {
        let (Some(left), Some(right)) = (self.as_values(), other.as_values()) else {
            return Self::top();
        };
        Self::collect_bounded(left.union(right).copied(), capacity)
            .expect("union of nonempty finite constant sets is nonempty")
    }

    /// 精确交集；Top 是单位元，空交集返回矛盾错误。
    pub fn meet(&self, other: &Self) -> Result<Self, EmptyFiniteConstantSet> {
        match (self.as_values(), other.as_values()) {
            (None, _) => Ok(other.clone()),
            (_, None) => Ok(self.clone()),
            (Some(left), Some(right)) => {
                Self::try_from_values(left.intersection(right).copied().collect())
            }
        }
    }

    /// 根据约束过滤有限候选；候选全部被排除时返回矛盾错误。
    ///
    /// Top 无法枚举，保持 Top；其他组件仍可保存过滤谓词表达的约束。
    pub fn filter(
        &self,
        mut predicate: impl FnMut(U256) -> bool,
    ) -> Result<Self, EmptyFiniteConstantSet> {
        let Some(values) = self.as_values() else {
            return Ok(Self::top());
        };
        Self::try_from_values(
            values
                .iter()
                .copied()
                .filter(|value| predicate(*value))
                .collect(),
        )
    }
}

impl Default for FiniteConstantSet {
    fn default() -> Self {
        Self::top()
    }
}

impl Serialize for FiniteConstantSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.as_values() {
            None => serializer.serialize_str("Top"),
            Some(values) => values.serialize(serializer),
        }
    }
}

impl fmt::Display for FiniteConstantSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(values) = self.as_values() else {
            return fmt::Display::fmt(&KnownBits::top(), formatter);
        };
        write!(formatter, "{{")?;
        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                write!(formatter, ", ")?;
            }
            write!(formatter, "0x{value:x}")?;
        }
        write!(formatter, "}}")
    }
}

#[cfg(test)]
mod tests;
