//! 根执行、数值域、状态操作和摘要共同使用的累计工作账本。
//! 调用者先预留可信上界，再执行整项操作；失败时不能安装半个结果。

/// 单次分析的累计逻辑工作预算，不等同于 EVM gas。
#[derive(Debug)]
pub struct WorkBudget {
    maximum: usize,
    consumed: usize,
    exhausted: bool,
    cancellation: embedded_smt::Cancellation,
    smt_cache: embedded_smt::DecidedCache,
}
impl WorkBudget {
    /// 不预分配；零额度允许用于验证中断行为。
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum,
            consumed: 0,
            exhausted: false,
            cancellation: embedded_smt::current_cancellation(),
            smt_cache: embedded_smt::DecidedCache::default(),
        }
    }
    /// 事务式预留。超限保留此前消费量，并记录中断。
    pub fn charge(&mut self, amount: usize) -> bool {
        if self.cancellation.is_cancelled() {
            self.exhausted = true;
            return false;
        }
        if amount > self.maximum.saturating_sub(self.consumed) {
            self.exhausted = true;
            return false;
        }
        self.consumed += amount;
        true
    }
    /// Borrow the same root ledger for a sequence of individually paid queries.
    /// The captured cancellation token and charge-before-query rule survive;
    /// dropping the lease never refunds a paid query or copies its counters.
    #[cfg(test)]
    pub(crate) fn query_budget(&mut self, amount: usize) -> embedded_smt::QueryBudget<'_> {
        embedded_smt::QueryBudget::new(
            self.maximum,
            &mut self.consumed,
            &mut self.exhausted,
            self.cancellation.clone(),
            amount,
        )
    }
    /// One explicit analysis-owned cache, independent of transfer snapshots.
    pub(crate) fn smt_cache_query(
        &mut self,
        scope: embedded_smt::CacheScope,
    ) -> embedded_smt::CacheQuery<'_> {
        embedded_smt::CacheQuery::new(
            self.maximum,
            &mut self.consumed,
            self.cancellation.clone(),
            &mut self.smt_cache,
            scope,
        )
    }
    /// Preserve each original fee while borrowing disjoint owner fields.
    pub(crate) fn cached_query_budget(
        &mut self,
        amount: usize,
        scope: embedded_smt::CacheScope,
    ) -> embedded_smt::QueryBudget<'_> {
        embedded_smt::QueryBudget::new(
            self.maximum,
            &mut self.consumed,
            &mut self.exhausted,
            self.cancellation.clone(),
            amount,
        )
        .with_cache(&mut self.smt_cache, scope)
    }
    /// Bounded counters for this analysis's optional SMT-answer cache.
    pub fn smt_cache_stats(&self) -> embedded_smt::CacheStats {
        self.smt_cache.stats()
    }
    /// 是否有操作因额度不足被拒绝。
    pub fn exhausted(&self) -> bool {
        self.exhausted || self.cancellation.is_cancelled()
    }
    /// 已成功预留的工作量。
    pub fn used(&self) -> usize {
        self.consumed
    }
}
