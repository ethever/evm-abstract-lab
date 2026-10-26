//! Fixed multi-account input and transaction-local abstract state.
//!
//! An omitted account is unknown, while [`EvmAccount::empty`] is a present empty
//! account and [`EvmAccount::absent`] confirms absence. Typed snapshot identity,
//! fork and code bytes are fixed before execution. Explicit [`rpc`] acquisition
//! is separate from execution, which changes a [`EvmStore`] for nested rollback.

use crate::domain::{EvmNumericAlgebra, WordElement};
#[cfg(test)]
use crate::world::test_support::TestValue;
mod bytes;
pub mod environment;
mod initial;
#[cfg(test)]
mod native_algebras_tests;
pub mod process;
pub mod rpc;
#[cfg(test)]
mod runtime_code_tests;
mod snapshot;
#[cfg(test)]
mod test_support;
pub use environment::{
    AddressInput, EvmBlobHashes, EvmEnvironmentValues, GasInput, InputScope, Symbol,
};
mod store;

pub use bytes::{
    EvmByteArray, EvmByteArraySummary, EvmByteArraySummaryView, EvmByteContentView,
    GuardRefinementError, RangeError,
};
pub use snapshot::{FingerprintError, SnapshotEnvironment, SnapshotIdentity};
pub use store::{
    EvmLog, EvmPlaneSummaryView, EvmSnapshot, EvmStore, EvmStoreSummary, EvmStoreSummaryView,
    LogError, LogKey, LogSourceKind, OrderedMap, StoreError,
};

use crate::{Fork, bytecode::DecodeError, bytecode::Program, domain::EvmValue};
use alloy_primitives::{Address, B256, U256, hex, keccak256};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Complete word candidates, including exact elements without a finite view.
/// The singleton is held inline; querying an independently represented constant
/// must not allocate a new finite set or invoke another numeric policy.
struct Candidates<'a> {
    finite: Option<&'a std::collections::BTreeSet<U256>>,
    singleton: Option<U256>,
}
impl Candidates<'_> {
    fn iter(&self) -> impl Iterator<Item = &U256> {
        self.finite
            .into_iter()
            .flatten()
            .chain(self.singleton.iter())
    }
    fn is_subset(&self, values: &std::collections::BTreeSet<U256>) -> bool {
        self.iter().all(|value| values.contains(value))
    }
}
fn candidates<N: WordElement>(value: &EvmValue<N>) -> Option<Candidates<'_>> {
    let finite = value.constants();
    let singleton = finite.is_none().then(|| value.singleton()).flatten();
    (finite.is_some() || singleton.is_some()).then_some(Candidates { finite, singleton })
}
fn unsigned_bounds<N: WordElement>(value: &EvmValue<N>) -> (U256, U256) {
    value
        .singleton()
        .map_or_else(|| value.interval().unsigned_bounds(), |word| (word, word))
}

/// Observed code at an account, independently of the account's storage owner.
#[derive(Clone, Debug, Serialize)]
pub enum Code {
    /// Ordinary EVM runtime bytecode decoded under the world's fork.
    Runtime(#[serde(serialize_with = "serialize_runtime_code")] Program),
    /// [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702)'s account code pointer; execution retains the delegating account.
    Delegation(Address),
    /// Code was observed to be empty.
    Empty,
    /// The snapshot does not establish the code at this account.
    Unknown,
}

// Account observations retain their captured bytes, including historical Store
// versions. Decoded blocks remain the global Program and frame JSON contract.
fn serialize_runtime_code<S: serde::Serializer>(
    program: &Program,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct RuntimeCode {
        fork: Fork,
        byte_len: usize,
        bytecode: String,
    }
    RuntimeCode {
        fork: program.fork(),
        byte_len: program.byte_len(),
        bytecode: hex::encode_prefixed(program.bytes()),
    }
    .serialize(serializer)
}

impl PartialEq for Code {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Runtime(left), Self::Runtime(right)) => {
                left.fork() == right.fork() && left.bytes() == right.bytes()
            }
            (Self::Delegation(left), Self::Delegation(right)) => left == right,
            (Self::Empty, Self::Empty) | (Self::Unknown, Self::Unknown) => true,
            _ => false,
        }
    }
}

impl Eq for Code {}

/// Whether the snapshot establishes that an account exists in the state trie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Existence {
    /// No account-presence observation was supplied.
    Unknown,
    /// The account exists, even if its code and balance are empty.
    Present,
    /// The account is explicitly absent, with zero nonce, balance and storage.
    Absent,
}

/// One initial account observation. Missing slots are zero only if complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename = "Account")]
pub struct EvmAccount<N: WordElement> {
    /// Observed runtime code, delegation, empty code, or missing code.
    pub code: Code,
    /// Explicit initial slot values.
    pub storage: BTreeMap<U256, EvmValue<N>>,
    /// Whether unspecified initial storage slots can have any value.
    pub storage_unknown: bool,
    /// Initial balance; an unobserved balance must be [`EvmValue::top`].
    pub balance: EvmValue<N>,
    /// Initial nonce; required for CREATE addresses and collision checks.
    pub nonce: EvmValue<N>,
    /// Explicit account-presence observation, independently of empty code.
    pub existence: Existence,
}

impl<N: WordElement> EvmAccount<N> {
    /// Decode account code, preserving [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) as a code pointer.
    ///
    /// This synthetic constructor defaults storage, balance and nonce to zero
    /// and establishes presence. Partial inputs must supply unknown facts
    /// explicitly; JSON and RPC loaders enforce their own observation boundary.
    pub fn from_hex(input: &str, fork: Fork) -> Result<Self, DecodeError> {
        let code = match Program::from_hex_with_fork(input, fork) {
            Ok(program) if program.byte_len() == 0 => Code::Empty,
            Ok(program) => Code::Runtime(program),
            Err(DecodeError::DelegatedCode { address }) => Code::Delegation(address),
            Err(error) => return Err(error),
        };
        Ok(Self {
            code,
            storage: BTreeMap::new(),
            storage_unknown: false,
            balance: EvmValue::<N>::constant(U256::ZERO),
            nonce: EvmValue::<N>::constant(U256::ZERO),
            existence: Existence::Present,
        })
    }

    /// A present empty account with zero nonce, balance and storage.
    pub fn empty() -> Self {
        Self {
            code: Code::Empty,
            storage: BTreeMap::new(),
            storage_unknown: false,
            balance: EvmValue::<N>::constant(U256::ZERO),
            nonce: EvmValue::<N>::constant(U256::ZERO),
            existence: Existence::Present,
        }
    }

    /// A confirmed absent account, distinct from a present empty account.
    pub fn absent() -> Self {
        Self {
            existence: Existence::Absent,
            ..Self::empty()
        }
    }

    /// An account whose presence, code, nonce, balance and storage are unobserved.
    pub fn unknown() -> Self {
        Self {
            code: Code::Unknown,
            storage: BTreeMap::new(),
            storage_unknown: true,
            balance: EvmValue::<N>::top(),
            nonce: EvmValue::<N>::top(),
            existence: Existence::Unknown,
        }
    }
}

/// Fixed initial observations. One analysis cannot silently mix identities or forks.
#[derive(Clone, Debug, Serialize)]
#[serde(rename = "World")]
pub struct EvmWorld<N: WordElement> {
    fork: Fork,
    identity: SnapshotIdentity,
    // Free-form offline labels are descriptions, not source equality identities.
    // Clone retains this process-private token; independent fixtures get fresh ones.
    storage_source: crate::domain::symbolic::ExprId,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_environment: Option<SnapshotEnvironment>,
    provenance: String,
    accounts: BTreeMap<Address, EvmAccount<N>>,
}

/// Invalid account insertion into an otherwise coherent snapshot.
#[derive(Debug, Error)]
pub enum WorldError {
    /// Runtime bytecode was decoded under a different rule set.
    #[error("account uses {account}, but world uses {world}")]
    MixedFork {
        /// World's fixed fork.
        world: Fork,
        /// Account runtime's fork.
        account: Fork,
    },
    /// [EIP-7702](https://eips.ethereum.org/EIPS/eip-7702) code delegation is unavailable before Prague.
    #[error("EIP-7702 delegation is unavailable under {0}")]
    UnsupportedDelegation(Fork),
    /// An address already has a different observation in this snapshot.
    #[error("conflicting snapshot observation at {address}")]
    Conflict {
        /// Address whose established facts cannot be overwritten.
        address: Address,
    },
    /// Supplied hash does not describe the actual observed code bytes.
    #[error("code hash mismatch at {address}: expected {expected}, observed {observed}")]
    CodeHash {
        /// Account supplying the inconsistent code observation.
        address: Address,
        /// Declared Keccak-256 of the code bytes (zero for confirmed absence).
        expected: B256,
        /// Computed hash, or zero for confirmed absence.
        observed: B256,
    },
    /// A declared code hash cannot be checked against missing code bytes.
    #[error("code hash at {0} has no observed code bytes")]
    UnknownCodeHash(Address),
    /// Confirmed absence is inconsistent with an observed nonempty fact.
    #[error("absent account {0} has nonzero or unknown state")]
    InvalidAbsence(Address),
}

impl<N: WordElement> EvmWorld<N> {
    /// Install a checked batch of new exact RPC slots without opaque queries.
    /// Acquisition filters cached keys; a duplicate never overwrites native state.
    pub(crate) fn install_storage(
        &mut self,
        address: Address,
        storage: &BTreeMap<U256, U256>,
    ) -> Result<(), WorldError> {
        let account = self
            .accounts
            .get(&address)
            .ok_or(WorldError::Conflict { address })?;
        for (slot, value) in storage {
            if account.storage.contains_key(slot)
                || (!account.storage_unknown || account.existence == Existence::Absent)
                    && *value != U256::ZERO
            {
                return Err(WorldError::Conflict { address });
            }
        }
        let present = storage.values().any(|value| *value != U256::ZERO);
        let account = self
            .accounts
            .get_mut(&address)
            .expect("validated account exists");
        account.storage.extend(
            storage
                .iter()
                .map(|(slot, value)| (*slot, EvmValue::<N>::constant(*value))),
        );
        if present && account.existence == Existence::Unknown {
            account.existence = Existence::Present;
        }
        Ok(())
    }

    /// Store 初始化投影可能生成的完整候选；复制费用由输入 work_size 另计。
    pub(crate) fn projection_work<D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: crate::domain::AlgebraRef<'_, D>,
    ) -> usize {
        self.accounts.values().fold(0usize, |work, account| {
            let default = if account.storage_unknown {
                0
            } else {
                domain.projection_work(&EvmValue::<N>::constant(U256::ZERO))
            };
            account.storage.values().fold(
                work.saturating_add(domain.projection_work(&account.balance))
                    .saturating_add(domain.projection_work(&account.nonce))
                    .saturating_add(default),
                |n, value| n.saturating_add(domain.projection_work(value)),
            )
        })
    }
    pub(crate) fn work_size(&self) -> usize {
        self.accounts.values().fold(
            if self.snapshot_environment.is_some() {
                32
            } else {
                16
            },
            |work, account| {
                let code = match &account.code {
                    Code::Runtime(program) => program.byte_len().saturating_mul(3),
                    Code::Delegation(_) => 23,
                    _ => 1,
                };
                account.storage.values().fold(
                    work.saturating_add(code)
                        .saturating_add(account.balance.work_size())
                        .saturating_add(account.nonce.work_size()),
                    |n, value| n.saturating_add(value.work_size()),
                )
            },
        )
    }
    /// Start an unanchored synthetic fixture using provenance as its label.
    /// A free-form string never establishes a live chain or block identity.
    pub fn new(fork: Fork, provenance: impl Into<String>) -> Self {
        let provenance = provenance.into();
        Self {
            fork,
            identity: SnapshotIdentity::Offline {
                label: provenance.clone(),
            },
            storage_source: crate::domain::symbolic::ExprId::fresh()
                .expect("snapshot source identity namespace available"),
            snapshot_environment: None,
            provenance,
            accounts: BTreeMap::new(),
        }
    }

    /// Insert an observation, rejecting inconsistent runtime/delegation rules.
    pub fn insert(
        &mut self,
        address: Address,
        account: EvmAccount<N>,
    ) -> Result<Option<EvmAccount<N>>, WorldError> {
        match &account.code {
            Code::Runtime(program) if program.fork() != self.fork => {
                return Err(WorldError::MixedFork {
                    world: self.fork,
                    account: program.fork(),
                });
            }
            Code::Delegation(_) if !self.fork.supports_delegation() => {
                return Err(WorldError::UnsupportedDelegation(self.fork));
            }
            _ => {}
        }
        if account.existence == Existence::Absent
            && (account.code != Code::Empty
                || account.balance.singleton() != Some(U256::ZERO)
                || account.nonce.singleton() != Some(U256::ZERO)
                || account.storage_unknown
                || account
                    .storage
                    .values()
                    .any(|value| value.singleton() != Some(U256::ZERO)))
        {
            return Err(WorldError::InvalidAbsence(address));
        }
        if let Some(previous) = self.accounts.get(&address) {
            if previous != &account {
                return Err(WorldError::Conflict { address });
            }
            return Ok(Some(previous.clone()));
        }
        Ok(self.accounts.insert(address, account))
    }

    /// Insert code bytes only when their Keccak hash agrees with the snapshot.
    pub fn insert_with_code_hash(
        &mut self,
        address: Address,
        account: EvmAccount<N>,
        expected: B256,
    ) -> Result<Option<EvmAccount<N>>, WorldError> {
        let observed = account_code_hash(&account).ok_or(WorldError::UnknownCodeHash(address))?;
        if observed != expected {
            return Err(WorldError::CodeHash {
                address,
                expected,
                observed,
            });
        }
        self.insert(address, account)
    }

    /// Execution rules fixed when the snapshot was created.
    pub fn fork(&self) -> Fork {
        self.fork
    }

    /// Identifies the source/block snapshot chosen by the caller.
    pub fn provenance(&self) -> &str {
        &self.provenance
    }

    /// Account observation; absence denotes unknown code and persistent state.
    pub fn account(&self, address: Address) -> Option<&EvmAccount<N>> {
        self.accounts.get(&address)
    }

    /// Actual runtime owned by this account; does not follow a delegation.
    pub fn runtime(&self, address: Address) -> Option<&Program> {
        match &self.account(address)?.code {
            Code::Runtime(program) => Some(program),
            _ => None,
        }
    }

    /// Observed account code bytes, including the original delegation marker.
    /// Unknown account/code observations return `None`.
    pub fn raw_account_code(&self, address: Address) -> Option<Vec<u8>> {
        raw_code(&self.account(address)?.code)
    }

    /// Observed code hash; confirmed absence is zero and missing code is unknown.
    /// Account presence must also be considered when implementing EXTCODEHASH.
    pub fn code_hash(&self, address: Address) -> Option<B256> {
        account_code_hash(self.account(address)?)
    }

    /// All explicit observations, ordered by address.
    pub fn accounts(&self) -> &BTreeMap<Address, EvmAccount<N>> {
        &self.accounts
    }
}

fn raw_code(code: &Code) -> Option<Vec<u8>> {
    match code {
        Code::Runtime(program) => Some(program.bytes().to_vec()),
        Code::Delegation(target) => {
            let mut bytes = Vec::with_capacity(23);
            bytes.extend_from_slice(&[0xef, 0x01, 0x00]);
            bytes.extend_from_slice(target.as_slice());
            Some(bytes)
        }
        Code::Empty => Some(Vec::new()),
        Code::Unknown => None,
    }
}

fn account_code_hash<N: WordElement>(account: &EvmAccount<N>) -> Option<B256> {
    if account.existence == Existence::Absent {
        Some(B256::ZERO)
    } else {
        raw_code(&account.code).map(keccak256)
    }
}

/// Original root binding, independent of execution or process admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RootInputBinding {
    /// The request supplied an actual physical destination.
    Physical {
        /// Concrete logical To, validated against the root state coordinate.
        address: Address,
    },
    /// The request supplied only a program/state template at this coordinate.
    Namespace {
        /// Private template coordinate; no physical account is asserted.
        coordinate: Address,
    },
}

/// One root frame and its immutable transaction and block inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmEntry<N: WordElement> {
    /// Supplied root state coordinate. With concrete To this is the physical
    /// destination; with symbolic To it is a private program/state namespace,
    /// which may alias named or outside physical accounts after binding.
    pub address: Address,
    /// Root call inputs and transaction/block observations shared by all frames.
    pub environment: EvmEnvironmentValues<N>,
}

impl<N: WordElement> Serialize for EvmEntry<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut entry = serializer.serialize_struct("Entry", 3)?;
        entry.serialize_field("binding", &self.binding())?;
        entry.serialize_field("address", &self.address)?;
        entry.serialize_field("environment", &self.environment)?;
        entry.end()
    }
}

impl<N: WordElement> EvmEntry<N> {
    /// Constant metadata available even if no frame or process was admitted.
    pub fn binding(&self) -> RootInputBinding {
        self.environment.to.as_concrete().map_or(
            RootInputBinding::Namespace {
                coordinate: self.address,
            },
            |address| RootInputBinding::Physical { address },
        )
    }
    /// Analyze all unspecified call inputs at a known destination address.
    pub fn new(address: Address) -> Self {
        Self {
            address,
            environment: EvmEnvironmentValues {
                to: AddressInput::Concrete(address),
                ..EvmEnvironmentValues::<N>::default()
            },
        }
    }

    /// Create a concrete root call while leaving other environment facts unknown.
    pub fn concrete(
        address: Address,
        caller: Address,
        value: EvmValue<N>,
        calldata: EvmByteArray<N>,
    ) -> Self {
        let mut entry = Self::new(address);
        entry.environment.caller = AddressInput::Concrete(caller);
        entry.environment.value = value;
        entry.environment.calldata = calldata;
        entry
    }
}
