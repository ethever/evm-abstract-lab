//! 一次分析冻结的共享数值与关系上限，不代表自定义代数或规则的语义身份。
//! 摘要复用由引擎、会话及来源的冻结契约约束。
use serde::Serialize;
use std::num::NonZeroUsize;

/// Invalid limits in a frozen numeric/shared policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DomainSpecError {
    /// Relation bounds remain mandatory even when relational queries are disabled.
    #[error("relation constraint count, node count, and depth must all be nonzero")]
    InvalidRelationLimits,
}

/// 数值语义和有界交换策略。私有字段确保运行中不切换策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DomainSpec {
    schema_version: u16,
    word_bits: u16,
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
        capacity: NonZeroUsize,
        reduction_rounds: NonZeroUsize,
        fact_limit: NonZeroUsize,
    ) -> Self {
        Self {
            schema_version: 3,
            word_bits: 256,
            capacity,
            reduction_rounds,
            fact_limit,
            // Version 7 uses native borrowed operations and explicit opcode quotes.
            cost_version: 7,
            widening_after_updates: 2,
            provenance_policy: "scoped-expressions-and-value-identities-v3",
            relations: super::relational::RelationLimits::default(),
        }
    }
    /// 冻结的域策略序列化结构版本。
    pub fn schema_version(self) -> u16 {
        self.schema_version
    }
    /// 数值域中一个字的位宽。
    pub fn word_bits(self) -> u16 {
        self.word_bits
    }
    /// 冻结的累计工作收费策略版本。
    pub fn cost_version(self) -> u16 {
        self.cost_version
    }
    /// 表达式与值身份使用的来源追踪策略标识。
    pub fn provenance_policy(self) -> &'static str {
        self.provenance_policy
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
    /// Validate shared limits, including the bounds of disabled relations.
    pub fn validate(&self) -> Result<(), DomainSpecError> {
        if !self.relations.valid() {
            return Err(DomainSpecError::InvalidRelationLimits);
        }
        Ok(())
    }
    /// 已有节点发生多少次严格增强后扩大不断移动的区间端点。
    pub fn widening_after_updates(self) -> usize {
        self.widening_after_updates
    }
    /// 完整有限候选集合的最大容量。
    pub fn capacity(self) -> usize {
        self.capacity.get()
    }
    /// 向组件传递已验证的容量，每个值无需重复保存容量策略。
    pub(crate) fn constant_capacity(self) -> NonZeroUsize {
        self.capacity
    }
    /// 局部交换的调度额度参数，不声称理论最精确闭包。
    /// 默认协作 product 按饱和运算换算为最多 `1 + 3 * R` 次任务评估，
    /// 包含输入初始化；它不是所有组件同步读取旧快照的轮数。
    pub fn reduction_rounds(self) -> usize {
        self.reduction_rounds.get()
    }
    /// 每次交换最多容纳的语义事实原子数。
    pub fn fact_limit(self) -> usize {
        self.fact_limit.get()
    }
}

impl Default for DomainSpec {
    fn default() -> Self {
        Self::new(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
    }
}

#[cfg(test)]
mod tests;
