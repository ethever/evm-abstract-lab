//! Transaction-local storage, balances, code and account lifecycle with rollback.

use super::{ByteArray, Code, Existence, World};
use crate::bytecode::Program;
use crate::domain::{Domain, Value};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Serialize, Serializer};
use snapshot_state::Checkpoint;
use std::collections::BTreeSet;
use thiserror::Error;

/// Ordered state observations with the selected snapshot storage backend.
pub use snapshot_state::OrderedMap;

/// One possible event source. Code and storage owners differ in delegated calls.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LogKey {
    /// Emitter address, i.e. the frame's execution/storage account.
    pub address: Address,
    /// Account whose runtime contains this LOG instruction.
    pub code_address: Address,
    /// LOG instruction's byte offset in that runtime.
    pub pc: usize,
}

/// Possible event payload at a source site.
///
/// Topics and data summarize all visits to the site. The summary retains neither
/// event ordering nor event multiplicity; presence means the event may occur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AbstractLog {
    /// Zero to four ordered topic words.
    pub topics: Vec<Value>,
    /// Memory bytes copied into this event, summarized over site visits.
    pub data: ByteArray,
}

/// Violations of LOG's source-site and topic-count invariants.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LogError {
    /// EVM LOG instructions have at most four topics.
    #[error("LOG cannot contain {0} topics; maximum is four")]
    TooManyTopics(usize),
    /// A fixed opcode at one source site cannot change its topic arity.
    #[error("LOG source site previously had {previous} topics, received {incoming}")]
    InconsistentTopics {
        /// Established source-site topic count.
        previous: usize,
        /// Rejected new source-site topic count.
        incoming: usize,
    },
}

impl AbstractLog {
    fn join(&self, other: &Self, domain: Domain) -> Self {
        Self {
            topics: self
                .topics
                .iter()
                .zip(&other.topics)
                .map(|(left, right)| domain.join(left, right))
                .collect(),
            data: self.data.join(&other.data, domain),
        }
    }

    fn work_size(&self) -> usize {
        self.topics
            .iter()
            .fold(self.data.work_size(), |work, topic| {
                work.saturating_add(topic.constants().map_or(1, |values| values.len()))
            })
    }
}

fn serialize_logs<S: Serializer>(
    logs: &OrderedMap<LogKey, AbstractLog>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Log<'a> {
        #[serde(flatten)]
        source: &'a LogKey,
        #[serde(flatten)]
        payload: &'a AbstractLog,
    }
    logs.iter()
        .map(|(source, payload)| Log { source, payload })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Plane {
    #[serde(serialize_with = "serialize_slots")]
    slots: OrderedMap<(Address, U256), Value>,
    defaults: OrderedMap<Address, Value>,
    global_default: Value,
}

fn serialize_slots<S: Serializer>(
    slots: &OrderedMap<(Address, U256), Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Slot<'a> {
        address: Address,
        slot: U256,
        value: &'a Value,
    }
    slots
        .iter()
        .map(|((address, slot), value)| Slot {
            address: *address,
            slot: *slot,
            value,
        })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

impl Plane {
    fn new(global_default: Value) -> Self {
        Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default,
        }
    }

    fn default_at(&self, address: Address) -> &Value {
        self.defaults.get(&address).unwrap_or(&self.global_default)
    }

    fn at(&self, address: Address, slot: U256) -> &Value {
        self.slots
            .get(&(address, slot))
            .unwrap_or_else(|| self.default_at(address))
    }

    fn read(&self, address: Address, slot: &Value, domain: Domain) -> Value {
        let Some(slots) = slot.constants() else {
            // Even a complete account can contain arbitrarily many zero slots
            // plus the explicitly stored ones. Join all those possibilities.
            return self
                .slots
                .range((address, U256::ZERO)..=(address, U256::MAX))
                .fold(self.default_at(address).clone(), |value, (_, stored)| {
                    domain.join(&value, stored)
                });
        };
        let mut values = slots.iter().map(|slot| self.at(address, *slot));
        let first = values.next().expect("Value constants are nonempty").clone();
        values.fold(first, |value, stored| domain.join(&value, stored))
    }

    fn write(&mut self, address: Address, slot: &Value, value: &Value, domain: Domain) {
        let Some(slots) = slot.constants() else {
            // A symbolic alias may replace any slot, including unlisted ones.
            let default = domain.join(self.default_at(address), value);
            self.slots
                .update_range((address, U256::ZERO)..=(address, U256::MAX), |_, stored| {
                    *stored = domain.join(stored, value)
                });
            self.defaults.insert(address, default);
            return;
        };
        let strong = slots.len() == 1;
        for slot in slots {
            let updated = if strong {
                value.clone()
            } else {
                domain.join(self.at(address, *slot), value)
            };
            self.slots.insert((address, *slot), updated);
        }
    }

    fn havoc_account(&mut self, address: Address) {
        self.slots.retain(|(owner, _), _| *owner != address);
        self.defaults.insert(address, Value::top());
    }

    fn reset_account(&mut self, address: Address) {
        self.slots.retain(|(owner, _), _| *owner != address);
        self.defaults.insert(address, Value::constant(U256::ZERO));
    }

    fn havoc_all(&mut self) {
        self.slots.clear();
        self.defaults.clear();
        self.global_default = Value::top();
    }

    fn join(&self, other: &Self, domain: Domain) -> Self {
        let mut joined = Self::new(domain.join(&self.global_default, &other.global_default));
        let addresses: BTreeSet<_> = self
            .defaults
            .keys()
            .chain(other.defaults.keys())
            .copied()
            .collect();
        for address in addresses {
            joined.defaults.insert(
                address,
                domain.join(self.default_at(address), other.default_at(address)),
            );
        }
        let keys: BTreeSet<_> = self
            .slots
            .keys()
            .chain(other.slots.keys())
            .copied()
            .collect();
        for (address, slot) in keys {
            joined.slots.insert(
                (address, slot),
                domain.join(self.at(address, slot), other.at(address, slot)),
            );
        }
        joined
    }
}

/// Mutable transaction state, shared by nested frames and cloned at call entry.
///
/// Storage ownership is an explicit address, so DELEGATECALL uses its caller's
/// slots. Transient storage begins at zero regardless of snapshot completeness.
/// Creation and deferred deletion change this store; the input world stays fixed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Store {
    persistent: Plane,
    transient: Plane,
    balances: OrderedMap<Address, Value>,
    balance_default: Value,
    codes: OrderedMap<Address, Code>,
    nonces: OrderedMap<Address, Value>,
    existence: OrderedMap<Address, Existence>,
    created: OrderedMap<Address, Option<bool>>,
    pending_destruction: OrderedMap<Address, Option<bool>>,
    #[serde(serialize_with = "serialize_logs")]
    possible_logs: OrderedMap<LogKey, AbstractLog>,
    logs_unknown: bool,
}

/// An opaque savepoint covering every transaction-local observation and effect.
pub type Snapshot = Checkpoint<Store>;

impl Store {
    /// Build transaction state without modifying the fixed input snapshot.
    pub fn new(world: &World) -> Self {
        let mut persistent = Plane::new(Value::top());
        let mut balances = OrderedMap::new();
        let mut codes = OrderedMap::new();
        let mut nonces = OrderedMap::new();
        let mut existence = OrderedMap::new();
        for (address, account) in world.accounts() {
            persistent.defaults.insert(
                *address,
                if account.storage_unknown {
                    Value::top()
                } else {
                    Value::constant(U256::ZERO)
                },
            );
            persistent.slots.extend(
                account
                    .storage
                    .iter()
                    .map(|(slot, value)| ((*address, *slot), value.clone())),
            );
            balances.insert(*address, account.balance.clone());
            codes.insert(*address, account.code.clone());
            nonces.insert(*address, account.nonce.clone());
            existence.insert(*address, account.existence);
        }
        Self {
            persistent,
            transient: Plane::new(Value::constant(U256::ZERO)),
            balances,
            balance_default: Value::top(),
            codes,
            nonces,
            existence,
            created: OrderedMap::new(),
            pending_destruction: OrderedMap::new(),
            possible_logs: OrderedMap::new(),
            logs_unknown: false,
        }
    }

    /// Read persistent storage, joining finite possible slot aliases.
    pub fn read(&self, address: Address, slot: &Value, domain: Domain) -> Value {
        self.persistent.read(address, slot, domain)
    }

    /// Strongly update one slot; weakly update finite or unknown aliases.
    pub fn write(&mut self, address: Address, slot: &Value, value: &Value, domain: Domain) {
        self.persistent.write(address, slot, value, domain);
    }

    /// Read transaction-scoped transient storage.
    pub fn read_transient(&self, address: Address, slot: &Value, domain: Domain) -> Value {
        self.transient.read(address, slot, domain)
    }

    /// Update transient storage with the same alias semantics as SSTORE.
    pub fn write_transient(
        &mut self,
        address: Address,
        slot: &Value,
        value: &Value,
        domain: Domain,
    ) {
        self.transient.write(address, slot, value, domain);
    }

    /// Current balance of an explicit or unknown account.
    pub fn read_balance(&self, address: Address) -> Value {
        self.balances
            .get(&address)
            .unwrap_or(&self.balance_default)
            .clone()
    }

    /// Replace an account balance after a modeled transfer.
    pub fn write_balance(&mut self, address: Address, value: Value) {
        // A positive transfer establishes account presence. A joined balance
        // containing zero cannot discard the possibility of an absent account.
        if value.may_be_nonzero() {
            if !value.may_be_zero() {
                self.mark_present(address);
            } else if self.existence(address) == Existence::Absent {
                self.existence.insert(address, Existence::Unknown);
            }
        }
        self.balances.insert(address, value);
    }

    /// Current observed code, including runtime deployed earlier in this transaction.
    /// Omitted accounts and joins of different code observations remain unknown.
    pub fn code(&self, address: Address) -> Option<&Code> {
        self.codes.get(&address)
    }

    /// Structural identity for executable account versions and their lifecycle.
    /// Values such as balances and slots can join within this identity. Different
    /// code/deletion histories stay separate so finite creation does not become
    /// unknown code merely by meeting an immediate failure path.
    pub fn code_identity(&self) -> B256 {
        let addresses: BTreeSet<_> = self
            .codes
            .keys()
            .chain(self.existence.keys())
            .chain(self.created.keys())
            .chain(self.pending_destruction.keys())
            .copied()
            .collect();
        let mut bytes = Vec::new();
        for address in addresses {
            bytes.extend(address.as_slice());
            match self.code(address) {
                Some(Code::Runtime(program)) => {
                    bytes.push(1);
                    bytes.extend(keccak256(program.bytes()).as_slice());
                }
                Some(Code::Delegation(target)) => {
                    bytes.push(2);
                    bytes.extend(target.as_slice());
                }
                Some(Code::Empty) => bytes.push(3),
                Some(Code::Unknown) | None => bytes.push(4),
            }
            bytes.push(match self.existence(address) {
                Existence::Unknown => 0,
                Existence::Present => 1,
                Existence::Absent => 2,
            });
            bytes.push(match self.created_in_transaction(address) {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            });
            bytes.push(match self.pending_destruction(address) {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            });
        }
        keccak256(bytes)
    }

    /// Actual current bytes, preserving a delegation marker without following it.
    pub fn raw_account_code(&self, address: Address) -> Option<Vec<u8>> {
        match self.code(address)? {
            Code::Runtime(program) => Some(program.bytes().to_vec()),
            Code::Empty => Some(Vec::new()),
            Code::Delegation(target) => {
                let mut bytes = vec![0xef, 0x01, 0x00];
                bytes.extend_from_slice(target.as_slice());
                Some(bytes)
            }
            Code::Unknown => None,
        }
    }

    /// Current account nonce; unobserved accounts carry an unknown value.
    pub fn nonce(&self, address: Address) -> Value {
        self.nonces
            .get(&address)
            .cloned()
            .unwrap_or_else(Value::top)
    }

    /// Replace the nonce after a creation attempt or transaction-local update.
    pub fn write_nonce(&mut self, address: Address, nonce: Value) {
        self.nonces.insert(address, nonce);
    }

    /// Account existence is independent of an empty code observation.
    pub fn existence(&self, address: Address) -> Existence {
        self.existence
            .get(&address)
            .copied()
            .unwrap_or(Existence::Unknown)
    }

    /// Mark an account present after a value transfer. Zero-value transfers do
    /// not establish presence and should not invoke this method.
    pub fn mark_present(&mut self, address: Address) {
        self.existence.insert(address, Existence::Present);
    }

    /// Prepare a collision-free destination for initcode execution. A prefunded
    /// balance survives; persistent and transient slots start at zero.
    pub fn begin_creation(&mut self, address: Address) {
        self.persistent.reset_account(address);
        self.transient.reset_account(address);
        self.codes.insert(address, Code::Empty);
        self.nonces.insert(address, Value::constant(U256::from(1)));
        self.existence.insert(address, Existence::Present);
        self.created.insert(address, Some(true));
        self.pending_destruction.insert(address, Some(false));
    }

    /// Install the runtime returned by successful, validated initcode.
    pub fn deploy_code(&mut self, address: Address, program: Program) {
        self.codes.insert(
            address,
            if program.byte_len() == 0 {
                Code::Empty
            } else {
                Code::Runtime(program)
            },
        );
    }

    /// Whether this account was created in this transaction; `None` represents
    /// a join between created and preexisting lifecycles.
    pub fn created_in_transaction(&self, address: Address) -> Option<bool> {
        self.created.get(&address).copied().unwrap_or(Some(false))
    }

    /// Whether EIP-6780 schedules this account for deletion at transaction end.
    /// Code remains visible and callable while the transaction is executing.
    pub fn pending_destruction(&self, address: Address) -> Option<bool> {
        self.pending_destruction
            .get(&address)
            .copied()
            .unwrap_or(Some(false))
    }

    /// EIP-6780 balance movement and deferred deletion for all supported forks.
    /// The caller enumerates finite beneficiaries before invoking this operation.
    pub fn selfdestruct(&mut self, address: Address, beneficiary: Address, domain: Domain) {
        let balance = self.read_balance(address);
        let created = self.created_in_transaction(address);
        if address != beneficiary {
            let recipient = self.read_balance(beneficiary);
            self.write_balance(
                beneficiary,
                domain.apply(revm_bytecode::opcode::ADD, &[recipient, balance.clone()]),
            );
            self.write_balance(address, Value::constant(U256::ZERO));
            if !balance.may_be_zero() {
                self.mark_present(beneficiary);
            } else if balance.may_be_nonzero() && self.existence(beneficiary) != Existence::Present
            {
                self.existence.insert(beneficiary, Existence::Unknown);
            }
        } else if created != Some(false) {
            self.write_balance(
                address,
                if created == Some(true) {
                    Value::constant(U256::ZERO)
                } else {
                    domain.join(&balance, &Value::constant(U256::ZERO))
                },
            );
        }
        if created != Some(false) {
            self.pending_destruction.insert(address, created);
        }
    }

    /// Commit deferred EIP-6780 deletion only at outermost completion. A joined
    /// optional deletion conservatively joins the retained and deleted accounts.
    pub fn finalize_transaction(&mut self, domain: Domain) {
        let pending = std::mem::take(&mut self.pending_destruction);
        for (address, pending) in pending {
            if pending == Some(false) {
                continue;
            }
            let mut deleted = self.clone();
            deleted.persistent.reset_account(address);
            deleted.transient.reset_account(address);
            deleted.codes.insert(address, Code::Empty);
            deleted.nonces.insert(address, Value::constant(U256::ZERO));
            deleted
                .balances
                .insert(address, Value::constant(U256::ZERO));
            deleted.existence.insert(address, Existence::Absent);
            if pending == Some(true) {
                *self = deleted;
            } else {
                *self = self.join(&deleted, domain);
            }
        }
        // These flags describe an active transaction, never the next transaction.
        self.created.clear();
    }

    /// Forget storage, balance, code, nonce and lifecycle facts at an account.
    pub fn havoc_account(&mut self, address: Address) {
        self.persistent.havoc_account(address);
        self.transient.havoc_account(address);
        self.balances.insert(address, Value::top());
        self.codes.insert(address, Code::Unknown);
        self.nonces.insert(address, Value::top());
        self.existence.insert(address, Existence::Unknown);
        self.created.insert(address, None);
        self.pending_destruction.insert(address, None);
        self.logs_unknown = true;
    }

    /// Unknown external code can reenter and affect any account in this world.
    pub fn havoc_all(&mut self) {
        self.persistent.havoc_all();
        self.transient.havoc_all();
        self.balances.clear();
        self.balance_default = Value::top();
        self.codes.update_values(|code| *code = Code::Unknown);
        self.nonces.update_values(|nonce| *nonce = Value::top());
        self.existence
            .update_values(|existence| *existence = Existence::Unknown);
        for address in self.codes.keys() {
            self.created.insert(*address, None);
            self.pending_destruction.insert(*address, None);
        }
        self.logs_unknown = true;
    }

    /// Join complete transaction states, treating missing keys as defaults.
    /// Disagreeing code observations become unknown; the machine's structural
    /// code identity keeps finite executable versions separate during execution.
    pub fn join(&self, other: &Self, domain: Domain) -> Self {
        let addresses: BTreeSet<_> = self
            .balances
            .keys()
            .chain(other.balances.keys())
            .chain(self.codes.keys())
            .chain(other.codes.keys())
            .chain(self.nonces.keys())
            .chain(other.nonces.keys())
            .chain(self.existence.keys())
            .chain(other.existence.keys())
            .chain(self.created.keys())
            .chain(other.created.keys())
            .chain(self.pending_destruction.keys())
            .chain(other.pending_destruction.keys())
            .copied()
            .collect();
        let mut possible_logs = self.possible_logs.clone();
        let mut logs_unknown = self.logs_unknown || other.logs_unknown;
        for (key, log) in &other.possible_logs {
            if possible_logs
                .get(key)
                .is_some_and(|existing| existing.topics.len() != log.topics.len())
            {
                // Store joins are intended for one fixed world. Preserve a
                // conservative result even for stores created against different
                // code at the same address/site, rather than asserting/panicking.
                possible_logs.remove(key);
                logs_unknown = true;
                continue;
            }
            if let Some(existing) = possible_logs.get_mut(key) {
                *existing = existing.join(log, domain);
            } else {
                possible_logs.insert(key.clone(), log.clone());
            }
        }
        Self {
            persistent: self.persistent.join(&other.persistent, domain),
            transient: self.transient.join(&other.transient, domain),
            balances: addresses
                .iter()
                .copied()
                .map(|address| {
                    (
                        address,
                        domain.join(&self.read_balance(address), &other.read_balance(address)),
                    )
                })
                .collect(),
            balance_default: domain.join(&self.balance_default, &other.balance_default),
            codes: addresses
                .iter()
                .map(|address| {
                    (
                        *address,
                        match (self.code(*address), other.code(*address)) {
                            (Some(left), Some(right)) if left == right => left.clone(),
                            _ => Code::Unknown,
                        },
                    )
                })
                .collect(),
            nonces: addresses
                .iter()
                .map(|address| {
                    (
                        *address,
                        domain.join(&self.nonce(*address), &other.nonce(*address)),
                    )
                })
                .collect(),
            existence: addresses
                .iter()
                .map(|address| {
                    (
                        *address,
                        if self.existence(*address) == other.existence(*address) {
                            self.existence(*address)
                        } else {
                            Existence::Unknown
                        },
                    )
                })
                .collect(),
            created: addresses
                .iter()
                .map(|address| {
                    (
                        *address,
                        if self.created_in_transaction(*address)
                            == other.created_in_transaction(*address)
                        {
                            self.created_in_transaction(*address)
                        } else {
                            None
                        },
                    )
                })
                .collect(),
            pending_destruction: addresses
                .iter()
                .map(|address| {
                    (
                        *address,
                        if self.pending_destruction(*address) == other.pending_destruction(*address)
                        {
                            self.pending_destruction(*address)
                        } else {
                            None
                        },
                    )
                })
                .collect(),
            possible_logs,
            logs_unknown,
        }
    }

    /// Add a possible event, joining repeated visits to its fixed source site.
    /// No order or multiplicity is implied by this idempotent event summary.
    pub fn emit_log(
        &mut self,
        key: LogKey,
        log: AbstractLog,
        domain: Domain,
    ) -> Result<(), LogError> {
        if log.topics.len() > 4 {
            return Err(LogError::TooManyTopics(log.topics.len()));
        }
        if let Some(existing) = self.possible_logs.get(&key)
            && existing.topics.len() != log.topics.len()
        {
            return Err(LogError::InconsistentTopics {
                previous: existing.topics.len(),
                incoming: log.topics.len(),
            });
        }
        if let Some(existing) = self.possible_logs.get_mut(&key) {
            *existing = existing.join(&log, domain);
        } else {
            self.possible_logs.insert(key, log);
        }
        Ok(())
    }

    /// Possible event payloads grouped by source site.
    pub fn possible_logs(&self) -> &OrderedMap<LogKey, AbstractLog> {
        &self.possible_logs
    }

    /// Unknown external code may emit additional events absent from this map.
    pub fn logs_unknown(&self) -> bool {
        self.logs_unknown
    }

    /// Save all mutable state before entering a child frame.
    pub fn snapshot(&self) -> Snapshot {
        Checkpoint::capture(self)
    }

    /// Roll back a reverted or exceptional child frame.
    pub fn restore(&mut self, snapshot: Snapshot) {
        snapshot.restore(self);
    }

    /// Explicit current persistent slots, keyed by storage owner and slot.
    pub fn slots(&self) -> &OrderedMap<(Address, U256), Value> {
        &self.persistent.slots
    }

    /// Accounts with explicit initial observations or transaction effects.
    /// This finite set does not imply that omitted accounts are absent.
    pub fn addresses(&self) -> BTreeSet<Address> {
        self.balances
            .keys()
            .chain(self.codes.keys())
            .chain(self.nonces.keys())
            .chain(self.existence.keys())
            .chain(self.created.keys())
            .chain(self.pending_destruction.keys())
            .chain(self.persistent.defaults.keys())
            .chain(self.transient.defaults.keys())
            .copied()
            .chain(self.persistent.slots.keys().map(|(address, _)| *address))
            .chain(self.transient.slots.keys().map(|(address, _)| *address))
            .chain(
                self.possible_logs
                    .keys()
                    .flat_map(|source| [source.address, source.code_address]),
            )
            .collect()
    }

    /// Cost estimate covering values, defaults, code bytes and lifecycle flags.
    pub fn work_size(&self) -> usize {
        let storage_work = self
            .persistent
            .slots
            .values()
            .chain(self.persistent.defaults.values())
            .chain(self.transient.slots.values())
            .chain(self.transient.defaults.values())
            .chain(self.balances.values())
            .chain(self.nonces.values())
            .chain([
                &self.persistent.global_default,
                &self.transient.global_default,
                &self.balance_default,
            ])
            .fold(0_usize, |work, value| {
                work.saturating_add(value.constants().map_or(1, |values| values.len()))
            });
        let lifecycle_work = self
            .codes
            .values()
            .fold(storage_work, |work, code| {
                work.saturating_add(match code {
                    Code::Runtime(program) => program.byte_len(),
                    _ => 1,
                })
            })
            .saturating_add(self.existence.len())
            .saturating_add(self.created.len())
            .saturating_add(self.pending_destruction.len());
        self.possible_logs
            .values()
            .fold(lifecycle_work, |work, log| {
                work.saturating_add(log.work_size())
            })
    }
}
