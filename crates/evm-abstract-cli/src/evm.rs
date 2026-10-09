//! One typed EVM environment shared by every command that executes bytecode.
//!
//! Missing call and block facts remain symbolic. Explicit zero quantities and
//! empty calldata are observations, so they stay distinct from omitted flags.

use crate::{number, world};
use alloy_primitives::{Address, B256, U256};
use clap::Args;
use evm_abstract::{
    domain::AbstractValue,
    world::{
        AddressInput, BlobHashes, ByteArray, EvmEnvironment, GasInput,
        environment::EnvironmentError,
    },
};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Args, Default)]
#[group(skip)]
pub(crate) struct EvmArgs {
    /// Executed account; required for world/RPC input, symbolic for raw bytecode.
    #[arg(id = "evm.to", long = "evm.to", value_parser = address)]
    pub to: Option<Address>,
    /// External sender; symbolic when omitted.
    #[arg(id = "evm.caller", long = "evm.caller", value_parser = address)]
    pub caller: Option<Address>,
    /// Transaction origin; aliases caller when omitted.
    #[arg(id = "evm.origin", long = "evm.origin", value_parser = address)]
    pub origin: Option<Address>,
    /// Entry CALLVALUE in wei; symbolic when omitted.
    #[arg(id = "evm.value", long = "evm.value", value_parser = number::parse)]
    pub value: Option<U256>,
    /// Observed calldata bytes; omit for symbolic bytes and length, use 0x for empty.
    #[arg(id = "evm.calldata", long = "evm.calldata", value_parser = calldata)]
    pub calldata: Option<ByteArray>,
    /// Apply STATICCALL write restrictions to the entry and descendants.
    #[arg(id = "evm.static", long = "evm.static")]
    pub is_static: bool,
    /// Entry gas upper bound; subsequent gas accounting remains conservative.
    #[arg(id = "evm.gas", long = "evm.gas", value_parser = number::parse)]
    pub gas: Option<U256>,
    /// Transaction GASPRICE in wei.
    #[arg(id = "evm.gas-price", long = "evm.gas-price", value_parser = number::parse)]
    pub gas_price: Option<U256>,
    /// Block beneficiary address.
    #[arg(id = "evm.coinbase", long = "evm.coinbase", value_parser = address)]
    pub coinbase: Option<Address>,
    /// Block TIMESTAMP override in seconds; RPC defaults to the pinned header.
    #[arg(id = "evm.timestamp", long = "evm.timestamp", value_parser = number::parse)]
    pub timestamp: Option<U256>,
    /// EVM NUMBER override; RPC defaults to the pinned header without changing the selector.
    #[arg(id = "evm.number", long = "evm.number", value_parser = number::parse)]
    pub number: Option<U256>,
    /// Block PREVRANDAO, supplied as a 256-bit quantity.
    #[arg(id = "evm.prevrandao", long = "evm.prevrandao", value_parser = number::parse)]
    pub prevrandao: Option<U256>,
    /// Block GASLIMIT.
    #[arg(id = "evm.gas-limit", long = "evm.gas-limit", value_parser = number::parse)]
    pub gas_limit: Option<U256>,
    /// EVM CHAINID override; independent of RPC acquisition identity.
    #[arg(id = "evm.chain-id", long = "evm.chain-id", value_parser = number::parse)]
    pub chain_id: Option<U256>,
    /// Block BASEFEE in wei.
    #[arg(id = "evm.basefee", long = "evm.basefee", value_parser = number::parse)]
    pub basefee: Option<U256>,
    /// Block BLOBBASEFEE in wei.
    #[arg(id = "evm.blob-basefee", long = "evm.blob-basefee", value_parser = number::parse)]
    pub blob_basefee: Option<U256>,
    /// Observed BLOCKHASH entry NUMBER:HASH (repeatable; 256-bit number and 32-byte hash).
    #[arg(id = "evm.block-hash", long = "evm.block-hash", value_parser = indexed_hash)]
    pub block_hash: Vec<IndexedHash>,
    /// Observed BLOBHASH entry INDEX:HASH (repeatable; 256-bit index and 32-byte hash).
    #[arg(id = "evm.blob-hash", long = "evm.blob-hash", value_parser = indexed_hash)]
    pub blob_hash: Vec<IndexedHash>,
    /// Exact blob count; omit for unknown length, use zero for an empty blob list.
    #[arg(id = "evm.blob-count", long = "evm.blob-count", value_parser = number::parse)]
    pub blob_count: Option<U256>,
}

#[derive(Clone)]
pub(crate) struct IndexedHash {
    index: U256,
    hash: B256,
}

#[derive(Debug, Error)]
pub(crate) enum EnvironmentInputError {
    #[error("{0}")]
    Invalid(EnvironmentError),
    #[error("conflicting --{flag} observations at index {index:#x}")]
    ConflictingHash { flag: &'static str, index: U256 },
    #[error("--evm.blob-hash index {index:#x} is outside --evm.blob-count {count:#x}")]
    BlobIndex { index: U256, count: U256 },
}

impl From<EnvironmentError> for EnvironmentInputError {
    fn from(error: EnvironmentError) -> Self {
        Self::Invalid(error)
    }
}

fn address(input: &str) -> Result<Address, String> {
    world::address(input, "EVM environment").map_err(|error| error.to_string())
}

fn calldata(input: &str) -> Result<ByteArray, String> {
    world::calldata(input)
        .map(|bytes| ByteArray::exact(&bytes))
        .map_err(|error| error.to_string())
}

fn indexed_hash(input: &str) -> Result<IndexedHash, String> {
    let (index, hash) = input
        .split_once(':')
        .ok_or_else(|| "expected INDEX:HASH".to_owned())?;
    Ok(IndexedHash {
        index: number::parse(index).map_err(|error| format!("invalid hash index: {error}"))?,
        hash: world::hash(hash, "EVM environment").map_err(|error| error.to_string())?,
    })
}

fn indexed(
    values: &[IndexedHash],
    flag: &'static str,
) -> Result<BTreeMap<U256, B256>, EnvironmentInputError> {
    let mut observations = BTreeMap::new();
    for value in values {
        if observations
            .insert(value.index, value.hash)
            .is_some_and(|previous| previous != value.hash)
        {
            return Err(EnvironmentInputError::ConflictingHash {
                flag,
                index: value.index,
            });
        }
    }
    Ok(observations)
}

impl EvmArgs {
    pub(crate) fn environment(self) -> Result<EvmEnvironment, EnvironmentInputError> {
        if let Some(count) = self.blob_count
            && let Some(entry) = self.blob_hash.iter().find(|entry| entry.index >= count)
        {
            return Err(EnvironmentInputError::BlobIndex {
                index: entry.index,
                count,
            });
        }
        let environment = EvmEnvironment {
            input_scope: Default::default(),
            to: self.to.map_or_else(AddressInput::unknown_to, Into::into),
            caller: self
                .caller
                .map_or_else(AddressInput::unknown_caller, Into::into),
            origin: self.origin.map(Into::into),
            value: quantity(self.value),
            calldata: self.calldata.unwrap_or_else(ByteArray::unknown),
            is_static: self.is_static,
            gas_price: quantity(self.gas_price),
            coinbase: self
                .coinbase
                .map_or_else(AddressInput::unknown_coinbase, Into::into),
            timestamp: quantity(self.timestamp),
            number: quantity(self.number),
            prevrandao: quantity(self.prevrandao),
            gas_limit: quantity(self.gas_limit),
            chain_id: self.chain_id.map(AbstractValue::constant),
            base_fee: quantity(self.basefee),
            blob_base_fee: quantity(self.blob_basefee),
            gas: self.gas.map_or(GasInput::Unknown, GasInput::UpperBound),
            block_hashes: indexed(&self.block_hash, "evm.block-hash")?,
            blob_hashes: BlobHashes {
                length: quantity(self.blob_count),
                hashes: indexed(&self.blob_hash, "evm.blob-hash")?,
            },
        };
        environment.validate()?;
        Ok(environment)
    }
}

fn quantity(input: Option<U256>) -> AbstractValue {
    input.map_or_else(AbstractValue::top, AbstractValue::constant)
}
