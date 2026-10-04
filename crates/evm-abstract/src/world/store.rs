//! Sparse transaction storage with account-specific defaults and rollback.

use super::{ByteArray, World};
use crate::domain::{Domain, Value};
use alloy_primitives::{Address, U256};
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

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
    logs: &BTreeMap<LogKey, AbstractLog>,
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
    slots: BTreeMap<(Address, U256), Value>,
    defaults: BTreeMap<Address, Value>,
    global_default: Value,
}

fn serialize_slots<S: Serializer>(
    slots: &BTreeMap<(Address, U256), Value>,
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
            slots: BTreeMap::new(),
            defaults: BTreeMap::new(),
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
            for (_, stored) in self
                .slots
                .range_mut((address, U256::ZERO)..=(address, U256::MAX))
            {
                *stored = domain.join(stored, value);
            }
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Store {
    persistent: Plane,
    transient: Plane,
    balances: BTreeMap<Address, Value>,
    balance_default: Value,
    #[serde(serialize_with = "serialize_logs")]
    possible_logs: BTreeMap<LogKey, AbstractLog>,
    logs_unknown: bool,
}

/// An opaque savepoint covering persistent, transient, and balance changes.
#[derive(Clone, Debug)]
pub struct Snapshot(Store);

impl Store {
    /// Build transaction state without modifying the fixed input snapshot.
    pub fn new(world: &World) -> Self {
        let mut persistent = Plane::new(Value::top());
        let mut balances = BTreeMap::new();
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
        }
        Self {
            persistent,
            transient: Plane::new(Value::constant(U256::ZERO)),
            balances,
            balance_default: Value::top(),
            possible_logs: BTreeMap::new(),
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
        self.balances.insert(address, value);
    }

    /// Forget all possible effects at a storage owner, including its balance.
    pub fn havoc_account(&mut self, address: Address) {
        self.persistent.havoc_account(address);
        self.transient.havoc_account(address);
        self.balances.insert(address, Value::top());
        self.logs_unknown = true;
    }

    /// Unknown external code can reenter and affect any account in this world.
    pub fn havoc_all(&mut self) {
        self.persistent.havoc_all();
        self.transient.havoc_all();
        self.balances.clear();
        self.balance_default = Value::top();
        self.logs_unknown = true;
    }

    /// Join complete transaction states, treating missing keys as defaults.
    pub fn join(&self, other: &Self, domain: Domain) -> Self {
        let addresses: BTreeSet<_> = self
            .balances
            .keys()
            .chain(other.balances.keys())
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
            possible_logs
                .entry(key.clone())
                .and_modify(|existing| *existing = existing.join(log, domain))
                .or_insert_with(|| log.clone());
        }
        Self {
            persistent: self.persistent.join(&other.persistent, domain),
            transient: self.transient.join(&other.transient, domain),
            balances: addresses
                .into_iter()
                .map(|address| {
                    (
                        address,
                        domain.join(&self.read_balance(address), &other.read_balance(address)),
                    )
                })
                .collect(),
            balance_default: domain.join(&self.balance_default, &other.balance_default),
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
        self.possible_logs
            .entry(key)
            .and_modify(|existing| *existing = existing.join(&log, domain))
            .or_insert(log);
        Ok(())
    }

    /// Possible event payloads grouped by source site.
    pub fn possible_logs(&self) -> &BTreeMap<LogKey, AbstractLog> {
        &self.possible_logs
    }

    /// Unknown external code may emit additional events absent from this map.
    pub fn logs_unknown(&self) -> bool {
        self.logs_unknown
    }

    /// Save all mutable state before entering a child frame.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot(self.clone())
    }

    /// Roll back a reverted or exceptional child frame.
    pub fn restore(&mut self, snapshot: Snapshot) {
        *self = snapshot.0;
    }

    /// Explicit current persistent slots, keyed by storage owner and slot.
    pub fn slots(&self) -> &BTreeMap<(Address, U256), Value> {
        &self.persistent.slots
    }

    /// Cost estimate for joining explicit values and all missing-key defaults.
    pub fn work_size(&self) -> usize {
        let storage_work = self
            .persistent
            .slots
            .values()
            .chain(self.persistent.defaults.values())
            .chain(self.transient.slots.values())
            .chain(self.transient.defaults.values())
            .chain(self.balances.values())
            .chain([
                &self.persistent.global_default,
                &self.transient.global_default,
                &self.balance_default,
            ])
            .fold(0_usize, |work, value| {
                work.saturating_add(value.constants().map_or(1, |values| values.len()))
            });
        self.possible_logs.values().fold(storage_work, |work, log| {
            work.saturating_add(log.work_size())
        })
    }
}
