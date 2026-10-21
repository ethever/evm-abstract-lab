//! Execution limits, independent of the selected numeric algebra.

use serde::Serialize;
use thiserror::Error;

/// Finite control-context and global graph/transfer budgets.
#[derive(Clone, Debug, Serialize)]
pub struct Config {
    /// Maximum retained intraprocedural jump history.
    pub context_depth: usize,
    /// Maximum cumulative state allocations.
    pub max_states: usize,
    /// Maximum cumulative basic-block transfers.
    pub max_transfers: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            context_depth: 128,
            max_states: 4096,
            max_transfers: 100_000,
        }
    }
}

/// Invalid immutable execution inputs, before running the machine.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Invalid shared semantic bounds on the actual selected algebra.
    #[error("{0}")]
    DomainSpec(#[from] crate::domain::DomainSpecError),
    /// Transaction or block observations contradict an input invariant.
    #[error("{0}")]
    Environment(#[from] crate::world::environment::EnvironmentError),
    /// A required execution budget is zero.
    #[error("state, transfer, work, call depth and materialization budgets must be positive")]
    Budget,
}

/// Admitted execution limits. Numeric policies belong to ValueAlgebra::spec().
#[derive(Clone, Debug)]
pub struct ValidatedConfig {
    config: Config,
}
impl Config {
    /// Admit control-state and transfer budgets without selecting a numeric model.
    pub fn validate(self) -> Result<ValidatedConfig, ConfigError> {
        self.check()?;
        Ok(ValidatedConfig { config: self })
    }
    pub(super) fn check(&self) -> Result<(), ConfigError> {
        if self.max_states == 0 || self.max_transfers == 0 {
            Err(ConfigError::Budget)
        } else {
            Ok(())
        }
    }
}
impl ValidatedConfig {
    /// Immutable admitted limits.
    pub fn config(&self) -> &Config {
        &self.config
    }
}
