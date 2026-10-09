//! Execution-visible observations obtained from one pinned RPC block header.

use crate::{
    Address, U256,
    domain::AbstractValue,
    world::{AddressInput, EvmEnvironment},
};
use alloy_primitives::B256;
use serde::Serialize;

/// Concrete block observations. Optional fee fields are unavailable when the
/// endpoint does not report the corresponding chain feature; absence is never
/// interpreted as a zero fee. Transaction-specific inputs remain independent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotEnvironment {
    /// Pinned block height.
    pub number: U256,
    /// Parent block hash, also usable for BLOCKHASH(number - 1).
    pub parent_hash: B256,
    /// Block timestamp in seconds.
    pub timestamp: U256,
    /// Header miner/fee-recipient address.
    pub coinbase: Address,
    /// Post-merge randomness from the header's mixHash.
    pub prevrandao: B256,
    /// Block gas limit, not the transaction's remaining gas.
    pub gas_limit: U256,
    /// Header baseFeePerGas; absent means not reported, not a proven zero.
    pub base_fee: Option<U256>,
    /// Fee-history observation for this exact block, accounting for the chain's
    /// blob schedule including BPO parameter changes within the Osaka VM fork.
    pub blob_base_fee: Option<U256>,
    /// Header excessBlobGas, if this chain reports blob accounting.
    pub excess_blob_gas: Option<U256>,
    /// Header blobGasUsed, if this chain reports blob accounting.
    pub blob_gas_used: Option<U256>,
}

impl SnapshotEnvironment {
    pub(crate) fn apply(&self, environment: &mut EvmEnvironment) {
        fn missing(value: &mut AbstractValue, observed: U256) {
            // Preserve explicitly supplied constants, intervals, provenance and
            // symbolic expressions. Only the unqualified default is filled.
            if *value == AbstractValue::top() {
                *value = AbstractValue::constant(observed);
            }
        }
        missing(&mut environment.number, self.number);
        missing(&mut environment.timestamp, self.timestamp);
        missing(
            &mut environment.prevrandao,
            U256::from_be_slice(self.prevrandao.as_slice()),
        );
        missing(&mut environment.gas_limit, self.gas_limit);
        if environment.coinbase == AddressInput::unknown_coinbase() {
            environment.coinbase = self.coinbase.into();
        }
        if let Some(value) = self.base_fee {
            missing(&mut environment.base_fee, value);
        }
        if let Some(value) = self.blob_base_fee {
            missing(&mut environment.blob_base_fee, value);
        }
        if self.number != U256::ZERO && environment.block_hashes.len() < 256 {
            environment
                .block_hashes
                .entry(self.number - U256::from(1))
                .or_insert(self.parent_hash);
        }
    }
}
