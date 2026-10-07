//! 一次分析冻结的域策略；摘要兼容性比较整个策略，而不只比较常量容量。
use serde::Serialize;
use std::num::NonZeroUsize;

/// 可复现的数值分析方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// 常量集合、位、U/S 区间和一般同余共同约束一个字。
    #[default]
    Product,
    /// 保留原有限常量域，供精度和性能对照。
    ConstantsOnly,
}

/// 数值语义和有界交换策略。私有字段确保运行中不切换策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DomainSpec {
    schema_version: u16,
    word_bits: u16,
    profile: Profile,
    capacity: NonZeroUsize,
    reduction_rounds: NonZeroUsize,
    fact_limit: NonZeroUsize,
    cost_version: u16,
    widening_after_updates: usize,
    provenance_policy: &'static str,
    relations: super::relational::RelationLimits,
}

impl DomainSpec {
    /// 策略参数均已由类型证明非零；不按参数预分配。
    pub fn new(
        profile: Profile,
        capacity: NonZeroUsize,
        reduction_rounds: NonZeroUsize,
        fact_limit: NonZeroUsize,
    ) -> Self {
        Self {
            schema_version: 2,
            word_bits: 256,
            profile,
            capacity,
            reduction_rounds,
            fact_limit,
            cost_version: 2,
            widening_after_updates: 2,
            provenance_policy: "scoped-expressions-and-value-identities-v3",
            relations: super::relational::RelationLimits::default(),
        }
    }
    /// Freeze the relation limits alongside the numeric component policy.
    pub fn with_relations(mut self, relations: super::relational::RelationLimits) -> Self {
        self.relations = relations;
        self
    }
    /// Relation policy shared by execution, symbolic queries and summaries.
    pub fn relations(self) -> super::relational::RelationLimits {
        self.relations
    }
    /// 已有节点发生多少次严格增强后扩大不断移动的区间端点。
    pub fn widening_after_updates(self) -> usize {
        self.widening_after_updates
    }
    /// 当前运行方式。
    pub fn profile(self) -> Profile {
        self.profile
    }
    /// 完整有限候选集合的最大容量。
    pub fn capacity(self) -> usize {
        self.capacity.get()
    }
    /// 向组件传递已验证的容量，每个值无需重复保存容量策略。
    pub(crate) fn constant_capacity(self) -> NonZeroUsize {
        self.capacity
    }
    /// 一次临时交换最多执行的完整轮数；不声称理论最精确闭包。
    pub fn reduction_rounds(self) -> usize {
        self.reduction_rounds.get()
    }
    /// 每次交换最多容纳的语义事实原子数。
    pub fn fact_limit(self) -> usize {
        self.fact_limit.get()
    }
}
