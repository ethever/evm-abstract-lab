//! 数值的可能来源与受信任的运行时复制身份。
//!
//! 来源只是解释信息：两个值都来自 `Storage` 不意味着它们相等。只有执行器为
//! 同一次定义分配的身份，经过 DUP 等逐值复制后，才能证明当前两个操作数相等。
//! 身份不参与持久状态的相等性、序列化或 join；否则循环会不断产生新状态。

use serde::Serialize;
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

/// 执行器内部的复制身份；用户不能从 PC、SSA 编号或来源标签伪造它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeIdentity {
    scope: u64,
    definition: u32,
}

impl RuntimeIdentity {
    /// scope 必须由执行器为每次进入基本块分配；definition 是本次进入中的新定义。
    pub(crate) fn new(scope: u64, definition: u32) -> Self {
        Self { scope, definition }
    }
}

/// 可能来源与临时复制身份。身份故意不参与抽象状态比较。
#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    origins: OriginSet,
    code_address_role: bool,
    #[serde(skip)]
    identity: Option<RuntimeIdentity>,
}

impl PartialEq for Provenance {
    fn eq(&self, other: &Self) -> bool {
        self.origins == other.origins && self.code_address_role == other.code_address_role
    }
}

impl Eq for Provenance {}

impl Provenance {
    /// 来源未知。
    pub fn top() -> Self {
        Self {
            origins: OriginSet::top(),
            code_address_role: false,
            identity: None,
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
            identity: None,
        }
    }

    /// 构造完整来源摘要。
    pub fn from_origins(origins: OriginSet) -> Self {
        Self {
            origins,
            code_address_role: false,
            identity: None,
        }
    }

    /// 来源上界，供 fact 交换使用。
    pub fn origins(&self) -> &OriginSet {
        &self.origins
    }

    /// 是否来源完全未知；身份不改变该数值无关的查询。
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

    /// 路径汇合忘记复制身份，即使两个输入碰巧持有同一个身份。
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

    /// 两个操作数是否是同一次受信任定义的复制。
    /// 数值摘要相等或来源相同均不会使此查询返回 true。
    pub fn same_identity(&self, other: &Self) -> bool {
        self.identity.is_some() && self.identity == other.identity
    }

    /// 基本块、调用、汇合或摘要边界上的保守失效入口。
    pub fn forget_identity(&mut self) {
        self.identity = None;
    }

    /// 执行器只在创建新的定义或复制一个确定值时调用此入口。
    pub(crate) fn with_identity(mut self, identity: RuntimeIdentity) -> Self {
        self.identity = Some(identity);
        self
    }
}

#[cfg(test)]
mod tests;
