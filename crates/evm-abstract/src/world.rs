//! Fixed multi-account input and transaction-local abstract state.
//!
//! An omitted account is unknown, while [`Account::empty`] is an explicitly
//! observed empty account. The world records one fork and snapshot provenance;
//! execution changes a separate [`Store`] so nested calls can roll back.

mod bytes;
mod store;

pub use bytes::{ByteArray, RangeError};
pub use store::{AbstractLog, LogError, LogKey, Snapshot, Store};

use crate::{Fork, bytecode::DecodeError, bytecode::Program, domain::Value};
use alloy_primitives::{Address, U256};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Observed code at an account, independently of the account's storage owner.
#[derive(Clone, Debug, Serialize)]
pub enum Code {
    /// Legacy runtime bytecode decoded under the world's fork.
    Runtime(Program),
    /// EIP-7702's account code pointer; execution retains the delegating account.
    Delegation(Address),
    /// Code was observed to be empty.
    Empty,
    /// The snapshot does not establish the code at this account.
    Unknown,
}

/// One initial account observation. Missing slots are zero only if complete.
#[derive(Clone, Debug, Serialize)]
pub struct Account {
    /// Observed runtime code, delegation, empty code, or missing code.
    pub code: Code,
    /// Explicit initial slot values.
    pub storage: BTreeMap<U256, Value>,
    /// Whether unspecified initial storage slots can have any value.
    pub storage_unknown: bool,
    /// Initial balance; an unobserved balance must be [`Value::top`].
    pub balance: Value,
}

impl Account {
    /// Decode account code, preserving EIP-7702 as a code pointer.
    ///
    /// Initial storage and balance default to zero. Partial snapshots should
    /// set `storage_unknown` and the balance explicitly.
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
        })
    }

    /// An observed empty account with zero balance and storage.
    pub fn empty() -> Self {
        Self {
            code: Code::Empty,
            storage: BTreeMap::new(),
            storage_unknown: false,
            balance: Value::constant(U256::ZERO),
        }
    }

    /// An account whose code, balance, and initial storage are unobserved.
    pub fn unknown() -> Self {
        Self {
            code: Code::Unknown,
            storage: BTreeMap::new(),
            storage_unknown: true,
            balance: Value::top(),
        }
    }
}

/// A fixed offline snapshot. One analysis cannot silently mix fork rules.
#[derive(Clone, Debug, Serialize)]
pub struct World {
    fork: Fork,
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
}

impl World {
    /// Start a fixed snapshot with a caller-supplied provenance identifier.
    pub fn new(fork: Fork, provenance: impl Into<String>) -> Self {
        Self {
            fork,
            provenance: provenance.into(),
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
        Ok(self.accounts.insert(address, account))
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
        match &self.account(address)?.code {
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

    /// All explicit observations, ordered by address.
    pub fn accounts(&self) -> &BTreeMap<Address, Account> {
        &self.accounts
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
