//! EVM Word256 的静态组合域与有界语义事实交换。
//!
//! 每个值的含义是常量、KnownBits、U/S 区间、一般同余约束的交集。
//! 工作表保存笛卡尔组件的真实 join；临时规约不参与存储 join，因而
//! 不把有界、不完全的 fact 传播误称为 canonical reduced lattice。
//! NumericValue 只持有数值组件；EvmValue 另持来源、值身份和可选表达式。
//! 来源标签不证明相等；身份与跨值关系通过独立的类型和规则提供证据。

mod algebra;
mod byte_projection;
mod concrete;
pub mod congruence;
pub mod cooperation;
mod element;
pub mod facts;
mod finite_algebra;
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
pub use cooperation::{BuiltinWork, ProductAlgebra};
pub use element::WordElement;
pub use finite_algebra::FiniteSetAlgebra;
pub use finite_constant_set::{EmptyFiniteConstantSet, FiniteConstantSet};
pub use identity::{InputIdentity, RuntimeIdentity, ValueIdentity};
pub use numeric::NumericValue;
pub use observation::WordObservation;
pub use reduce::{EvmReduction, ReductionStatus};
pub use spec::DomainSpec;
pub use value::EvmValue;

/// 根工作账本拒绝整项数值运算；不是 EVM gas 不足。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("shared domain work budget exhausted")]
pub struct WorkExhausted;
