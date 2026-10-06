//! EVM Word256 的静态组合域与有界语义事实交换。
//!
//! 每个值的含义是常量、KnownBits、U/S 区间、一般同余约束的交集。
//! 工作表保存笛卡尔组件的真实 join；临时规约不参与存储 join，因而
//! 不把有界、不完全的 fact 传播误称为 canonical reduced lattice。
//! 来源标签描述可能来源，只有引擎签发的局部复制身份证明同一个运行时值。

use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};

mod concrete;
pub mod congruence;
pub mod facts;
pub mod finite_constant_set;
pub mod interval;
pub mod known_bits;
pub mod provenance;
mod query;
mod reduce;
mod spec;
mod transfer;
mod value;

use concrete::evaluate;
pub use finite_constant_set::{EmptyFiniteConstantSet, FiniteConstantSet};
pub use reduce::{Reduction, ReductionStatus};
pub use spec::{DomainSpec, Profile};
pub use value::Value;

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
    /// 在相同语义输入上切换表示。有限候选完整保留；开放约束在常量对照中变粗。
    pub fn project(self, value: &Value) -> Value {
        if self.spec.profile() == Profile::Product {
            let mut value = value.clone();
            value.forget_identity();
            value.finite = value.finite.into_limited(self.spec.constant_capacity());
            return value;
        }
        query::candidates(value, self.capacity())
            .filter(|s| !s.is_empty() && s.len() <= self.capacity())
            .map_or_else(Value::top, |values| {
                Self::finite(
                    FiniteConstantSet::try_from_values(values)
                        .expect("candidate filter established a nonempty set"),
                )
            })
    }
    /// 从受控的一元语义事实建立初始值。矛盾与容量不足不会变成 Top/空成功。
    pub fn from_facts(self, facts: &[facts::UnaryPredicate]) -> Result<Value, facts::FactError> {
        let mut lattice = facts::FactLattice::new(self.spec.fact_limit());
        for predicate in facts {
            lattice.insert(facts::Fact::Unary(facts::UnaryFact::new(
                facts::Symbol::THIS,
                predicate.clone(),
            )))?;
        }
        let initial = reduce::import(&Value::top(), &lattice)?;
        let reduced = reduce::reduce(self, initial);
        if reduced.status == ReductionStatus::Empty {
            return Err(facts::FactError::Contradiction {
                subject: facts::Symbol::THIS,
            });
        }
        if reduced.status == ReductionStatus::OriginConflict {
            return Err(facts::FactError::OriginContradiction {
                subject: facts::Symbol::THIS,
            });
        }
        let mut result = reduced.value;
        result.finite = result.finite.into_limited(self.spec.constant_capacity());
        if query::candidates(&result, self.capacity()).is_some_and(|s| s.is_empty()) {
            return Err(facts::FactError::Contradiction {
                subject: facts::Symbol::THIS,
            });
        }
        Ok(self.project(&result))
    }
    /// 临时传播已有约束，不改变原值及存储 join 的合同。
    pub fn reduce(self, value: &Value) -> Reduction {
        reduce::reduce(self, value.clone())
    }
    /// stored anchor 上的区间 widening；其他组件仍保留 next 的保证。
    pub(crate) fn widen(self, old: &Value, next: &Value) -> Value {
        let mut out = next.clone();
        if self.spec.profile() == Profile::Product {
            out.interval = old.interval.widen(&next.interval);
        }
        out.forget_identity();
        out
    }
    /// 逐组件最小上界；不运行 fact 交换。路径特有断言只能保留共同保证。
    pub fn join(&self, left: &Value, right: &Value) -> Value {
        let finite = left
            .finite
            .join(&right.finite, self.spec.constant_capacity());
        if self.spec.profile() == Profile::ConstantsOnly {
            return Self::finite(finite);
        }
        Value {
            finite,
            bits: left.bits.join(&right.bits),
            interval: left.interval.join(&right.interval),
            congruence: left.congruence.join(&right.congruence),
            provenance: left.provenance.join(&right.provenance),
            nonzero: left.nonzero && right.nonzero,
        }
    }
    fn finite(finite: FiniteConstantSet) -> Value {
        let Some(values) = finite.as_values() else {
            return Value::top();
        };
        let bits = known_bits::KnownBits::from_values(values);
        let interval = interval::Interval::from_values(values);
        let congruence = congruence::Congruence::from_values(values);
        let nonzero = !values.contains(&U256::ZERO);
        Value {
            bits,
            interval,
            congruence,
            nonzero,
            finite,
            provenance: provenance::Provenance::constant(),
        }
    }
    fn collect(&self, values: impl IntoIterator<Item = U256>) -> Value {
        Self::finite(
            FiniteConstantSet::collect_bounded(values, self.spec.constant_capacity())
                .expect("reachable finite values are nonempty"),
        )
    }
    fn finite_apply(&self, op: u8, args: &[Value]) -> Value {
        let Some(sets) = args
            .iter()
            .map(Value::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                self.collect((0_u64..=256).map(U256::from))
            } else if matches!(op, 0x10..=0x15) {
                self.collect([U256::ZERO, U256::from(1)])
            } else {
                Value::top()
            };
        };
        let singleton = BTreeSet::from([U256::ZERO]);
        let second = sets.get(1).copied().unwrap_or(&singleton);
        let third = sets.get(2).copied().unwrap_or(&singleton);
        let results = sets[0].iter().flat_map(|a| {
            second
                .iter()
                .flat_map(move |b| third.iter().map(move |c| evaluate(op, *a, *b, *c)))
        });
        self.collect(results)
    }
    /// 纯栈运算，栈顶先弹出；未知/不支持的纯数值运算保守产生 Top。
    /// 引擎在执行前从根账本预留包含交换的工作上界。
    pub fn apply(&self, op: u8, args: &[Value]) -> Value {
        self.apply_detailed(op, args).value
    }
    /// 库调用方可使用和引擎相同的根账本；失败时不返回部分数值结果。
    pub fn apply_budgeted(
        self,
        op: u8,
        args: &[Value],
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<Reduction, WorkExhausted> {
        if !budget.charge(self.operation_work(args)) {
            return Err(WorkExhausted);
        }
        Ok(self.apply_detailed(op, args))
    }
    /// EVM 地址取低 160 位。原 word 的高位未知不妨碍投影后成为单点。
    pub fn address_projection(self, value: &Value) -> Value {
        let mut projected = self.apply(
            opcode::AND,
            &[value.clone(), Value::constant(U256::MAX >> 96usize)],
        );
        projected.provenance = projected.provenance.with_code_address_role();
        projected
    }
    /// 包含交换状态的查询入口，便于区分稳定和可选精度截断。
    pub fn apply_detailed(&self, op: u8, args: &[Value]) -> Reduction {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return Reduction::unchanged(Value::top());
        }
        if self.spec.profile() == Profile::ConstantsOnly {
            return Reduction::unchanged(self.finite_apply(op, args));
        }
        transfer::apply(*self, op, args)
    }
    /// 纯数值运算及其有界 fact 交换的逻辑工作上界。
    /// 费用以 limb/位规则为单位，与策略版本一起冻结。
    pub fn operation_work(self, args: &[Value]) -> usize {
        let tuples = args.iter().fold(1usize, |n, v| {
            n.saturating_mul(v.constants().map_or(1, BTreeSet::len))
        });
        if self.spec.profile() == Profile::ConstantsOnly {
            return tuples;
        }
        if !args.is_empty()
            && args.iter().all(|v| v.constants().is_some())
            && tuples <= self.capacity()
        {
            return tuples
                .saturating_mul(256)
                .saturating_add(args.iter().map(Value::work_size).sum::<usize>());
        }
        // 一个 credit 覆盖 256 个 elementary bit steps；固定宽整数算法另计。
        // 每轮 ≤15 次标量插入，≤2 段 DP，≤capacity 完整候选，及 gcd/CRT。
        let scalar_atoms = self.capacity().saturating_add(32);
        let round = 4096usize
            .saturating_add(256usize.saturating_mul(self.capacity()))
            .saturating_add(32usize.saturating_mul(scalar_atoms));
        tuples
            .saturating_add(4096)
            .saturating_add(round.saturating_mul(self.spec.reduction_rounds()))
            .saturating_add(args.iter().map(Value::work_size).sum::<usize>())
    }
}
