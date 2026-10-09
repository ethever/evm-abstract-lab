//! Immutable root, transaction and block inputs for EVM execution.
//!
//! Omitted inputs describe all admissible values instead of a fabricated
//! transaction. Address identities remain distinct from account-state owners.
//! Indexed observations are partial: a missing valid entry remains unknown.

use super::{ByteArray, SnapshotIdentity};
pub use crate::domain::provenance::Symbol;
use crate::domain::{AbstractValue, Domain};
use alloy_primitives::{Address, B256, U256};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

static NEXT_INPUT_SCOPE: AtomicU64 = AtomicU64::new(1);

/// Opaque namespace for immutable symbolic inputs.
///
/// A fresh environment owns a fresh namespace; cloning preserves the same inputs.
/// IDs are never reused. Exhaustion disables optional identity precision rather
/// than allowing unrelated environments to share a symbol. Serialized reports
/// keep readable names and omit this process-local namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputScope(Option<u64>);

impl Default for InputScope {
    fn default() -> Self {
        Self(
            NEXT_INPUT_SCOPE
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .ok(),
        )
    }
}

impl InputScope {
    pub(crate) fn id(self) -> Option<u64> {
        self.0
    }
}

/// A concrete EVM address or one stable symbolic input address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum AddressInput {
    /// Exact twenty-byte address supplied or derived by execution.
    Concrete(Address),
    /// Unknown address with a persistent identity in this environment.
    Symbolic(Symbol),
}

impl AddressInput {
    /// Unspecified root destination for bytecode-only analysis.
    pub fn unknown_to() -> Self {
        Self::Symbolic(Symbol::To)
    }
    /// Unspecified root caller.
    pub fn unknown_caller() -> Self {
        Self::Symbolic(Symbol::Caller)
    }
    /// An origin independent of the root caller.
    pub fn unknown_origin() -> Self {
        Self::Symbolic(Symbol::Origin)
    }
    /// Unspecified block beneficiary.
    pub fn unknown_coinbase() -> Self {
        Self::Symbolic(Symbol::Coinbase)
    }
    /// Exact address when this input is concrete.
    pub fn as_concrete(self) -> Option<Address> {
        match self {
            Self::Concrete(address) => Some(address),
            Self::Symbolic(_) => None,
        }
    }
    /// Numeric representation with the high 96 bits guaranteed zero.
    pub fn value(self) -> AbstractValue {
        match self {
            Self::Concrete(address) => {
                AbstractValue::constant(U256::from_be_slice(address.as_slice()))
            }
            Self::Symbolic(_) => AbstractValue::unknown_address(),
        }
    }
    pub(crate) fn scoped_value(self, scope: Option<u64>) -> AbstractValue {
        match self {
            Self::Concrete(_) => self.value(),
            Self::Symbolic(symbol) => self.value().with_symbol(symbol, scope),
        }
    }
}

impl From<Address> for AddressInput {
    fn from(address: Address) -> Self {
        Self::Concrete(address)
    }
}

impl fmt::Display for AddressInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Concrete(address) => address.fmt(f),
            Self::Symbolic(symbol) => write!(f, "symbolic({symbol:?})"),
        }
    }
}

/// Remaining gas is a bound: exact gas accounting is outside this model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum GasInput {
    /// No observed upper bound on gas available to the root frame.
    #[default]
    Unknown,
    /// GAS lies between zero and this initial remaining-gas bound.
    UpperBound(U256),
}

impl GasInput {
    pub(crate) fn value(self) -> AbstractValue {
        match self {
            Self::Unknown => AbstractValue::top(),
            Self::UpperBound(upper) => AbstractValue::unsigned_range(U256::ZERO, upper)
                .expect("zero never exceeds an unsigned bound"),
        }
    }
}

/// Partial transaction blob hashes plus an independently known or unknown count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlobHashes {
    /// Count of versioned hashes; omitted count remains unknown.
    pub length: AbstractValue,
    /// Only observed indices; holes below a known count remain unknown.
    pub hashes: BTreeMap<U256, B256>,
}

impl Default for BlobHashes {
    fn default() -> Self {
        Self {
            length: AbstractValue::top(),
            hashes: BTreeMap::new(),
        }
    }
}

impl BlobHashes {
    /// A complete observed list; indices beyond its length return zero.
    pub fn exact(hashes: &[B256]) -> Self {
        Self {
            length: AbstractValue::constant(U256::from(hashes.len())),
            hashes: hashes
                .iter()
                .enumerate()
                .map(|(index, hash)| (U256::from(index), *hash))
                .collect(),
        }
    }

    pub(crate) fn get(
        &self,
        index: &AbstractValue,
        domain: Domain,
        scope: Option<u64>,
    ) -> AbstractValue {
        let Some(indices) = index.constants() else {
            return if self.length.singleton() == Some(U256::ZERO) {
                AbstractValue::constant(U256::ZERO)
            } else {
                AbstractValue::top()
            };
        };
        let mut values = indices.iter().map(|index| {
            if *index == U256::MAX
                || self
                    .length
                    .singleton()
                    .is_some_and(|length| *index >= length)
            {
                return AbstractValue::constant(U256::ZERO);
            }
            if let Some(hash) = self.hashes.get(index) {
                // An observed hash also establishes that this index exists.
                return AbstractValue::constant(U256::from_be_slice(hash.as_slice()));
            }
            AbstractValue::top().with_symbol(Symbol::BlobHash(*index), scope)
        });
        let first = values.next().expect("finite value candidates are nonempty");
        values.fold(first, |value, incoming| domain.join(&value, &incoming))
    }
}

/// All immutable transaction and block observations consumed by EVM opcodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvmEnvironment {
    /// Identity namespace; cloning preserves the same symbolic inputs.
    #[serde(skip)]
    pub input_scope: InputScope,
    /// Logical root ADDRESS; known world/RPC destinations use a concrete address.
    pub to: AddressInput,
    /// Root CALLER; child callers follow CALL-family rules.
    pub caller: AddressInput,
    /// Transaction ORIGIN; omission aliases the exact same input as caller.
    pub origin: Option<AddressInput>,
    /// Root CALLVALUE; omitted value covers every unsigned word.
    pub value: AbstractValue,
    /// Root calldata, including its length; omitted data is unknown.
    pub calldata: ByteArray,
    /// Whether the root frame is static; children inherit restrictions.
    pub is_static: bool,
    /// Effective transaction gas price.
    pub gas_price: AbstractValue,
    /// Block beneficiary.
    pub coinbase: AddressInput,
    /// Block timestamp.
    pub timestamp: AbstractValue,
    /// Execution block number; RPC snapshots fill the default, while an explicit
    /// override changes execution without changing the acquisition snapshot.
    pub number: AbstractValue,
    /// PREVRANDAO under all supported post-Merge forks.
    pub prevrandao: AbstractValue,
    /// Block gas limit.
    pub gas_limit: AbstractValue,
    /// Execution CHAINID override; omission uses an anchored world's chain ID.
    pub chain_id: Option<AbstractValue>,
    /// Block base fee per gas.
    pub base_fee: AbstractValue,
    /// Block blob base fee.
    pub blob_base_fee: AbstractValue,
    /// Initial remaining-gas upper bound; GAS is never treated as a constant.
    pub gas: GasInput,
    /// Partial BLOCKHASH observations, indexed by full-width block number.
    pub block_hashes: BTreeMap<U256, B256>,
    /// Transaction BLOBHASH observations and count.
    pub blob_hashes: BlobHashes,
}

impl Default for EvmEnvironment {
    fn default() -> Self {
        Self {
            input_scope: InputScope::default(),
            to: AddressInput::unknown_to(),
            caller: AddressInput::unknown_caller(),
            origin: None,
            value: AbstractValue::top(),
            calldata: ByteArray::unknown(),
            is_static: false,
            gas_price: AbstractValue::top(),
            coinbase: AddressInput::unknown_coinbase(),
            timestamp: AbstractValue::top(),
            number: AbstractValue::top(),
            prevrandao: AbstractValue::top(),
            gas_limit: AbstractValue::top(),
            chain_id: None,
            base_fee: AbstractValue::top(),
            blob_base_fee: AbstractValue::top(),
            gas: GasInput::Unknown,
            block_hashes: BTreeMap::new(),
            blob_hashes: BlobHashes::default(),
        }
    }
}

/// Invalid immutable input observations; rejected before acquisition/execution.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EnvironmentError {
    /// Observations must fit a bounded acquisition-sized input table.
    #[error("environment hash tables must have at most 256 block hashes and 4096 blob hashes")]
    TableLimit,
    /// An observed hash contradicts an explicitly known blob count.
    #[error("blob hash index {index} lies outside supplied count {count}")]
    BlobIndex {
        /// Contradictory hash index.
        index: U256,
        /// Explicit blob count.
        count: U256,
    },
    /// Logical destination and the world's concrete state owner must agree.
    #[error("environment destination {observed} differs from root state owner {expected}")]
    Destination {
        /// Root state owner.
        expected: Address,
        /// Supplied logical destination.
        observed: Address,
    },
}

impl EvmEnvironment {
    pub(crate) fn address_value(&self, address: AddressInput) -> AbstractValue {
        address.scoped_value(self.input_scope.id())
    }

    /// Transaction origin with the default caller alias resolved.
    pub fn resolved_origin(&self) -> AddressInput {
        self.origin.unwrap_or(self.caller)
    }

    /// Validate bounded, non-contradictory indexed observations.
    pub fn validate(&self) -> Result<(), EnvironmentError> {
        if self.block_hashes.len() > 256 || self.blob_hashes.hashes.len() > 4096 {
            return Err(EnvironmentError::TableLimit);
        }
        if self.blob_hashes.hashes.contains_key(&U256::MAX) {
            return Err(EnvironmentError::BlobIndex {
                index: U256::MAX,
                count: U256::MAX,
            });
        }
        if let Some(count) = self.blob_hashes.length.singleton()
            && let Some(index) = self
                .blob_hashes
                .hashes
                .keys()
                .find(|index| **index >= count)
        {
            return Err(EnvironmentError::BlobIndex {
                index: *index,
                count,
            });
        }
        Ok(())
    }

    pub(crate) fn work_size(&self) -> usize {
        [
            &self.value,
            &self.gas_price,
            &self.timestamp,
            &self.number,
            &self.prevrandao,
            &self.gas_limit,
            &self.base_fee,
            &self.blob_base_fee,
            &self.blob_hashes.length,
        ]
        .iter()
        .fold(
            self.calldata.work_size().saturating_add(8),
            |cost, value| cost.saturating_add(value.work_size()),
        )
        .saturating_add(self.chain_id.as_ref().map_or(0, AbstractValue::work_size))
        .saturating_add(
            self.block_hashes
                .len()
                .saturating_add(self.blob_hashes.hashes.len())
                .saturating_mul(64),
        )
    }

    pub(crate) fn projection_work(&self, domain: Domain) -> usize {
        [
            &self.value,
            &self.gas_price,
            &self.timestamp,
            &self.number,
            &self.prevrandao,
            &self.gas_limit,
            &self.base_fee,
            &self.blob_base_fee,
            &self.blob_hashes.length,
        ]
        .iter()
        .fold(self.calldata.projection_work(domain), |cost, value| {
            cost.saturating_add(domain.projection_work(value))
        })
        .saturating_add(
            self.chain_id
                .as_ref()
                .map_or(0, |value| domain.projection_work(value)),
        )
        .saturating_add(domain.projection_work(&self.to.value()))
        .saturating_add(domain.projection_work(&self.caller.value()))
        .saturating_add(domain.projection_work(&self.resolved_origin().value()))
        .saturating_add(domain.projection_work(&self.coinbase.value()))
    }

    pub(crate) fn chain_id(&self, identity: &SnapshotIdentity) -> AbstractValue {
        self.chain_id
            .clone()
            .unwrap_or_else(|| match identity {
                SnapshotIdentity::Chain { chain_id, .. } => AbstractValue::constant(*chain_id),
                SnapshotIdentity::Offline { .. } => AbstractValue::top(),
            })
            .with_symbol(Symbol::ChainId, self.input_scope.id())
    }

    pub(crate) fn block_hash(&self, index: &AbstractValue, domain: Domain) -> AbstractValue {
        let current = self.number.singleton();
        let Some(indices) = index.constants() else {
            return if current == Some(U256::ZERO) {
                AbstractValue::constant(U256::ZERO)
            } else {
                AbstractValue::top()
            };
        };
        let mut values = indices.iter().map(|index| {
            if *index == U256::MAX {
                return AbstractValue::constant(U256::ZERO);
            }
            if let Some(current) = current {
                if *index >= current || current - *index > U256::from(256) {
                    return AbstractValue::constant(U256::ZERO);
                }
                return self.block_hashes.get(index).map_or_else(
                    || {
                        AbstractValue::top()
                            .with_symbol(Symbol::BlockHash(*index), self.input_scope.id())
                    },
                    |hash| AbstractValue::constant(U256::from_be_slice(hash.as_slice())),
                );
            }
            // Without NUMBER the index can be either valid or out of range.
            self.block_hashes
                .get(index)
                .map_or_else(AbstractValue::top, |hash| {
                    domain.join(
                        &AbstractValue::constant(U256::ZERO),
                        &AbstractValue::constant(U256::from_be_slice(hash.as_slice())),
                    )
                })
        });
        let first = values.next().expect("finite value candidates are nonempty");
        values.fold(first, |value, incoming| domain.join(&value, &incoming))
    }
}
