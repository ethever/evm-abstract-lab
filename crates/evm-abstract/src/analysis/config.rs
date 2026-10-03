//! 配置准入：把用户可编辑的参数转换成执行引擎可以信任的输入。
//!
//! `Config` 的 usize 字段适合 CLI/JSON 和实验配置，但它们还不是证明。
//! `validate` 在这里检查所有条件，并把域容量转换成 NonZeroUsize。
//! 执行引擎只接收私有字段的 `ValidatedConfig`，不再依靠运行中的 expect。

use crate::domain::Domain;
use serde::Serialize;
use std::num::NonZeroUsize;
use thiserror::Error;

/// 有限分析的原始参数；进入引擎前通过 [`Config::validate`] 统一验证。
#[derive(Clone, Debug, Serialize)]
pub struct Config {
    /// 每个槽位最多保留的常量数，范围 1..=64。
    pub max_constants: usize,
    /// 保留最近 k 个跳转来源块，范围 0..=3；0 代表上下文不敏感。
    pub context_depth: usize,
    /// 可创建的状态数上限；限制触发后结果是 Incomplete。
    pub max_states: usize,
    /// 基本块 transfer 次数上限；一次重新执行也计数。
    pub max_transfers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_constants: 8,
            context_depth: 0,
            max_states: 4096,
            max_transfers: 100_000,
        }
    }
}

/// 配置不成立，分析尚未开始。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 域容量超出教学实现允许的范围。
    #[error("max_constants must be in 1..=64")]
    Constants,
    /// 上下文增长过快，教学实现限制到三层。
    #[error("context_depth must be in 0..=3")]
    Context,
    /// 资源预算不能是零。
    #[error("max_states and max_transfers must be positive")]
    Budget,
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
    /// 非零转换与范围校验在同一处完成，执行阶段无需重复解析 usize。
    /// ```
    /// use evm_abstract::analysis::Config;
    /// let validated = Config::default().validate()?;
    /// assert_eq!(validated.config().max_constants, 8);
    /// # Ok::<(), evm_abstract::analysis::ConfigError>(())
    /// ```
    pub fn validate(self) -> Result<ValidatedConfig, ConfigError> {
        let capacity = NonZeroUsize::new(self.max_constants)
            .filter(|capacity| capacity.get() <= 64)
            .ok_or(ConfigError::Constants)?;
        if self.context_depth > 3 {
            return Err(ConfigError::Context);
        }
        if self.max_states == 0 || self.max_transfers == 0 {
            return Err(ConfigError::Budget);
        }
        Ok(ValidatedConfig {
            domain: Domain::new(capacity),
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
