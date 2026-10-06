//! 配置准入：把用户可编辑的参数转换成执行引擎可以信任的输入。
//!
//! `Config` 的 usize 字段适合 CLI/JSON 和实验配置，但它们还不是证明。
//! `validate` 在这里检查所有条件，并把域容量转换成 NonZeroUsize。
//! 执行引擎只接收私有字段的 `ValidatedConfig`，不再依靠运行中的 expect。

use crate::domain::{Domain, DomainSpec, Profile};
use serde::Serialize;
use std::num::NonZeroUsize;
use thiserror::Error;

/// 有限分析的原始参数；进入引擎前通过 [`Config::validate`] 统一验证。
#[derive(Clone, Debug, Serialize)]
pub struct Config {
    /// 默认组合域；constants-only 保留有限集合对照。
    pub domain_profile: Profile,
    /// 一次临时 fact 交换最多执行的完整轮数。
    pub reduction_rounds: usize,
    /// 一次交换的语义事实原子上限；不按上限预分配。
    pub max_facts: usize,
    /// 每个槽位最多保留的常量数；任意正 usize，默认 8，不按上限预分配。
    pub max_constants: usize,
    /// 保留最近 k 个跳转来源块；0 代表上下文不敏感，默认 8。
    /// 接受任意 usize，不按 k 预分配；实际历史增长受执行资源预算约束。
    pub context_depth: usize,
    /// 可创建的状态数上限；限制触发后结果是 Incomplete。
    pub max_states: usize,
    /// 基本块 transfer 次数上限；一次重新执行也计数。
    pub max_transfers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            domain_profile: Profile::Product,
            reduction_rounds: 4,
            max_facts: 256,
            max_constants: 8,
            context_depth: 8,
            max_states: 4096,
            max_transfers: 100_000,
        }
    }
}

/// 配置不成立，分析尚未开始。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 有限常量集合的容量必须非零。
    #[error("max_constants must be positive")]
    Constants,
    /// 资源预算不能是零。
    #[error("max_states and max_transfers must be positive")]
    Budget,
    /// 交换精度策略必须允许至少一个原子和一轮传播。
    #[error("reduction_rounds and max_facts must be positive")]
    Facts,
}

/// 已完成准入验证的配置，携带由同一份参数构建的非零容量域。
///
/// 字段私有，没有绕过验证的构造器；引擎与外部调用者都不能直接伪造它。
/// ```compile_fail
/// use evm_abstract::analysis::{Config, ValidatedConfig};
/// use evm_abstract::domain::Domain;
/// let raw = Config { max_constants: 0, ..Config::default() };
/// let unchecked = ValidatedConfig { config: raw, domain: Domain::default() };
/// ```
#[derive(Clone, Debug)]
pub struct ValidatedConfig {
    config: Config,
    domain: Domain,
}

impl Config {
    /// 消耗原始配置，检查所有条件后才允许构建引擎输入。
    ///
    /// 非零转换在准入处完成；实际集合增长受执行工作预算约束。
    /// ```
    /// use evm_abstract::analysis::Config;
    /// let validated = Config::default().validate()?;
    /// assert_eq!(validated.config().max_constants, 8);
    /// # Ok::<(), evm_abstract::analysis::ConfigError>(())
    /// ```
    pub fn validate(self) -> Result<ValidatedConfig, ConfigError> {
        let capacity = NonZeroUsize::new(self.max_constants).ok_or(ConfigError::Constants)?;
        if self.max_states == 0 || self.max_transfers == 0 {
            return Err(ConfigError::Budget);
        }
        let rounds = NonZeroUsize::new(self.reduction_rounds).ok_or(ConfigError::Facts)?;
        let facts = NonZeroUsize::new(self.max_facts).ok_or(ConfigError::Facts)?;
        Ok(ValidatedConfig {
            domain: Domain::from_spec(DomainSpec::new(
                self.domain_profile,
                capacity,
                rounds,
                facts,
            )),
            config: self,
        })
    }
}

impl ValidatedConfig {
    /// 只读参数，供调用者查看准入结果；不能原地修改已验证的配置。
    pub fn config(&self) -> &Config {
        &self.config
    }

    // 只有 analysis 模块能取出运行需要的部分；其他模块不能拆开再重新组装。
    pub(super) fn into_parts(self) -> (Config, Domain) {
        (self.config, self.domain)
    }
}
