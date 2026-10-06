//! 数值的可能来源与受信任的运行时复制身份。
//!
//! 来源只是解释信息：两个值都来自 `Storage` 不意味着它们相等。只有执行器为
//! 同一次定义分配的身份，经过 DUP 等逐值复制后，才能证明当前两个操作数相等。
//! 临时复制身份不参与持久状态比较或序列化；固定环境符号则参与比较，并且
//! 只有全部输入保持同一个符号时才跨 join 保留。

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

/// Stable identity of an immutable input shared by every frame and block.
///
/// Equal identities assert equal concrete values within one environment. The
/// environment is part of summary qualification; arithmetic creates fresh values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Symbol {
    /// Logical root ADDRESS when analyzing bytecode without a destination.
    To,
    /// Unspecified root caller, also the default transaction origin.
    Caller,
    /// Independently specified symbolic transaction origin.
    Origin,
    /// Symbolic block beneficiary.
    Coinbase,
    /// Root frame's immutable call value.
    CallValue,
    /// Effective transaction gas price.
    GasPrice,
    /// Block timestamp.
    Timestamp,
    /// Block number.
    Number,
    /// Block randomness.
    Prevrandao,
    /// Block gas limit.
    GasLimit,
    /// Execution chain identifier when no anchored identity is available.
    ChainId,
    /// Block base fee.
    BaseFee,
    /// Blob base fee.
    BlobBaseFee,
    /// Root calldata length.
    CalldataLength,
    /// One root calldata word at an exact byte offset.
    CalldataWord(alloy_primitives::U256),
    /// A valid historical block hash at an exact height.
    BlockHash(alloy_primitives::U256),
    /// A blob versioned hash at an exact index.
    BlobHash(alloy_primitives::U256),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct InputIdentity {
    #[serde(skip)]
    scope: u64,
    name: Symbol,
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

/// 来源、临时复制身份和可跨控制流保留的固定环境符号。
#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    origins: OriginSet,
    code_address_role: bool,
    #[serde(skip)]
    identity: Option<RuntimeIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<InputIdentity>,
}

impl PartialEq for Provenance {
    fn eq(&self, other: &Self) -> bool {
        self.origins == other.origins
            && self.code_address_role == other.code_address_role
            && self.symbol == other.symbol
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
            symbol: None,
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
            symbol: None,
        }
    }

    /// 构造完整来源摘要。
    pub fn from_origins(origins: OriginSet) -> Self {
        Self {
            origins,
            code_address_role: false,
            identity: None,
            symbol: None,
        }
    }

    /// 来源上界，供 fact 交换使用。
    pub fn origins(&self) -> &OriginSet {
        &self.origins
    }

    /// 是否来源完全未知；身份不改变该数值无关的查询。
    pub fn is_top(&self) -> bool {
        self.origins.is_top() && !self.code_address_role && self.symbol.is_none()
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
        joined.symbol = self.symbol.filter(|symbol| Some(*symbol) == other.symbol);
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

    /// 两个操作数是否持有同一个固定环境符号或临时复制身份。
    /// 数值摘要相等或来源相同均不会使此查询返回 true。
    pub fn same_identity(&self, other: &Self) -> bool {
        (self.symbol.is_some() && self.symbol == other.symbol)
            || (self.identity.is_some() && self.identity == other.identity)
    }

    pub(crate) fn same_symbol(&self, other: &Self) -> bool {
        self.symbol.is_some() && self.symbol == other.symbol
    }

    pub(crate) fn preserve_symbol_from(&mut self, other: &Self) {
        self.symbol = other.symbol;
    }

    /// 基本块、调用、汇合或摘要边界上的保守失效入口。
    pub fn forget_identity(&mut self) {
        self.identity = None;
    }

    /// Preserve an immutable environment identity across control-flow boundaries.
    pub(crate) fn with_symbol(mut self, symbol: Symbol, scope: u64) -> Self {
        self.symbol = Some(InputIdentity {
            scope,
            name: symbol,
        });
        self
    }

    pub(crate) fn set_origin(&mut self, origin: Origin) {
        self.origins = OriginSet::source(origin);
    }

    /// 执行器只在创建新的定义或复制一个确定值时调用此入口。
    pub(crate) fn with_identity(mut self, identity: RuntimeIdentity) -> Self {
        self.identity = Some(identity);
        self
    }
}

#[cfg(test)]
mod tests;
