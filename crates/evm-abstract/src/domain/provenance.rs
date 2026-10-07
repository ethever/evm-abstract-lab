//! Source metadata only: possible origins and a code-address role.
//!
//! Two values from Storage are not thereby equal. Value identities live in
//! the separate identity module and are not changed by source metadata updates.

use serde::Serialize;
// Keep the input-name path compatible with existing environment callers.
pub use super::identity::Symbol;
use std::collections::BTreeSet;

/// 来源集合的固定容量；超出后升到 Top，保留可能来源的覆盖性。
pub const MAX_ORIGINS: usize = 16;

/// 一个可能的来源类别；它不证明地址存在、存储位置相同或数值相等。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Origin {
    /// 字节码中的常量或由外部提供的精确常量。
    Constant,
    /// 调用输入。
    Calldata,
    /// 当前帧的内存。
    Memory,
    /// 持久存储。
    Storage,
    /// 瞬态存储。
    TransientStorage,
    /// 执行环境字段。
    Environment,
    /// 地址字段；范围保证仍由数值域独立保存。
    Address,
    /// 作为代码地址使用；不声明对应账户有代码。
    CodeAddress,
    /// 当前调用的 CALLVALUE。
    CallValue,
    /// 最近一次子调用的返回数据。
    Returndata,
    /// 栈上的数值运算。
    Arithmetic,
    /// 账户余额。
    Balance,
    /// 账户 nonce。
    Nonce,
}

/// 完整的可能来源上界；空集合没有合法含义，未知来源使用 Top。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
enum OriginKind {
    #[serde(rename = "Sources")]
    Sources(BTreeSet<Origin>),
    Top,
}

/// 私有表示禁止空来源集合。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct OriginSet(OriginKind);

impl OriginSet {
    /// 来源未知。
    pub fn top() -> Self {
        Self(OriginKind::Top)
    }

    /// 一个已知来源类别。
    pub fn source(origin: Origin) -> Self {
        Self(OriginKind::Sources(BTreeSet::from([origin])))
    }

    /// 构造非空的完整来源集合；容量溢出保守地返回 Top。
    pub fn from_sources(sources: impl IntoIterator<Item = Origin>) -> Option<Self> {
        let sources: BTreeSet<_> = sources.into_iter().collect();
        if sources.is_empty() {
            return None;
        }
        Some(if sources.len() > MAX_ORIGINS {
            Self::top()
        } else {
            Self(OriginKind::Sources(sources))
        })
    }

    /// 借用完整集合；None 表示来源未知。
    pub fn sources(&self) -> Option<&BTreeSet<Origin>> {
        match &self.0 {
            OriginKind::Sources(sources) => Some(sources),
            OriginKind::Top => None,
        }
    }

    /// 是否没有完整的来源上界。
    pub fn is_top(&self) -> bool {
        self.sources().is_none()
    }

    /// 路径汇合需要并集，因为任一路径的来源仍可能出现。
    pub fn join(&self, other: &Self) -> Self {
        match (self.sources(), other.sources()) {
            (Some(left), Some(right)) => Self::from_sources(left.union(right).copied())
                .expect("the union of nonempty source sets is nonempty"),
            _ => Self::top(),
        }
    }

    /// 同一个值的两个完整来源上界可以求交；空交表示来源声明互相矛盾。
    /// 这不是数值 Bottom，不得据此丢弃数值仍可达的执行路径。
    pub fn meet(&self, other: &Self) -> Option<Self> {
        match (self.sources(), other.sources()) {
            (Some(left), Some(right)) => Self::from_sources(left.intersection(right).copied()),
            (Some(_), None) => Some(self.clone()),
            (None, Some(_)) => Some(other.clone()),
            (None, None) => Some(Self::top()),
        }
    }

    /// 保存和交换这个来源摘要时需要复制的类别数。
    pub fn work_size(&self) -> usize {
        self.sources().map_or(1, BTreeSet::len)
    }
}

/// Possible source categories and a must code-address role; no value identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Provenance {
    origins: OriginSet,
    code_address_role: bool,
}

impl Provenance {
    /// 来源未知。
    pub fn top() -> Self {
        Self {
            origins: OriginSet::top(),
            code_address_role: false,
        }
    }

    /// 常量来源。
    pub fn constant() -> Self {
        Self::source(Origin::Constant)
    }

    /// 一个已知来源类别；任何数值约束都必须另行建立。
    pub fn source(origin: Origin) -> Self {
        Self {
            origins: OriginSet::source(origin),
            code_address_role: false,
        }
    }

    /// 构造完整来源摘要。
    pub fn from_origins(origins: OriginSet) -> Self {
        Self {
            origins,
            code_address_role: false,
        }
    }

    /// 来源上界，供 fact 交换使用。
    pub fn origins(&self) -> &OriginSet {
        &self.origins
    }

    /// 来源上界和代码地址角色均未知。
    pub fn is_top(&self) -> bool {
        self.origins.is_top() && !self.code_address_role
    }

    /// 是否在全部被覆盖的执行中作为代码地址使用。
    /// 该角色不证明数值范围、账户存在或账户有代码。
    pub fn is_code_address(&self) -> bool {
        self.code_address_role
    }

    /// 标记确定的代码地址角色；数值保证仍由数值域独立建立。
    pub fn with_code_address_role(mut self) -> Self {
        self.code_address_role = true;
        self
    }

    /// 汇合可能来源，并只保留两条路径都具有的代码地址角色。
    pub fn join(&self, other: &Self) -> Self {
        let mut joined = Self::from_origins(self.origins.join(&other.origins));
        joined.code_address_role = self.code_address_role && other.code_address_role;
        joined
    }

    /// 运算结果可能含任一操作数的来源，并增加 Arithmetic 类别。
    pub fn transfer(args: &[Self]) -> Self {
        let mut origins = OriginSet::source(Origin::Arithmetic);
        for arg in args {
            origins = origins.join(arg.origins());
        }
        Self::from_origins(origins)
    }

    pub(crate) fn set_origin(&mut self, origin: Origin) {
        self.origins = OriginSet::source(origin);
    }
}

#[cfg(test)]
mod tests;
