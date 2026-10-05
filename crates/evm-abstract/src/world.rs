//! Fixed multi-account input and transaction-local abstract state.
//!
//! An omitted account is unknown, while [`Account::empty`] is a present empty
//! account and [`Account::absent`] confirms absence. Typed snapshot identity,
//! fork and code bytes are fixed before execution. Explicit [`rpc`] acquisition
//! is separate from execution, which changes a [`Store`] for nested rollback.

mod bytes;
pub mod rpc;
mod snapshot;
mod store;

pub use bytes::{ByteArray, RangeError};
pub use snapshot::SnapshotIdentity;
pub use store::{AbstractLog, LogError, LogKey, OrderedMap, Snapshot, Store};

use crate::{Fork, bytecode::DecodeError, bytecode::Program, domain::Value};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Observed code at an account, independently of the account's storage owner.
#[derive(Clone, Debug, Serialize)]
pub enum Code {
    /// Ordinary EVM runtime bytecode decoded under the world's fork.
    Runtime(Program),
    /// EIP-7702's account code pointer; execution retains the delegating account.
    Delegation(Address),
    /// Code was observed to be empty.
    Empty,
    /// The snapshot does not establish the code at this account.
    Unknown,
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
pub struct Account {
    /// Observed runtime code, delegation, empty code, or missing code.
    pub code: Code,
    /// Explicit initial slot values.
    pub storage: BTreeMap<U256, Value>,
    /// Whether unspecified initial storage slots can have any value.
    pub storage_unknown: bool,
    /// Initial balance; an unobserved balance must be [`Value::top`].
    pub balance: Value,
    /// Initial nonce; required for CREATE addresses and collision checks.
    pub nonce: Value,
    /// Explicit account-presence observation, independently of empty code.
    pub existence: Existence,
}

impl Account {
    /// Decode account code, preserving EIP-7702 as a code pointer.
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
            balance: Value::constant(U256::ZERO),
            nonce: Value::constant(U256::ZERO),
            existence: Existence::Present,
        })
    }

    /// A present empty account with zero nonce, balance and storage.
    pub fn empty() -> Self {
        Self {
            code: Code::Empty,
            storage: BTreeMap::new(),
            storage_unknown: false,
            balance: Value::constant(U256::ZERO),
            nonce: Value::constant(U256::ZERO),
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
            balance: Value::top(),
            nonce: Value::top(),
            existence: Existence::Unknown,
        }
    }
}

/// Fixed initial observations. One analysis cannot silently mix identities or forks.
#[derive(Clone, Debug, Serialize)]
pub struct World {
    fork: Fork,
    identity: SnapshotIdentity,
    provenance: String,
    accounts: BTreeMap<Address, Account>,
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
    /// EIP-7702 code delegation is unavailable before Prague.
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

impl World {
    /// Start an unanchored synthetic fixture using provenance as its label.
    /// A free-form string never establishes a live chain or block identity.
    pub fn new(fork: Fork, provenance: impl Into<String>) -> Self {
        let provenance = provenance.into();
        Self {
            fork,
            identity: SnapshotIdentity::Offline {
                label: provenance.clone(),
            },
            provenance,
            accounts: BTreeMap::new(),
        }
    }

    /// Insert an observation, rejecting inconsistent runtime/delegation rules.
    pub fn insert(
        &mut self,
        address: Address,
        account: Account,
    ) -> Result<Option<Account>, WorldError> {
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
                || account.balance != Value::constant(U256::ZERO)
                || account.nonce != Value::constant(U256::ZERO)
                || account.storage_unknown
                || account
                    .storage
                    .values()
                    .any(|value| *value != Value::constant(U256::ZERO)))
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
        account: Account,
        expected: B256,
    ) -> Result<Option<Account>, WorldError> {
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
    pub fn account(&self, address: Address) -> Option<&Account> {
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
    pub fn accounts(&self) -> &BTreeMap<Address, Account> {
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

fn account_code_hash(account: &Account) -> Option<B256> {
    if account.existence == Existence::Absent {
        Some(B256::ZERO)
    } else {
        raw_code(&account.code).map(keccak256)
    }
}

/// External entry frame. Transaction origin is the supplied `caller`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Account entered by the transaction.
    pub address: Address,
    /// External sender, also used as ORIGIN in all nested frames.
    pub caller: Address,
    /// Supplied CALLVALUE abstract value.
    pub value: Value,
    /// Supplied calldata; out-of-range reads are zero.
    pub calldata: ByteArray,
    /// Whether this entry is executed under STATICCALL restrictions.
    pub is_static: bool,
}
