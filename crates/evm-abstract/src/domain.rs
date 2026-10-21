//! EVM Word256 的静态组合域与有界语义事实交换。
//!
//! 每个值的含义是常量、KnownBits、U/S 区间、一般同余约束的交集。
//! 工作表保存笛卡尔组件的真实 join；临时规约不参与存储 join，因而
//! 不把有界、不完全的 fact 传播误称为 canonical reduced lattice。
//! NumericValue 只持有数值组件；AbstractValue 另持来源、值身份和可选表达式。
//! 来源标签不证明相等；身份与跨值关系通过独立的类型和规则提供证据。

use std::num::NonZeroUsize;

mod algebra;
mod byte_projection;
mod concrete;
pub mod congruence;
pub mod cooperation;
mod element;
pub mod facts;
pub mod finite_constant_set;
pub mod identity;
pub mod interval;
pub mod known_bits;
mod numeric;
mod observation;
mod product;
pub mod provenance;
mod query;
mod reduce;
pub mod relational;
mod spec;
pub mod symbolic;
mod transfer;
mod value;
mod widening;

pub(crate) use algebra::SingletonProof;
pub use algebra::{AlgebraRef, EvmNumericAlgebra, NumericReduction, ValueAlgebra};
use concrete::evaluate;
pub use element::WordElement;
pub use finite_constant_set::{EmptyFiniteConstantSet, FiniteConstantSet};
pub use identity::{InputIdentity, RuntimeIdentity, ValueIdentity};
pub use numeric::NumericValue;
pub use observation::WordObservation;
pub use reduce::{EvmReduction, Reduction, ReductionStatus};
pub use spec::{DomainSpec, Profile};
pub use value::{AbstractValue, EvmValue};

/// 根工作账本拒绝整项数值运算；不是 EVM gas 不足。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("shared domain work budget exhausted")]
pub struct WorkExhausted;

/// 一次分析共用的冻结策略；全部派发是静态的。
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    spec: DomainSpec,
}
impl Default for Domain {
    fn default() -> Self {
        Self::from_spec(DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        ))
    }
}
impl Domain {
    /// 原有限常量域的兼容构造器；新分析默认使用组合域。
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self::from_spec(DomainSpec::new(
            Profile::ConstantsOnly,
            capacity,
            NonZeroUsize::new(1).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        ))
    }
    /// 从已经固定的策略构造，不为配置上限预分配集合。
    pub fn from_spec(spec: DomainSpec) -> Self {
        Self { spec }
    }
    /// 冻结的全量策略，供输出和摘要兼容性 guard 使用。
    pub fn spec(self) -> DomainSpec {
        self.spec
    }
    /// 完整有限集合的容量。
    pub fn capacity(self) -> usize {
        self.spec.capacity()
    }
    #[cfg(test)]
    fn finite(finite: FiniteConstantSet) -> AbstractValue {
        let mut value = AbstractValue::from_numeric(product::finite(finite));
        if !value.numeric().is_top() {
            value.provenance = provenance::Provenance::constant();
        }
        value
    }
    /// A single borrowed facade preserves metadata and the exact default policy.
    fn algebra(&self) -> AlgebraRef<'_, Self> {
        ValueAlgebra::new(self)
    }
    /// Project representation under this frozen policy.
    pub fn project(self, value: &AbstractValue) -> AbstractValue {
        self.algebra().project(value)
    }
    /// Construct a value from validated unary guarantees.
    pub fn from_facts(
        self,
        facts: &[facts::UnaryPredicate],
    ) -> Result<AbstractValue, facts::FactError> {
        self.algebra().from_facts(facts)
    }
    /// Local bounded reduction, separate from stored joins.
    pub fn reduce(self, value: &AbstractValue) -> Reduction {
        self.algebra().reduce(value)
    }
    /// Global acceleration, retaining shared metadata contracts.
    #[cfg(test)]
    pub(crate) fn widen(self, old: &AbstractValue, next: &AbstractValue) -> AbstractValue {
        self.algebra().widen(old, next)
    }
    /// Stored component join, without reduction.
    pub fn join(&self, left: &AbstractValue, right: &AbstractValue) -> AbstractValue {
        self.algebra().join(left, right)
    }
    /// Pure EVM numeric operation in pop order.
    pub fn apply(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        self.algebra().apply(op, args)
    }
    /// Prepay against the caller's root ledger before executing.
    pub fn apply_budgeted(
        self,
        op: u8,
        args: &[AbstractValue],
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<Reduction, WorkExhausted> {
        self.algebra().apply_budgeted(op, args, budget)
    }
    /// Truncate to a 160-bit EVM address.
    pub fn address_projection(self, value: &AbstractValue) -> AbstractValue {
        self.algebra().address_projection(value)
    }
    /// Numeric result and optional precision/retention status.
    pub fn apply_detailed(&self, op: u8, args: &[AbstractValue]) -> Reduction {
        self.algebra().apply_detailed(op, args)
    }
    /// Allocation-free borrowed quotation, before actual numeric execution.
    pub fn operation_work(self, args: &[AbstractValue]) -> usize {
        self.algebra().operation_work(args)
    }
    /// Guard import/reduction work, excluding already paid input copying.
    #[cfg(test)]
    pub(crate) fn refinement_work(self, value: &AbstractValue, facts: usize) -> usize {
        self.algebra().refinement_work(value, facts)
    }
}
