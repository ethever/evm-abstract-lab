//! 域间交换的语义语言与有界的规范事实表。
//!
//! Facts 断言同一次 transfer 中成立的语义性质，而不是某个域的内部表示。
//! `MemberOf({a,b})` 是析取，绝不能编码成同时成立的 `Eq(a)` 和 `Eq(b)`。
//! 表内的重复事实被合并到固定槽位；只有语义变强时才报告 Strengthened。
//! 容量不足返回独立错误，既不产生 Bottom，也不宣称规约已经达到固定点。

use super::{congruence::Congruence, provenance::OriginSet};
use alloy_primitives::U256;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// 一次局部交换中的值编号；不等同于 SSA 编号或跨执行的运行时身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Symbol(u32);

impl Symbol {
    /// 标量规约中的当前值。
    pub const THIS: Self = Self(0);

    /// 创建局部编号。相同编号意味着调用方明确引用同一个局部值。
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// 局部编号，仅供稳定排序和显示。
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// 一个值引用或精确常量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Term {
    /// 一次交换中的值。
    Symbol(Symbol),
    /// 一个精确 EVM word。
    Constant(U256),
}

/// 验证过的位编号；最高合法位是 255。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BitIndex(u16);

impl BitIndex {
    /// 拒绝超出 word 宽度的位编号。
    pub fn new(index: u16) -> Result<Self, FactError> {
        if index < 256 {
            Ok(Self(index))
        } else {
            Err(FactError::InvalidBitIndex(index))
        }
    }

    /// 已验证的编号。
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// 批量位事实；任何位都不能同时被断言为零和一。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BitConstraints {
    zero: U256,
    one: U256,
}

impl BitConstraints {
    /// 从两个掩码构造批量事实。
    pub fn new(zero: U256, one: U256) -> Result<Self, FactError> {
        if zero & one != U256::ZERO {
            return Err(FactError::ConflictingBits);
        }
        Ok(Self { zero, one })
    }

    /// 一定为零的位。
    pub fn zero(self) -> U256 {
        self.zero
    }

    /// 一定为一的位。
    pub fn one(self) -> U256 {
        self.one
    }

    /// 此具体值是否满足掩码约束。
    pub fn contains(self, value: U256) -> bool {
        value & self.zero == U256::ZERO && value & self.one == self.one
    }

    fn meet(self, other: Self) -> Result<Self, FactError> {
        Self::new(self.zero | other.zero, self.one | other.one)
    }
}

/// 验证过的闭区间。SignedBounds 使用 value XOR 2^255 的排序坐标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct WordBounds {
    lower: U256,
    upper: U256,
}

impl WordBounds {
    /// 构造非空的线性闭区间。
    pub fn new(lower: U256, upper: U256) -> Result<Self, FactError> {
        if lower > upper {
            return Err(FactError::InvalidBounds);
        }
        Ok(Self { lower, upper })
    }

    /// 下界。
    pub fn lower(self) -> U256 {
        self.lower
    }

    /// 上界。
    pub fn upper(self) -> U256 {
        self.upper
    }

    /// 闭区间成员关系。
    pub fn contains(self, value: U256) -> bool {
        self.lower <= value && value <= self.upper
    }

    fn meet(self, other: Self) -> Result<Self, FactError> {
        Self::new(self.lower.max(other.lower), self.upper.min(other.upper))
    }
}

/// 非空的完整候选集合；表的容量还会按集合基数收费。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FiniteSet(BTreeSet<U256>);

impl FiniteSet {
    /// 取得调用方已有集合的所有权，避免构造时额外复制。
    pub fn new(values: BTreeSet<U256>) -> Result<Self, FactError> {
        if values.is_empty() {
            return Err(FactError::EmptyFiniteSet);
        }
        Ok(Self(values))
    }

    /// 借用完整候选，不需要复制。
    pub fn values(&self) -> &BTreeSet<U256> {
        &self.0
    }
}

/// 一元语义性质。来源上界与代码地址角色不会证明数值相等。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum UnaryPredicate {
    /// 精确值。
    Exact(U256),
    /// 完整候选集合的析取。
    MemberOf(FiniteSet),
    /// 一批固定为零或一的位。
    KnownBits(BitConstraints),
    /// 一个固定为一的位。
    BitSet(BitIndex),
    /// 一个固定为零的位。
    BitClear(BitIndex),
    /// 无符号线性界限。
    UnsignedBounds(WordBounds),
    /// 有符号排序坐标上的线性界限。
    SignedBounds(WordBounds),
    /// 普通整数同余与有限 word 空间的交集。
    Congruent(Congruence),
    /// 值为零。
    IsZero,
    /// 值不为零。
    NonZero,
    /// 高 96 位为零；不保证对应账户存在。
    IsAddress,
    /// 代码地址角色；不保证对应账户有代码。
    IsCodeAddress,
    /// 全部可能来源类别的上界。
    PossibleOrigins(OriginSet),
}

impl UnaryPredicate {
    /// 构造 MultipleOf(m)，拒绝无意义的零模数。
    pub fn multiple_of(modulus: U256) -> Result<Self, FactError> {
        Congruence::new(modulus, U256::ZERO)
            .map(Self::Congruent)
            .ok_or(FactError::InvalidModulus)
    }

    /// 构造普通同余；内部将余数规范到合法范围。
    pub fn congruent(modulus: U256, residue: U256) -> Result<Self, FactError> {
        Congruence::new(modulus, residue)
            .map(Self::Congruent)
            .ok_or(FactError::InvalidModulus)
    }
}

/// 对某个局部值成立的一元性质。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnaryFact {
    /// 本事实约束的局部值。
    pub subject: Symbol,
    /// 已验证的一元性质。
    pub predicate: UnaryPredicate,
}

impl UnaryFact {
    /// 绑定性质与局部值。
    pub fn new(subject: Symbol, predicate: UnaryPredicate) -> Self {
        Self { subject, predicate }
    }
}

/// 二元关系，严格区分无符号与有符号比较。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum BinaryPredicate {
    /// 两个值相等。
    Eq,
    /// 两个值不同。
    Ne,
    /// 无符号严格小于。
    Ult,
    /// 无符号小于或等于。
    Ule,
    /// 有符号严格小于。
    Slt,
    /// 有符号小于或等于。
    Sle,
}

/// 一次 transfer 中成立的二元语义关系。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BinaryFact {
    /// 左操作数。
    pub left: Term,
    /// 比较关系。
    pub predicate: BinaryPredicate,
    /// 右操作数。
    pub right: Term,
}

impl BinaryFact {
    /// 构造二元关系。Eq/Ne 的排序规范在表中进行。
    pub fn new(left: Term, predicate: BinaryPredicate, right: Term) -> Self {
        Self {
            left,
            predicate,
            right,
        }
    }
}

/// 受支持的纯 word 运算关系；操作数按 EVM 弹栈顺序排列。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OperationRelation {
    opcode: u8,
    result: Symbol,
    operands: Vec<Term>,
}

impl OperationRelation {
    /// 验证操作码与操作数个数；不接受带存储、调用等效果的操作。
    pub fn new(opcode: u8, result: Symbol, operands: Vec<Term>) -> Result<Self, FactError> {
        if !matches!(opcode, 0x01..=0x0b | 0x10..=0x1e) {
            return Err(FactError::UnsupportedOperation(opcode));
        }
        let expected = match opcode {
            0x15 | 0x19 | 0x1e => 1,
            0x08 | 0x09 => 3,
            _ => 2,
        };
        if operands.len() != expected {
            return Err(FactError::InvalidOperation {
                opcode,
                expected,
                actual: operands.len(),
            });
        }
        Ok(Self {
            opcode,
            result,
            operands,
        })
    }

    /// 纯栈操作码。
    pub fn opcode(&self) -> u8 {
        self.opcode
    }

    /// 运算结果的局部编号。
    pub fn result(&self) -> Symbol {
        self.result
    }

    /// 按弹栈顺序借用操作数。
    pub fn operands(&self) -> &[Term] {
        &self.operands
    }
}

/// 多元语义关系。运算事实由执行器的实际 transfer 构造。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RelationalFact {
    /// result = opcode(operands)。
    Operation(OperationRelation),
}

/// 域间交换的三种语义声明。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Fact {
    /// 某个值的性质。
    Unary(UnaryFact),
    /// 两个值之间的关系。
    Binary(BinaryFact),
    /// 多个值之间的关系。
    Relation(RelationalFact),
}

/// 事实插入结果；只有 Strengthened 才需要再次唤醒消费者。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactChange {
    /// 新事实增加了约束。
    Strengthened,
    /// 事实已经由表内信息涵盖。
    Unchanged,
}

/// 原始事实、语义矛盾与资源停止使用不同错误分支。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum FactError {
    /// 非法位编号。
    #[error("bit index {0} is outside a 256-bit word")]
    InvalidBitIndex(u16),
    /// 位掩码自相矛盾。
    #[error("a bit cannot be both zero and one")]
    ConflictingBits,
    /// 空线性区间。
    #[error("lower bound exceeds upper bound")]
    InvalidBounds,
    /// 可达值不能具有空候选集合。
    #[error("finite membership requires a nonempty set")]
    EmptyFiniteSet,
    /// 模数为零。
    #[error("a congruence modulus must be nonzero")]
    InvalidModulus,
    /// 非纯 word 操作。
    #[error("opcode 0x{0:02x} is not a supported pure word operation")]
    UnsupportedOperation(u8),
    /// 操作数个数不匹配。
    #[error("opcode 0x{opcode:02x} expects {expected} operands, received {actual}")]
    InvalidOperation {
        /// 操作码。
        opcode: u8,
        /// 必须的个数。
        expected: usize,
        /// 实际个数。
        actual: usize,
    },
    /// 数值约束证明局部值集合为空。
    #[error("numerical facts contradict for symbol {subject:?}")]
    Contradiction {
        /// 矛盾涉及的局部值。
        subject: Symbol,
    },
    /// 来源声明矛盾；这不会证明数值不可达。
    #[error("origin declarations contradict for symbol {subject:?}")]
    OriginContradiction {
        /// 来源声明涉及的局部值。
        subject: Symbol,
    },
    /// 交换资源限制，必须由调用方保留为不完整结果。
    #[error("fact capacity {max_atoms} cannot hold {required} semantic atoms")]
    Capacity {
        /// 表的资源上限。
        max_atoms: usize,
        /// 本次完整插入需要的容量。
        required: usize,
    },
}

/// 一个局部值的规范数值约束；没有字段表示未知，绝不表示空集合。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScalarFacts {
    finite: Option<BTreeSet<U256>>,
    bits: Option<BitConstraints>,
    unsigned: Option<WordBounds>,
    signed: Option<WordBounds>,
    congruence: Option<Congruence>,
    nonzero: bool,
    address: bool,
    code_address: bool,
    origins: Option<OriginSet>,
}

impl ScalarFacts {
    /// 完整有限候选集合。
    pub fn finite(&self) -> Option<&BTreeSet<U256>> {
        self.finite.as_ref()
    }
    /// 固定位掩码。
    pub fn known_bits(&self) -> Option<BitConstraints> {
        self.bits
    }
    /// 无符号闭区间。
    pub fn unsigned_bounds(&self) -> Option<WordBounds> {
        self.unsigned
    }
    /// 有符号偏置坐标上的闭区间。
    pub fn signed_bounds(&self) -> Option<WordBounds> {
        self.signed
    }
    /// 规范的一般同余。
    pub fn congruence(&self) -> Option<&Congruence> {
        self.congruence.as_ref()
    }
    /// 是否明确排除零。
    pub fn nonzero(&self) -> bool {
        self.nonzero
    }
    /// 是否断言合法的 160-bit 地址范围。
    pub fn is_address(&self) -> bool {
        self.address
    }
    /// 是否标记代码地址角色，不保证账户有代码。
    pub fn is_code_address(&self) -> bool {
        self.code_address
    }
    /// 全部可能来源类别的上界。
    pub fn possible_origins(&self) -> Option<&OriginSet> {
        self.origins.as_ref()
    }

    /// 具体值是否符合全部数值事实；来源与角色不会改变该判定。
    pub fn contains(&self, value: U256) -> bool {
        let bias = U256::from(1) << 255;
        self.finite.as_ref().is_none_or(|set| set.contains(&value))
            && self.bits.is_none_or(|bits| bits.contains(value))
            && self.unsigned.is_none_or(|bounds| bounds.contains(value))
            && self
                .signed
                .is_none_or(|bounds| bounds.contains(value ^ bias))
            && self.congruence.as_ref().is_none_or(|c| c.contains(value))
            && (!self.nonzero || value != U256::ZERO)
    }

    /// 被某个数值组件证明的单点；角色与来源不会参与证明。
    pub fn singleton(&self) -> Option<U256> {
        let candidate = self
            .finite
            .as_ref()
            .filter(|set| set.len() == 1)
            .and_then(|set| set.first().copied())
            .or_else(|| {
                self.bits
                    .filter(|bits| bits.zero | bits.one == U256::MAX)
                    .map(|bits| bits.one)
            })
            .or_else(|| {
                self.unsigned
                    .filter(|bounds| bounds.lower == bounds.upper)
                    .map(|bounds| bounds.lower)
            })
            .or_else(|| {
                self.signed
                    .filter(|bounds| bounds.lower == bounds.upper)
                    .map(|bounds| bounds.lower ^ (U256::from(1) << 255))
            })
            .or_else(|| self.congruence.as_ref().and_then(Congruence::singleton))?;
        self.contains(candidate).then_some(candidate)
    }

    fn atoms(&self) -> usize {
        self.finite.as_ref().map_or(0, BTreeSet::len)
            + usize::from(self.bits.is_some())
            + usize::from(self.unsigned.is_some())
            + usize::from(self.signed.is_some())
            + usize::from(self.congruence.is_some())
            + usize::from(self.nonzero)
            + usize::from(self.address)
            + usize::from(self.code_address)
            + self.origins.as_ref().map_or(0, OriginSet::work_size)
    }

    fn constrain_finite(&mut self, set: &BTreeSet<U256>) {
        self.finite = Some(match &self.finite {
            Some(current) => current.intersection(set).copied().collect(),
            None => set.clone(),
        });
    }

    fn validate(&mut self, subject: Symbol) -> Result<(), FactError> {
        let candidates = self.finite.take();
        if let Some(mut candidates) = candidates {
            candidates.retain(|value| self.contains(*value));
            if candidates.is_empty() {
                return Err(FactError::Contradiction { subject });
            }
            self.finite = Some(candidates);
        }
        if self
            .bits
            .is_some_and(|bits| bits.zero | bits.one == U256::MAX)
        {
            let exact = self.bits.expect("checked bits").one;
            if !self.contains(exact) {
                return Err(FactError::Contradiction { subject });
            }
        }
        if let Some(exact) = self.congruence.as_ref().and_then(Congruence::singleton)
            && !self.contains(exact)
        {
            return Err(FactError::Contradiction { subject });
        }
        for (bounds, signed) in [(self.unsigned, false), (self.signed, true)] {
            let Some(bounds) = bounds else {
                continue;
            };
            if bounds.lower == bounds.upper {
                let exact = if signed {
                    bounds.lower ^ (U256::from(1) << 255)
                } else {
                    bounds.lower
                };
                if !self.contains(exact) {
                    return Err(FactError::Contradiction { subject });
                }
            }
        }
        Ok(())
    }

    fn meet(&mut self, other: &Self, subject: Symbol) -> Result<(), FactError> {
        let contradiction = || FactError::Contradiction { subject };
        if let Some(finite) = &other.finite {
            self.constrain_finite(finite);
        }
        if let Some(bits) = other.bits {
            self.bits = Some(match self.bits {
                Some(current) => current.meet(bits).map_err(|_| contradiction())?,
                None => bits,
            });
        }
        if let Some(bounds) = other.unsigned {
            self.unsigned = Some(match self.unsigned {
                Some(current) => current.meet(bounds).map_err(|_| contradiction())?,
                None => bounds,
            });
        }
        if let Some(bounds) = other.signed {
            self.signed = Some(match self.signed {
                Some(current) => current.meet(bounds).map_err(|_| contradiction())?,
                None => bounds,
            });
        }
        if let Some(congruence) = &other.congruence {
            self.congruence = Some(match &self.congruence {
                Some(current) => current.meet(congruence).ok_or_else(contradiction)?,
                None => congruence.clone(),
            });
        }
        self.nonzero |= other.nonzero;
        self.address |= other.address;
        self.code_address |= other.code_address;
        if let Some(origins) = &other.origins {
            self.origins = Some(match &self.origins {
                Some(current) => current
                    .meet(origins)
                    .ok_or(FactError::OriginContradiction { subject })?,
                None => origins.clone(),
            });
        }
        self.validate(subject)
    }
}

/// 有界、规范的事实合取。此表仅用于局部规约，不能作为路径 join 的替代。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactLattice {
    max_atoms: usize,
    scalars: BTreeMap<Symbol, ScalarFacts>,
    binary: BTreeSet<BinaryFact>,
    relations: BTreeSet<RelationalFact>,
    representatives: BTreeMap<Symbol, Symbol>,
}

impl FactLattice {
    /// 设置语义原子容量；零容量允许空表，但不能插入非平凡事实。
    pub fn new(max_atoms: usize) -> Self {
        Self {
            max_atoms,
            scalars: BTreeMap::new(),
            binary: BTreeSet::new(),
            relations: BTreeSet::new(),
            representatives: BTreeMap::new(),
        }
    }

    /// 当前保存的原子数，有限候选集合按元素数收费。
    pub fn atoms(&self) -> usize {
        self.scalars.values().map(ScalarFacts::atoms).sum::<usize>()
            + self.binary.len()
            + self
                .relations
                .iter()
                .map(|relation| match relation {
                    RelationalFact::Operation(operation) => 1 + operation.operands.len(),
                })
                .sum::<usize>()
    }

    /// 借用某个局部值的规范事实。
    pub fn scalar(&self, symbol: Symbol) -> Option<&ScalarFacts> {
        self.scalars.get(&symbol)
    }

    /// 原子插入：发生矛盾或容量不足时，旧表完整保留。
    pub fn insert(&mut self, fact: Fact) -> Result<FactChange, FactError> {
        if let Fact::Unary(UnaryFact {
            predicate: UnaryPredicate::MemberOf(set),
            ..
        }) = &fact
            && set.values().len() > self.max_atoms
        {
            return Err(FactError::Capacity {
                max_atoms: self.max_atoms,
                required: set.values().len(),
            });
        }
        let mut next = self.clone();
        next.apply(fact)?;
        let required = next.atoms();
        if required > self.max_atoms {
            return Err(FactError::Capacity {
                max_atoms: self.max_atoms,
                required,
            });
        }
        if next == *self {
            return Ok(FactChange::Unchanged);
        }
        *self = next;
        Ok(FactChange::Strengthened)
    }

    fn apply(&mut self, fact: Fact) -> Result<(), FactError> {
        match fact {
            Fact::Unary(fact) => self.apply_unary(fact)?,
            Fact::Binary(mut fact) => {
                if matches!(fact.predicate, BinaryPredicate::Eq | BinaryPredicate::Ne)
                    && fact.left > fact.right
                {
                    std::mem::swap(&mut fact.left, &mut fact.right);
                }
                self.constrain_binary(&fact)?;
                if !self.binary_truth(&fact).unwrap_or(false) {
                    self.binary.insert(fact);
                }
                self.validate_relations()?;
            }
            Fact::Relation(relation) => {
                self.relations.insert(relation);
            }
        }
        self.normalize_equalities()?;
        self.validate_relations()
    }

    fn constrain_binary(&mut self, fact: &BinaryFact) -> Result<(), FactError> {
        let (subject, constant, symbol_left) = match (fact.left, fact.right) {
            (Term::Symbol(subject), Term::Constant(constant)) => (subject, constant, true),
            (Term::Constant(constant), Term::Symbol(subject)) => (subject, constant, false),
            _ => return Ok(()),
        };
        let contradiction = || FactError::Contradiction { subject };
        let predicate = match fact.predicate {
            BinaryPredicate::Eq => UnaryPredicate::Exact(constant),
            BinaryPredicate::Ne if constant == U256::ZERO => UnaryPredicate::NonZero,
            BinaryPredicate::Ne => {
                let representative = self.representative(subject);
                if let Some(scalar) = self.scalars.get_mut(&representative) {
                    if let Some(finite) = &mut scalar.finite {
                        finite.remove(&constant);
                    }
                    scalar.validate(representative)?;
                }
                return Ok(());
            }
            BinaryPredicate::Ult
            | BinaryPredicate::Ule
            | BinaryPredicate::Slt
            | BinaryPredicate::Sle => {
                let signed = matches!(fact.predicate, BinaryPredicate::Slt | BinaryPredicate::Sle);
                let strict = matches!(fact.predicate, BinaryPredicate::Ult | BinaryPredicate::Slt);
                let coordinate = if signed {
                    constant ^ (U256::from(1) << 255)
                } else {
                    constant
                };
                let bounds = if symbol_left {
                    if strict && coordinate == U256::ZERO {
                        return Err(contradiction());
                    }
                    WordBounds {
                        lower: U256::ZERO,
                        upper: if strict {
                            coordinate - U256::from(1)
                        } else {
                            coordinate
                        },
                    }
                } else {
                    if strict && coordinate == U256::MAX {
                        return Err(contradiction());
                    }
                    WordBounds {
                        lower: if strict {
                            coordinate + U256::from(1)
                        } else {
                            coordinate
                        },
                        upper: U256::MAX,
                    }
                };
                if signed {
                    UnaryPredicate::SignedBounds(bounds)
                } else {
                    UnaryPredicate::UnsignedBounds(bounds)
                }
            }
        };
        self.apply_unary(UnaryFact::new(subject, predicate))
    }

    fn representative(&self, symbol: Symbol) -> Symbol {
        self.representatives.get(&symbol).copied().unwrap_or(symbol)
    }

    /// 用每个分量最小编号作代表，将等价边规范成星形树，并共享标量约束。
    /// 总节点数和边数都受 atoms 上限约束，不建立无界反馈队列。
    fn normalize_equalities(&mut self) -> Result<(), FactError> {
        let mut symbols: BTreeSet<_> = self.scalars.keys().copied().collect();
        for fact in &self.binary {
            for term in [fact.left, fact.right] {
                if let Term::Symbol(symbol) = term {
                    symbols.insert(symbol);
                }
            }
        }
        let mut parents: BTreeMap<_, _> = symbols.iter().map(|&symbol| (symbol, symbol)).collect();
        for fact in &self.binary {
            if fact.predicate != BinaryPredicate::Eq {
                continue;
            }
            let (Term::Symbol(left), Term::Symbol(right)) = (fact.left, fact.right) else {
                continue;
            };
            let left = find_representative(&parents, left);
            let right = find_representative(&parents, right);
            if left != right {
                parents.insert(left.max(right), left.min(right));
            }
        }
        let representatives: BTreeMap<_, _> = symbols
            .into_iter()
            .map(|symbol| (symbol, find_representative(&parents, symbol)))
            .collect();
        let term_representative = |term| match term {
            Term::Symbol(symbol) => Term::Symbol(representatives[&symbol]),
            Term::Constant(_) => term,
        };
        let mut binary = BTreeSet::new();
        for (&symbol, &representative) in &representatives {
            if symbol != representative {
                binary.insert(BinaryFact::new(
                    Term::Symbol(representative),
                    BinaryPredicate::Eq,
                    Term::Symbol(symbol),
                ));
            }
        }
        for fact in &self.binary {
            if fact.predicate == BinaryPredicate::Eq
                && matches!((fact.left, fact.right), (Term::Symbol(_), Term::Symbol(_)))
            {
                continue;
            }
            let mut fact = fact.clone();
            fact.left = term_representative(fact.left);
            fact.right = term_representative(fact.right);
            if matches!(fact.predicate, BinaryPredicate::Eq | BinaryPredicate::Ne)
                && fact.left > fact.right
            {
                std::mem::swap(&mut fact.left, &mut fact.right);
            }
            binary.insert(fact);
        }
        let mut numerical: BTreeMap<Symbol, ScalarFacts> = BTreeMap::new();
        for (&symbol, scalar) in &self.scalars {
            let representative = representatives[&symbol];
            let mut scalar = scalar.clone();
            scalar.origins = None;
            scalar.code_address = false;
            numerical
                .entry(representative)
                .or_default()
                .meet(&scalar, representative)?;
        }
        let mut scalars = BTreeMap::new();
        for (&symbol, &representative) in &representatives {
            let mut scalar = numerical.get(&representative).cloned().unwrap_or_default();
            if let Some(local) = self.scalars.get(&symbol) {
                scalar.origins = local.origins.clone();
                scalar.code_address = local.code_address;
            }
            if scalar != ScalarFacts::default() {
                scalars.insert(symbol, scalar);
            }
        }
        self.binary = binary;
        self.scalars = scalars;
        self.representatives = representatives;
        self.binary = self
            .binary
            .iter()
            .filter(|fact| {
                (fact.predicate == BinaryPredicate::Eq
                    && matches!((fact.left, fact.right), (Term::Symbol(_), Term::Symbol(_))))
                    || self.binary_truth(fact) != Some(true)
            })
            .cloned()
            .collect();
        Ok(())
    }

    fn apply_unary(&mut self, fact: UnaryFact) -> Result<(), FactError> {
        let subject = fact.subject;
        let scalar = self.scalars.entry(subject).or_default();
        let contradiction = || FactError::Contradiction { subject };
        let bit_set = matches!(&fact.predicate, UnaryPredicate::BitSet(_));
        match fact.predicate {
            UnaryPredicate::Exact(value) => {
                scalar.constrain_finite(&BTreeSet::from([value]));
            }
            UnaryPredicate::MemberOf(set) => scalar.constrain_finite(set.values()),
            UnaryPredicate::KnownBits(bits) => {
                scalar.bits = Some(match scalar.bits {
                    Some(current) => current.meet(bits).map_err(|_| contradiction())?,
                    None => bits,
                });
            }
            UnaryPredicate::BitSet(index) | UnaryPredicate::BitClear(index) => {
                let mask = U256::from(1) << usize::from(index.get());
                let bits = if bit_set {
                    BitConstraints {
                        zero: U256::ZERO,
                        one: mask,
                    }
                } else {
                    BitConstraints {
                        zero: mask,
                        one: U256::ZERO,
                    }
                };
                scalar.bits = Some(match scalar.bits {
                    Some(current) => current.meet(bits).map_err(|_| contradiction())?,
                    None => bits,
                });
            }
            UnaryPredicate::UnsignedBounds(bounds) => {
                scalar.unsigned = Some(match scalar.unsigned {
                    Some(current) => current.meet(bounds).map_err(|_| contradiction())?,
                    None => bounds,
                });
            }
            UnaryPredicate::SignedBounds(bounds) => {
                scalar.signed = Some(match scalar.signed {
                    Some(current) => current.meet(bounds).map_err(|_| contradiction())?,
                    None => bounds,
                });
            }
            UnaryPredicate::Congruent(congruence) => {
                scalar.congruence = Some(match &scalar.congruence {
                    Some(current) => current.meet(&congruence).ok_or_else(contradiction)?,
                    None => congruence,
                });
            }
            UnaryPredicate::IsZero => scalar.constrain_finite(&BTreeSet::from([U256::ZERO])),
            UnaryPredicate::NonZero => scalar.nonzero = true,
            UnaryPredicate::IsAddress => {
                scalar.address = true;
                let address_bits = BitConstraints {
                    zero: U256::MAX << 160,
                    one: U256::ZERO,
                };
                scalar.bits = Some(match scalar.bits {
                    Some(current) => current.meet(address_bits).map_err(|_| contradiction())?,
                    None => address_bits,
                });
            }
            UnaryPredicate::IsCodeAddress => scalar.code_address = true,
            UnaryPredicate::PossibleOrigins(origins) => {
                scalar.origins = Some(match &scalar.origins {
                    Some(current) => current
                        .meet(&origins)
                        .ok_or(FactError::OriginContradiction { subject })?,
                    None => origins,
                });
            }
        }
        scalar.validate(subject)
    }

    /// 由显式 Eq 事实构成的等价关系；不使用 PC、来源或摘要相似性。
    pub fn equivalent(&self, left: Symbol, right: Symbol) -> bool {
        self.representative(left) == self.representative(right)
    }

    fn binary_truth(&self, fact: &BinaryFact) -> Option<bool> {
        let same = match (fact.left, fact.right) {
            (Term::Symbol(a), Term::Symbol(b)) => self.equivalent(a, b),
            (a, b) => a == b,
        };
        if same {
            return Some(matches!(
                fact.predicate,
                BinaryPredicate::Eq | BinaryPredicate::Ule | BinaryPredicate::Sle
            ));
        }
        let concrete = |term| match term {
            Term::Constant(value) => Some(value),
            Term::Symbol(symbol) => self.scalar(symbol).and_then(ScalarFacts::singleton),
        };
        let (Some(left), Some(right)) = (concrete(fact.left), concrete(fact.right)) else {
            return None;
        };
        let bias = U256::from(1) << 255;
        Some(match fact.predicate {
            BinaryPredicate::Eq => left == right,
            BinaryPredicate::Ne => left != right,
            BinaryPredicate::Ult => left < right,
            BinaryPredicate::Ule => left <= right,
            BinaryPredicate::Slt => left ^ bias < right ^ bias,
            BinaryPredicate::Sle => left ^ bias <= right ^ bias,
        })
    }

    fn validate_relations(&self) -> Result<(), FactError> {
        for fact in &self.binary {
            if self.binary_truth(fact) == Some(false) {
                let subject = match (fact.left, fact.right) {
                    (Term::Symbol(subject), _) | (_, Term::Symbol(subject)) => subject,
                    _ => Symbol::THIS,
                };
                return Err(FactError::Contradiction { subject });
            }
        }
        Ok(())
    }

    /// 导出规范事实；最多分配与 atoms() 成比例的空间，顺序稳定。
    pub fn facts(&self) -> Vec<Fact> {
        let mut facts = Vec::new();
        for (&subject, scalar) in &self.scalars {
            let mut push = |predicate| facts.push(Fact::Unary(UnaryFact::new(subject, predicate)));
            if let Some(finite) = &scalar.finite {
                push(UnaryPredicate::MemberOf(FiniteSet(finite.clone())));
            }
            if let Some(bits) = scalar.bits {
                push(UnaryPredicate::KnownBits(bits));
            }
            if let Some(unsigned) = scalar.unsigned {
                push(UnaryPredicate::UnsignedBounds(unsigned));
            }
            if let Some(signed) = scalar.signed {
                push(UnaryPredicate::SignedBounds(signed));
            }
            if let Some(congruence) = &scalar.congruence {
                push(UnaryPredicate::Congruent(congruence.clone()));
            }
            if scalar.nonzero {
                push(UnaryPredicate::NonZero);
            }
            if scalar.address {
                push(UnaryPredicate::IsAddress);
            }
            if scalar.code_address {
                push(UnaryPredicate::IsCodeAddress);
            }
            if let Some(origins) = &scalar.origins {
                push(UnaryPredicate::PossibleOrigins(origins.clone()));
            }
        }
        facts.extend(self.binary.iter().cloned().map(Fact::Binary));
        facts.extend(self.relations.iter().cloned().map(Fact::Relation));
        facts
    }
}

fn find_representative(parents: &BTreeMap<Symbol, Symbol>, mut symbol: Symbol) -> Symbol {
    while parents[&symbol] != symbol {
        symbol = parents[&symbol];
    }
    symbol
}

#[cfg(test)]
mod tests;
