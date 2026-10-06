//! 根执行、数值域、状态操作和摘要共同使用的累计工作账本。
//! 调用者先预留可信上界，再执行整项操作；失败时不能安装半个结果。

/// 单次分析的累计逻辑工作预算，不等同于 EVM gas。
#[derive(Debug)]
pub struct WorkBudget {
    maximum: usize,
    consumed: usize,
    exhausted: bool,
}
impl WorkBudget {
    /// 不预分配；零额度允许用于验证中断行为。
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum,
            consumed: 0,
            exhausted: false,
        }
    }
    /// 事务式预留。超限保留此前消费量，并记录中断。
    pub fn charge(&mut self, amount: usize) -> bool {
        if amount > self.maximum.saturating_sub(self.consumed) {
            self.exhausted = true;
            return false;
        }
        self.consumed += amount;
        true
    }
    /// 是否有操作因额度不足被拒绝。
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }
    /// 已成功预留的工作量。
    pub fn used(&self) -> usize {
        self.consumed
    }
}
