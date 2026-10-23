//! 根执行、数值域、状态操作和摘要共同使用的累计工作账本。
//! 调用者先预留可信上界，再执行整项操作；失败时不能安装半个结果。

mod allowance;
pub(crate) mod clone_payment;
mod completion;
mod summary;

pub(crate) use clone_payment::CloneError;
pub(crate) use completion::{AdmissionStop, CompletionPermit};
pub(crate) use summary::SummaryError;

/// 单次分析的累计逻辑工作预算，不等同于 EVM gas。
#[derive(Debug)]
pub struct WorkBudget {
    maximum: usize,
    consumed: usize,
    exhausted: bool,
    cancellation: analysis_control::Cancellation,
}
impl WorkBudget {
    /// 不预分配；零额度允许用于验证中断行为。
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum,
            consumed: 0,
            exhausted: false,
            cancellation: analysis_control::current_cancellation(),
        }
    }
    /// 保留创建账本时的取消身份，不读取调用者当前所在的取消作用域。
    pub(crate) fn cancellation(&self) -> analysis_control::Cancellation {
        self.cancellation.clone()
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
    /// 只读检查可选精确操作能否完整执行；拒绝时允许保留已有的可靠近似。
    /// 不预留额度，也不清除之前的耗尽或取消状态。
    pub fn can_afford(&self, amount: usize) -> bool {
        !self.exhausted() && amount <= self.maximum.saturating_sub(self.consumed)
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

#[cfg(test)]
mod tests;
