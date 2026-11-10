//! Explicit semantic premises shared by projection producers and validators.
//!
//! Resource facts belong to a declared execution domain. They do not follow
//! from the host's integer width, a test memory ceiling, or a Solidity allocator.
//! The complete derivation and precompile audit are recorded in
//! `docs/decompile-execution-domains.md`.

use crate::{Fork, U256};
use serde::{Deserialize, Deserializer, Serialize};

/// Executions for which source-certified semantic projections are claimed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionDomain {
    /// Generic EVM primitive hosts, without a transaction resource premise.
    #[default]
    Generic,
    /// Transactions included in valid Ethereum blocks, including their child
    /// frames, under the explicitly selected supported Ethereum fork.
    ///
    /// This declaration excludes unconstrained simulations, system calls and
    /// custom-chain rules. A host used to validate it must honor that domain.
    EthereumTransactions,
}

// The valid Ethereum header ceiling is a consensus premise, independent of the
// Rust type used below to evaluate its small, exact mathematical consequences.
const ETHEREUM_TRANSACTION_GAS_CEILING: u128 = (1_u128 << 63) - 1;

const fn memory_gas(words: u64) -> u128 {
    let words = words as u128;
    3 * words + words * words / 512
}

// Find a conservative power-of-two byte ceiling. The first unaffordable
// power-of-two word count is 2^36, ruling out 2^41 memory bytes in a frame.
const fn memory_byte_ceiling() -> u64 {
    let mut words = 1_u64;
    while memory_gas(words) <= ETHEREUM_TRANSACTION_GAS_CEILING {
        words *= 2;
    }
    words * 32
}

const ETHEREUM_RETURN_DATA_CEILING: u64 = memory_byte_ceiling();

impl ExecutionDomain {
    /// Whether no transaction-specific resource premise has been declared.
    pub const fn is_generic(&self) -> bool {
        matches!(self, Self::Generic)
    }

    /// Conservative inclusive RETURNDATASIZE bound under this declared domain.
    ///
    /// For all supported Ethereum forks, successful bytecode RETURN/REVERT and
    /// IDENTITY output are bounded by charged frame memory. MODEXP is bounded
    /// by its output-length gas cost (and Osaka's explicit length limit); other
    /// active precompiles return at most 256 bytes. Thus every return buffer
    /// visible to an EVM caller is strictly smaller than 2^41 bytes.
    ///
    /// The exhaustive fork match intentionally requires a fresh precompile and
    /// gas-rule audit before a newly supported fork inherits this premise.
    pub const fn return_data_size_bound(self, fork: Fork) -> Option<U256> {
        match self {
            Self::Generic => None,
            Self::EthereumTransactions => {
                match fork {
                    Fork::Cancun | Fork::Prague | Fork::Osaka => {}
                }
                Some(U256::from_limbs([
                    ETHEREUM_RETURN_DATA_CEILING - 1,
                    0,
                    0,
                    0,
                ]))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ExecutionDomain {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        match input.as_str() {
            "generic" => Ok(Self::Generic),
            "ethereum_transactions" => Ok(Self::EthereumTransactions),
            _ => Err(serde::de::Error::unknown_variant(
                &input,
                &["generic", "ethereum_transactions"],
            )),
        }
    }
}

#[cfg(test)]
mod tests;
