//! EVM Word256 的静态组合域与有界语义事实交换。
//!
//! 每个值的含义是常量、KnownBits、U/S 区间、一般同余约束的交集。
//! 工作表保存笛卡尔组件的真实 join；临时规约不参与存储 join，因而
//! 不把有界、不完全的 fact 传播误称为 canonical reduced lattice。
//! NumericValue 只持有数值组件；AbstractValue 另持来源、值身份和可选表达式。
//! 来源标签不证明相等；身份与跨值关系通过独立的类型和规则提供证据。

use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};

mod concrete;
pub mod congruence;
pub mod facts;
pub mod finite_constant_set;
pub mod identity;
pub mod interval;
pub mod known_bits;
mod numeric;
pub mod provenance;
mod query;
mod reduce;
pub mod relational;
mod spec;
pub mod symbolic;
mod transfer;
mod value;

use concrete::evaluate;
pub use finite_constant_set::{EmptyFiniteConstantSet, FiniteConstantSet};
pub use identity::{InputIdentity, RuntimeIdentity, ValueIdentity};
pub use numeric::NumericValue;
pub use reduce::{Reduction, ReductionStatus};
pub use spec::{DomainSpec, Profile};
pub use value::{AbstractValue, Value};

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
    /// 初始化投影新增的候选扫描与数值摘要工作；已有输入复制另计。
    /// 开放约束按实际查询的完整覆盖上界计费，配置容量本身不产生工作。
    pub(crate) fn projection_work(self, value: &AbstractValue) -> usize {
        if self.spec.profile() == Profile::Product {
            return 0;
        }
        query::candidate_visits(value.numeric(), self.capacity()).saturating_mul(256)
    }
    /// 在相同语义输入上切换表示。有限候选完整保留；开放约束在常量对照中变粗。
    pub fn project(self, value: &AbstractValue) -> AbstractValue {
        if self.spec.profile() == Profile::Product {
            let mut value = value.clone();
            value.forget_identity();
            if !self.spec.relations().enabled {
                value.forget_expression();
                value.symbolic_limit = false;
            }
            value.numeric.finite = value
                .numeric
                .finite
                .into_limited(self.spec.constant_capacity());
            return value;
        }
        let mut projected = query::candidates(value.numeric(), self.capacity())
            .filter(|s| !s.is_empty() && s.len() <= self.capacity())
            .map_or_else(AbstractValue::top, |values| {
                Self::finite(
                    FiniteConstantSet::try_from_values(values)
                        .expect("candidate filter established a nonempty set"),
                )
            });
        // Immutable environment aliases are semantic input facts, independent
        // of the numeric component profile. Temporary copy IDs still disappear.
        projected.provenance = value.provenance.clone();
        projected.identity = value.identity.join(&value.identity);
        projected.symbolic_limit = self.spec.relations().enabled
            && value.symbolic_limit
            && projected.singleton().is_none();
        projected.expression = self
            .spec
            .relations()
            .enabled
            .then(|| value.expression.clone())
            .flatten();
        projected
    }
    /// 从受控的一元语义事实建立初始值。矛盾与容量不足不会变成 Top/空成功。
    pub fn from_facts(
        self,
        facts: &[facts::UnaryPredicate],
    ) -> Result<AbstractValue, facts::FactError> {
        let mut lattice = facts::FactLattice::new(self.spec.fact_limit());
        for predicate in facts {
            lattice.insert(facts::Fact::Unary(facts::UnaryFact::new(
                facts::Symbol::THIS,
                predicate.clone(),
            )))?;
        }
        let initial = reduce::import(&AbstractValue::top(), &lattice)?;
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
        result.numeric.finite = result
            .numeric
            .finite
            .into_limited(self.spec.constant_capacity());
        if query::candidates(result.numeric(), self.capacity()).is_some_and(|s| s.is_empty()) {
            return Err(facts::FactError::Contradiction {
                subject: facts::Symbol::THIS,
            });
        }
        Ok(self.project(&result))
    }
    /// 临时传播已有约束，不改变原值及存储 join 的合同。
    pub fn reduce(self, value: &AbstractValue) -> Reduction {
        reduce::reduce(self, value.clone())
    }
    /// stored anchor 上的区间 widening；其他组件仍保留 next 的保证。
    pub(crate) fn widen(self, old: &AbstractValue, next: &AbstractValue) -> AbstractValue {
        let mut out = next.clone();
        if self.spec.profile() == Profile::Product {
            out.numeric.interval = old.numeric.interval.widen(&next.numeric.interval);
        }
        out.forget_identity();
        out.identity = old.identity.join(&next.identity);
        out.provenance = old.provenance.join(&next.provenance);
        out.symbolic_limit =
            (old.symbolic_limit || next.symbolic_limit) && out.singleton().is_none();
        if old.expression != next.expression {
            out.forget_expression();
        }
        out
    }
    /// Join numeric components without exchanging facts. Sources and identities
    /// remain independent, and expression identity must hold on both paths.
    pub fn join(&self, left: &AbstractValue, right: &AbstractValue) -> AbstractValue {
        let finite = left
            .numeric
            .finite
            .join(&right.numeric.finite, self.spec.constant_capacity());
        let expression = left
            .expression
            .as_ref()
            .filter(|expression| Some(*expression) == right.expression.as_ref())
            .cloned();
        let identity = left.identity.join(&right.identity);
        if self.spec.profile() == Profile::ConstantsOnly {
            let mut joined = Self::finite(finite);
            joined.provenance = left.provenance.join(&right.provenance);
            joined.identity = identity;
            joined.expression = expression;
            joined.symbolic_limit =
                (left.symbolic_limit || right.symbolic_limit) && joined.singleton().is_none();
            return joined;
        }
        let mut joined = AbstractValue {
            numeric: NumericValue {
                finite,
                bits: left.numeric.bits.join(&right.numeric.bits),
                interval: left.numeric.interval.join(&right.numeric.interval),
                congruence: left.numeric.congruence.join(&right.numeric.congruence),
                nonzero: left.numeric.nonzero && right.numeric.nonzero,
            },
            provenance: left.provenance.join(&right.provenance),
            identity,
            expression,
            symbolic_limit: left.symbolic_limit || right.symbolic_limit,
        };
        if joined.singleton().is_some() {
            joined.symbolic_limit = false;
        }
        joined
    }
    fn finite(finite: FiniteConstantSet) -> AbstractValue {
        let Some(values) = finite.as_values() else {
            return AbstractValue::top();
        };
        let numeric = NumericValue {
            bits: known_bits::KnownBits::from_values(values),
            interval: interval::Interval::from_values(values),
            congruence: congruence::Congruence::from_values(values),
            nonzero: !values.contains(&U256::ZERO),
            finite,
        };
        let mut value = AbstractValue::from_numeric(numeric);
        value.provenance = provenance::Provenance::constant();
        value
    }
    fn collect(&self, values: impl IntoIterator<Item = U256>) -> AbstractValue {
        Self::finite(
            FiniteConstantSet::collect_bounded(values, self.spec.constant_capacity())
                .expect("reachable finite values are nonempty"),
        )
    }
    fn finite_apply(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        let Some(sets) = args
            .iter()
            .map(AbstractValue::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                self.collect((0_u64..=256).map(U256::from))
            } else if matches!(op, 0x10..=0x15) {
                self.collect([U256::ZERO, U256::from(1)])
            } else {
                AbstractValue::top()
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
    pub fn apply(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        self.apply_detailed(op, args).value
    }
    /// 库调用方可使用和引擎相同的根账本；失败时不返回部分数值结果。
    pub fn apply_budgeted(
        self,
        op: u8,
        args: &[AbstractValue],
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<Reduction, WorkExhausted> {
        if !budget.charge(self.operation_work(args)) {
            return Err(WorkExhausted);
        }
        Ok(self.apply_detailed(op, args))
    }
    /// EVM 地址取低 160 位。原 word 的高位未知不妨碍投影后成为单点。
    pub fn address_projection(self, value: &AbstractValue) -> AbstractValue {
        let mut projected = self.apply(
            opcode::AND,
            &[value.clone(), AbstractValue::constant(U256::MAX >> 96usize)],
        );
        projected.provenance = projected.provenance.with_code_address_role();
        projected
    }
    /// 包含交换状态的查询入口，便于区分稳定和可选精度截断。
    pub fn apply_detailed(&self, op: u8, args: &[AbstractValue]) -> Reduction {
        let mut reduced = self.numeric_apply_detailed(op, args);
        reduced.value.symbolic_limit = args.iter().any(AbstractValue::symbolic_limit_reached)
            && reduced.value.singleton().is_none();
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if matches!(op,0x01..=0x0b|0x10..=0x1e) && args.len() == expected {
            reduced.value.provenance = provenance::Provenance::transfer(
                &args
                    .iter()
                    .map(|value| value.provenance.clone())
                    .collect::<Vec<_>>(),
            );
        }
        let relations = self.spec.relations();
        if relations.enabled
            && reduced.value.singleton().is_none()
            && let Some(expressions) = args
                .iter()
                .map(AbstractValue::symbolic_expression)
                .collect::<Option<Vec<_>>>()
        {
            match symbolic::ExprId::operation(op, &expressions, relations.expressions()) {
                Ok(expression) => {
                    if let Some(exact) = expression.as_constant() {
                        reduced.value = reduced.value.with_numeric(NumericValue::constant(exact));
                        reduced.value.forget_expression();
                    } else {
                        reduced.value = reduced.value.with_expression(expression);
                    }
                }
                Err(source) => {
                    if matches!(source, symbolic::ExprError::Limit { .. }) {
                        reduced.value.symbolic_limit = true;
                    }
                    if reduced.status == ReductionStatus::Stable {
                        reduced.status = ReductionStatus::SymbolicLimit;
                    }
                    reduced.symbolic_error = Some(source);
                }
            }
        }
        reduced
    }

    fn numeric_apply_detailed(&self, op: u8, args: &[AbstractValue]) -> Reduction {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if !matches!(op,0x01..=0x0b|0x10..=0x1e) || args.len() != expected {
            return Reduction::unchanged(AbstractValue::top());
        }
        // Trusted value identity belongs to AbstractValue, independently of
        // the numeric representation or the optional expression/path layer.
        if args.len() > 1 && args[0].identity.same_identity(&args[1].identity) {
            let exact = match op {
                opcode::EQ => Some(U256::from(1)),
                opcode::XOR | opcode::SUB | opcode::LT | opcode::GT | opcode::SLT | opcode::SGT => {
                    Some(U256::ZERO)
                }
                _ => None,
            };
            if let Some(exact) = exact {
                return Reduction::unchanged(AbstractValue::constant(exact));
            }
        }
        if self.spec.profile() == Profile::ConstantsOnly {
            return Reduction::unchanged(self.finite_apply(op, args));
        }
        transfer::apply(*self, op, args)
    }
    /// 纯数值运算及其有界 fact 交换的逻辑工作上界。
    /// 费用以 limb/位规则为单位，与策略版本一起冻结。
    pub fn operation_work(self, args: &[AbstractValue]) -> usize {
        let symbolic = if self.spec.relations().enabled {
            args.iter()
                .fold(1usize, |cost, value| {
                    cost.saturating_add(value.expression().map_or(1, symbolic::ExprId::nodes))
                })
                .saturating_mul(256)
        } else {
            0
        };
        self.numeric_operation_work(args).saturating_add(symbolic)
    }

    /// Bound scalar guard import and reduction independently of the selected
    /// numeric profile. Constants-only projection may still enumerate candidates.
    pub(crate) fn refinement_work(self, value: &AbstractValue, facts: usize) -> usize {
        let atoms = self.capacity().saturating_add(32);
        let round = 4096usize
            .saturating_add(256usize.saturating_mul(self.capacity()))
            .saturating_add(32usize.saturating_mul(atoms));
        let facts = facts
            .saturating_add(1)
            .min(self.spec.fact_limit().saturating_add(1));
        4096usize
            .saturating_add(round.saturating_mul(self.spec.reduction_rounds()))
            .saturating_add(
                facts
                    .saturating_mul(facts.saturating_add(atoms))
                    .saturating_mul(32),
            )
            .saturating_add(value.work_size())
    }

    fn numeric_operation_work(self, args: &[AbstractValue]) -> usize {
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
                .saturating_add(args.iter().map(AbstractValue::work_size).sum::<usize>());
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
            .saturating_add(args.iter().map(AbstractValue::work_size).sum::<usize>())
    }
}
