//! Transaction-local storage, balances, code and account lifecycle with rollback.

use super::{Code, EvmByteArray, EvmWorld, Existence};
use crate::bytecode::Program;
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{ExprError, ExprId, SlotStateId, SourceId, StateError},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use snapshot_state::SharedCheckpoint;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use {crate::world::test_support::TestAlgebra, crate::world::test_support::TestValue};
#[cfg(test)]
use {crate::world::test_support::TestBytes, crate::world::test_support::TestWorld};

mod clone_payment;
mod effects;
mod finalization;
mod lifecycle;
mod owner_deletion;
mod refinement;
mod root_terminal;
mod slots;
mod sparse;
mod summary;
#[cfg(test)]
mod tests;
mod traversal;
use thiserror::Error;

#[cfg(test)]
use crate::world::test_support::{TestLog, TestStore};
pub use summary::EvmStoreSummary;

/// Ordered state observations with the selected snapshot storage backend.
pub use snapshot_state::OrderedMap;

/// Execution phase of the captured bytecode containing a LOG instruction.
/// Kept in world so event identities do not depend on analysis frame types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSourceKind {
    /// Contract initialization, before returned runtime code is installed.
    Init,
    /// Deployed runtime, including delegated execution.
    Runtime,
}

/// One possible event source. Code and storage owners differ in delegated calls.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LogKey {
    /// Emitter address, i.e. the frame's execution/storage account.
    pub address: Address,
    /// Account whose captured code contains this LOG instruction.
    pub code_address: Address,
    /// Hash of the emitting frame's captured bytes, independent of later writes
    /// to the account's runtime code in the transaction store.
    pub code_hash: B256,
    /// Whether the captured bytes execute as initcode or deployed runtime.
    pub source_kind: LogSourceKind,
    /// LOG instruction's byte offset in the captured code.
    pub pc: usize,
}

/// Possible event payload at a source site.
///
/// Topics and data summarize all visits to the site. The summary retains neither
/// event ordering nor event multiplicity; presence means the event may occur.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename = "AbstractLog")]
pub struct EvmLog<N: WordElement> {
    /// Zero to four ordered topic words.
    pub topics: Vec<EvmValue<N>>,
    /// Memory bytes copied into this event, summarized over site visits.
    pub data: EvmByteArray<N>,
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

impl<N: WordElement> EvmLog<N> {
    fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
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
                work.saturating_add(topic.work_size())
            })
    }
}

fn serialize_logs<S: Serializer, N: WordElement>(
    logs: &OrderedMap<LogKey, EvmLog<N>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Log<'a, N: WordElement> {
        #[serde(flatten)]
        source: &'a LogKey,
        #[serde(flatten)]
        payload: &'a EvmLog<N>,
    }
    let mut sequence = serializer.serialize_seq(Some(logs.len()))?;
    for (source, payload) in logs {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(serde::ser::Error::custom(
                "world log serialization cancelled",
            ));
        }
        sequence.serialize_element(&Log { source, payload })?;
    }
    sequence.end()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Plane<N: WordElement> {
    roots: OrderedMap<Address, SlotStateId>,
    base: SlotStateId,
    #[serde(serialize_with = "super::bytes::native::serialize_facts")]
    operands: BTreeMap<ExprId, EvmValue<N>>,
    #[serde(skip_serializing_if = "super::bytes::conditional::ConditionalFacts::has_no_scopes")]
    conditional: super::bytes::conditional::ConditionalFacts<N>,
    #[serde(serialize_with = "super::bytes::native::serialize_failure")]
    failure: Option<StoreError>,
    #[serde(serialize_with = "serialize_slots")]
    slots: OrderedMap<(Address, U256), EvmValue<N>>,
    defaults: OrderedMap<Address, EvmValue<N>>,
    global_default: EvmValue<N>,
    // These must facts participate in execution reuse and rollback, while the
    // public Store JSON continues to describe transaction values only.
    #[serde(skip)]
    initial_independence: InitialIndependence,
}

/// Whether a current slot is independent of its initial snapshot value on every
/// represented path. Weak writes retain the baseline dependency; a strong write,
/// creation reset or havoc replaces it. Initial RPC facts refine the World and
/// require reexecution, never direct replacement of a transaction-local value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InitialIndependence {
    slots: OrderedMap<(Address, U256), bool>,
    defaults: OrderedMap<Address, bool>,
    global_default: bool,
}

impl InitialIndependence {
    fn new(global_default: bool) -> Self {
        Self {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default,
        }
    }

    fn default_at(&self, address: Address) -> bool {
        self.defaults
            .get(&address)
            .copied()
            .unwrap_or(self.global_default)
    }

    fn at(&self, address: Address, slot: U256) -> bool {
        self.slots
            .get(&(address, slot))
            .copied()
            .unwrap_or_else(|| self.default_at(address))
    }

    fn set_slot(&mut self, address: Address, slot: U256, independent: bool) {
        if independent == self.default_at(address) {
            self.slots.remove(&(address, slot));
        } else {
            self.slots.insert((address, slot), independent);
        }
    }

    fn replace_account(&mut self, address: Address) {
        self.slots.retain(|(owner, _), _| *owner != address);
        if self.global_default {
            self.defaults.remove(&address);
        } else {
            self.defaults.insert(address, true);
        }
    }

    fn replace_all(&mut self) {
        *self = Self::new(true);
    }

    fn join(&self, other: &Self) -> Self {
        let (defaults, global_default) = sparse::combine(
            (&self.defaults, &self.global_default),
            (&other.defaults, &other.global_default),
            |left, right| *left && *right,
        );
        let mut joined = Self {
            defaults,
            ..Self::new(global_default)
        };
        joined.slots = sparse::union_with(&self.slots, &other.slots, |&(address, slot)| {
            let independent = self.at(address, slot) && other.at(address, slot);
            (independent != joined.default_at(address)).then_some(independent)
        });
        joined
    }

    fn work_size(&self) -> usize {
        // Account for each explicit owner/key/flag and the global must fact.
        self.slots
            .len()
            .saturating_mul(3)
            .saturating_add(self.defaults.len().saturating_mul(2))
            .saturating_add(1)
    }
}

fn serialize_slots<S: Serializer, N: WordElement>(
    slots: &OrderedMap<(Address, U256), EvmValue<N>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Slot<'a, N: WordElement> {
        address: Address,
        slot: U256,
        value: &'a EvmValue<N>,
    }
    let mut sequence = serializer.serialize_seq(Some(slots.len()))?;
    for ((address, slot), value) in slots {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(serde::ser::Error::custom(
                "world slot serialization cancelled",
            ));
        }
        sequence.serialize_element(&Slot {
            address: *address,
            slot: *slot,
            value,
        })?;
    }
    sequence.end()
}

impl<N: WordElement> Plane<N> {
    fn new(global_default: EvmValue<N>, initial_independent: bool) -> Self {
        Self {
            roots: OrderedMap::new(),
            base: if global_default.singleton() == Some(U256::ZERO) {
                SlotStateId::zero()
            } else {
                SlotStateId::input(SourceId::fresh().expect("storage source identities available"))
            },
            operands: BTreeMap::new(),
            conditional: Default::default(),
            failure: None,
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default,
            initial_independence: InitialIndependence::new(initial_independent),
        }
    }

    fn default_at(&self, address: Address) -> &EvmValue<N> {
        self.defaults.get(&address).unwrap_or(&self.global_default)
    }

    fn at(&self, address: Address, slot: U256) -> &EvmValue<N> {
        self.slots
            .get(&(address, slot))
            .unwrap_or_else(|| self.default_at(address))
    }

    fn read_numeric_cache<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        slot: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValue<N> {
        if let Some(slot) = slot.singleton() {
            return self.at(address, slot).clone();
        }
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
        let first = values
            .next()
            .expect("AbstractValue constants are nonempty")
            .clone();
        values.fold(first, |value, stored| domain.join(&value, stored))
    }

    fn write_numeric_cache<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        slot: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) {
        // 复制身份只在当前基本块栈上成立，不进入可回滚/复用的持久载荷。
        let mut clean = value.clone();
        clean.forget_identity();
        let value = &clean;
        if let Some(slot) = slot.singleton() {
            self.slots.insert((address, slot), value.clone());
            self.initial_independence.set_slot(address, slot, true);
            return;
        }
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
        for slot in slots {
            let updated = domain.join(self.at(address, *slot), value);
            self.slots.insert((address, *slot), updated);
        }
    }

    fn havoc_account(&mut self, address: Address) {
        self.roots.insert(
            address,
            SlotStateId::input(
                SourceId::fresh().expect("storage havoc source identities available"),
            ),
        );
        self.slots.retain(|(owner, _), _| *owner != address);
        self.defaults.insert(address, EvmValue::<N>::top());
        self.initial_independence.replace_account(address);
    }

    fn reset_account(&mut self, address: Address) {
        self.roots.insert(address, SlotStateId::zero());
        self.slots.retain(|(owner, _), _| *owner != address);
        self.defaults
            .insert(address, EvmValue::<N>::constant(U256::ZERO));
        self.initial_independence.replace_account(address);
    }

    fn havoc_all(&mut self) {
        self.roots.clear();
        self.operands.clear();
        self.conditional = Default::default();
        self.base = SlotStateId::input(
            SourceId::fresh().expect("storage havoc source identities available"),
        );
        self.slots.clear();
        self.defaults.clear();
        self.global_default = EvmValue::<N>::top();
        self.initial_independence.replace_all();
    }

    fn join_numeric_cache<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
        let mut joined = self.clone();
        joined.slots.clear();
        joined.defaults.clear();
        joined.global_default = domain.join(&self.global_default, &other.global_default);
        joined.initial_independence = self.initial_independence.join(&other.initial_independence);
        // Ordinary caches preserve their explicit entries; summary maps below
        // canonicalize values equal to the joined fallback instead.
        joined.defaults = sparse::union_with(&self.defaults, &other.defaults, |address| {
            Some(domain.join(self.default_at(*address), other.default_at(*address)))
        });
        joined.slots = sparse::union_with(&self.slots, &other.slots, |&(address, slot)| {
            Some(domain.join(self.at(address, slot), other.at(address, slot)))
        });
        joined
    }
}

/// Complete symbolic transaction state could not be retained.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StoreError {
    /// Scalar expression retention or cancellation failure.
    #[error("{0}")]
    Expression(ExprError),
    /// Complete immutable slot state retention failed.
    #[error("{0}")]
    State(StateError),
    /// An event byte view could not be retained or renamed.
    #[error("{0}")]
    Bytes(super::RangeError),
}

impl From<ExprError> for StoreError {
    fn from(error: ExprError) -> Self {
        Self::Expression(error)
    }
}

impl From<StateError> for StoreError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<super::RangeError> for StoreError {
    fn from(error: super::RangeError) -> Self {
        Self::Bytes(error)
    }
}

/// Mutable transaction state, shared by nested frames and cloned at call entry.
///
/// Storage ownership is an explicit address, so DELEGATECALL uses its caller's
/// slots. Transient storage begins at zero regardless of snapshot completeness.
/// Creation and deferred deletion change this store; the input world stays fixed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename = "Store")]
pub struct EvmStore<N: WordElement> {
    persistent: Plane<N>,
    transient: Plane<N>,
    balances: OrderedMap<Address, EvmValue<N>>,
    balance_default: EvmValue<N>,
    codes: OrderedMap<Address, Code>,
    nonces: OrderedMap<Address, EvmValue<N>>,
    existence: OrderedMap<Address, Existence>,
    created: OrderedMap<Address, Option<bool>>,
    created_default: Option<bool>,
    pending_destruction: OrderedMap<Address, Option<bool>>,
    pending_destruction_default: Option<bool>,
    // Some(true) is must-touch, Some(false) is untouched, None is may-touch.
    // This is transaction state: joins, summaries and checkpoints must retain it.
    touched: OrderedMap<Address, Option<bool>>,
    touched_default: Option<bool>,
    #[serde(serialize_with = "serialize_logs")]
    possible_logs: OrderedMap<LogKey, EvmLog<N>>,
    logs_unknown: bool,
}

/// An opaque savepoint covering every transaction-local observation and effect.
/// Clones share the complete immutable captured Store; restoration isolates it.
pub type EvmSnapshot<N> = SharedCheckpoint<EvmStore<N>>;

impl<N: WordElement> EvmStore<N> {
    /// Directly owned scalar values copied by Clone. Logs are counted by their
    /// own topics/data containers; slot roots and saved histories stay shared.
    /// Consumers must prepay finalization_entries before scanning these values.
    pub(crate) fn cloned_values(&self) -> impl Iterator<Item = &EvmValue<N>> {
        [&self.persistent, &self.transient]
            .into_iter()
            .flat_map(|plane| {
                plane
                    .slots
                    .values()
                    .chain(plane.defaults.values())
                    .chain(plane.operands.values())
                    .chain(std::iter::once(&plane.global_default))
            })
            .chain(self.balances.values())
            .chain(self.nonces.values())
            .chain(std::iter::once(&self.balance_default))
    }

    /// O(1) census of the owner maps actually visited by `code_identity`.
    /// Duplicate owners intentionally count once per map; no scalar/header
    /// entries or unrelated transaction maps enter an error-key quote.
    pub(crate) fn code_identity_entries(&self) -> usize {
        self.codes
            .len()
            .saturating_add(self.existence.len())
            .saturating_add(self.created.len())
            .saturating_add(self.pending_destruction.len())
    }

    /// Conservative raw-byte allowance for code_identity, independent of
    /// Program digest cache warmth or cheap Code cloning. Consumers must prepay
    /// the flat code-entry scan first.
    pub(crate) fn code_identity_bytes(&self) -> usize {
        self.codes.values().fold(0usize, |bytes, code| {
            bytes.saturating_add(match code {
                Code::Runtime(program) => program.byte_len(),
                _ => 0,
            })
        })
    }

    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(&EvmValue<N>)) {
        for plane in [&self.persistent, &self.transient] {
            for value in plane.slots.values().chain(plane.defaults.values()) {
                visit(value);
            }
            visit(&plane.global_default);
            if plane.failure.is_some() {
                visit(&EvmValue::<N>::top().with_symbolic_limit());
            }
            for value in plane.operands.values() {
                visit(value);
            }
            plane.conditional.visit(&mut |expression, value| {
                if value.is_none_or(|value| value.expression() != Some(expression)) {
                    visit(&EvmValue::<N>::top().with_expression(expression.clone()));
                }
                if let Some(value) = value {
                    visit(value);
                }
            });
            for expression in slots::expressions(&plane.base)
                .into_iter()
                .chain(plane.roots.values().flat_map(slots::expressions))
            {
                visit(&EvmValue::<N>::top().with_expression(expression));
            }
        }
        for value in self.balances.values().chain(self.nonces.values()) {
            visit(value);
        }
        visit(&self.balance_default);
        for log in self.possible_logs.values() {
            for value in &log.topics {
                visit(value);
            }
            log.data.visit_values(visit);
        }
    }
    pub(crate) fn update_values(&mut self, update: &mut impl FnMut(&mut EvmValue<N>)) {
        for plane in [&mut self.persistent, &mut self.transient] {
            plane.slots.update_values(&mut *update);
            plane.defaults.update_values(&mut *update);
            update(&mut plane.global_default);
            for value in plane.operands.values_mut() {
                update(value);
            }
        }
        self.balances.update_values(&mut *update);
        self.nonces.update_values(&mut *update);
        update(&mut self.balance_default);
        self.possible_logs.update_values(|log| {
            for value in &mut log.topics {
                update(value);
            }
            log.data.update_values(update);
        });
    }
    pub(crate) fn project<D: EvmNumericAlgebra<Element = N>>(&mut self, domain: AlgebraRef<'_, D>) {
        let project = |value: &mut EvmValue<N>| {
            *value = domain.project(value);
        };
        for plane in [&mut self.persistent, &mut self.transient] {
            plane.slots.update_values(project);
            plane.defaults.update_values(project);
            project(&mut plane.global_default);
            for value in plane.operands.values_mut() {
                project(value);
            }
            plane.conditional = plane
                .conditional
                .map(domain, &mut |expression, value| {
                    Ok((expression.clone(), value.map(|value| domain.project(value))))
                })
                .expect("numeric projection is infallible");
        }
        self.balances.update_values(project);
        self.nonces.update_values(project);
        project(&mut self.balance_default);
    }
    /// Build transaction state without modifying the fixed input snapshot.
    pub fn new<D: EvmNumericAlgebra<Element = N>>(
        world: &EvmWorld<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
        Self::new_with_limits(world, domain.spec().relations().expressions(), domain)
    }

    /// Initialize every account root under the actual execution domain policy.
    pub fn new_checked<D: EvmNumericAlgebra<Element = N>>(
        world: &EvmWorld<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<Self, StoreError> {
        let limits = domain.spec().relations().expressions();
        let store = Self::new_with_limits(world, limits, domain);
        store.check_existing(limits)?;
        Ok(store)
    }

    fn new_with_limits<D: EvmNumericAlgebra<Element = N>>(
        world: &EvmWorld<N>,
        limits: crate::domain::symbolic::ExprLimits,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
        let mut persistent = Plane::<N>::new(EvmValue::<N>::top(), false);
        persistent.base =
            SlotStateId::input(SourceId::scoped(world.storage_source_token(), U256::ZERO));
        let mut balances = OrderedMap::new();
        let mut codes = OrderedMap::new();
        let mut nonces = OrderedMap::new();
        let mut existence = OrderedMap::new();
        for (address, account) in world.accounts() {
            persistent.defaults.insert(
                *address,
                if account.storage_unknown {
                    EvmValue::<N>::top()
                } else {
                    EvmValue::<N>::constant(U256::ZERO)
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
        persistent.initialize_roots(limits, domain);
        Self {
            persistent,
            transient: Plane::<N>::new(EvmValue::<N>::constant(U256::ZERO), true),
            balances,
            balance_default: EvmValue::<N>::top(),
            codes,
            nonces,
            existence,
            created: OrderedMap::new(),
            created_default: Some(false),
            pending_destruction: OrderedMap::new(),
            pending_destruction_default: Some(false),
            touched: OrderedMap::new(),
            touched_default: Some(false),
            possible_logs: OrderedMap::new(),
            logs_unknown: false,
        }
    }

    /// Read persistent storage, joining finite possible slot aliases.
    pub fn read<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        slot: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValue<N> {
        self.persistent.read(address, slot, domain)
    }

    /// Missing finite initial keys still needed by the current storage value.
    /// The address must be the actual storage owner, including delegated calls.
    /// Unknown aliases are not truncated into a partial list of RPC requests.
    pub(crate) fn missing_initial_slots(
        &self,
        world: &EvmWorld<N>,
        address: Address,
        slots: &EvmValue<N>,
    ) -> Vec<U256> {
        let account = world.account(address);
        let missing = |slot: U256| {
            !self.persistent.initial_independence.at(address, slot)
                && account.is_none_or(|account| {
                    account.storage_unknown && !account.storage.contains_key(&slot)
                })
        };
        if let Some(slot) = slots.singleton() {
            return missing(slot).then_some(slot).into_iter().collect();
        }
        slots.constants().map_or_else(Vec::new, |slots| {
            slots
                .iter()
                .copied()
                .filter(|slot| missing(*slot))
                .collect()
        })
    }

    /// Append an ordered slot write; numeric aliases remain conservative.
    /// Checked callers use write_checked to receive a typed retention failure.
    pub fn write<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        slot: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) {
        if let Err(error) = self.write_checked(address, slot, value, domain) {
            self.persistent.failure = Some(error);
        }
    }

    /// Read transaction-scoped transient storage.
    pub fn read_transient<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        slot: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValue<N> {
        self.transient.read(address, slot, domain)
    }

    /// Update transient storage with the same alias semantics as SSTORE.
    pub fn write_transient<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        slot: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) {
        if let Err(error) = self.write_transient_checked(address, slot, value, domain) {
            self.transient.failure = Some(error);
        }
    }

    /// Current balance of an explicit or unknown account.
    pub fn read_balance(&self, address: Address) -> EvmValue<N> {
        self.balances
            .get(&address)
            .unwrap_or(&self.balance_default)
            .clone()
    }

    /// Replace an account balance after a modeled transfer.
    pub fn write_balance(&mut self, address: Address, value: EvmValue<N>) {
        let nonzero = value.may_be_nonzero();
        let zero = nonzero && value.may_be_zero();
        self.write_balance_with_truth(address, value, zero, nonzero);
    }

    /// Install the same value whose zero/nonzero possibilities the executable
    /// caller has already prepared. Metadata changes do not require discovery.
    pub(crate) fn write_balance_with_truth(
        &mut self,
        address: Address,
        mut value: EvmValue<N>,
        zero: bool,
        nonzero: bool,
    ) {
        value.forget_identity();
        // A positive transfer establishes account presence. A joined balance
        // containing zero cannot discard the possibility of an absent account.
        if nonzero {
            if !zero {
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
        self.codes.get(&address).or_else(|| self.code_default())
    }

    /// Unobserved code for every account without an explicit code entry.
    /// `None` permits any runtime code, delegation marker or empty code.
    pub fn code_default(&self) -> Option<&Code> {
        None
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
        for flag in [self.created_default, self.pending_destruction_default] {
            bytes.push(match flag {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            });
        }
        for address in addresses {
            // Explicit defaults and an omitted owner denote the same code
            // state. Touch-only joins can introduce these cache entries; they
            // must not change a key already used to index the machine state.
            if matches!(self.code(address), None | Some(Code::Unknown))
                && self.existence(address) == Existence::Unknown
                && self.created_in_transaction(address) == self.created_default
                && self.pending_destruction(address) == self.pending_destruction_default
            {
                continue;
            }
            bytes.extend(address.as_slice());
            match self.code(address) {
                Some(Code::Runtime(program)) => {
                    bytes.push(1);
                    bytes.extend(program.bytes_digest().as_slice());
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
    pub fn nonce(&self, address: Address) -> EvmValue<N> {
        self.nonces
            .get(&address)
            .cloned()
            .unwrap_or_else(|| self.nonce_default())
    }

    /// Unconstrained nonce for every account without an explicit nonce entry.
    pub fn nonce_default(&self) -> EvmValue<N> {
        EvmValue::<N>::top()
    }

    /// Replace the nonce after a creation attempt or transaction-local update.
    pub fn write_nonce(&mut self, address: Address, mut nonce: EvmValue<N>) {
        nonce.forget_identity();
        self.nonces.insert(address, nonce);
    }

    /// Account existence is independent of an empty code observation.
    pub fn existence(&self, address: Address) -> Existence {
        self.existence
            .get(&address)
            .copied()
            .unwrap_or_else(|| self.existence_default())
    }

    /// Presence of every account without an explicit existence observation.
    pub fn existence_default(&self) -> Existence {
        Existence::Unknown
    }

    /// Mark an account present after a value transfer. Zero-value transfers do
    /// not establish presence and should not invoke this method.
    pub fn mark_present(&mut self, address: Address) {
        self.existence.insert(address, Existence::Present);
    }

    /// Prepare a collision-free destination for initcode execution. A prefunded
    /// balance survives; persistent and transient slots start at zero.
    pub fn begin_creation(&mut self, address: Address) {
        self.touch_account(address);
        self.persistent.reset_account(address);
        self.transient.reset_account(address);
        self.codes.insert(address, Code::Empty);
        self.nonces
            .insert(address, EvmValue::<N>::constant(U256::from(1)));
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
        self.created
            .get(&address)
            .copied()
            .unwrap_or_else(|| self.created_default())
    }

    /// Creation fact for every account without an explicit transaction entry.
    /// `None` permits both preexisting and transaction-created lifecycles.
    pub fn created_default(&self) -> Option<bool> {
        self.created_default
    }

    /// Whether [EIP-6780](https://eips.ethereum.org/EIPS/eip-6780) schedules this account for deletion at transaction end.
    /// Code remains visible and callable while the transaction is executing.
    pub fn pending_destruction(&self, address: Address) -> Option<bool> {
        self.pending_destruction
            .get(&address)
            .copied()
            .unwrap_or_else(|| self.pending_destruction_default())
    }

    /// Deferred-deletion fact for every account without an explicit entry.
    /// `None` permits both retained and deletion-pending lifecycles.
    pub fn pending_destruction_default(&self) -> Option<bool> {
        self.pending_destruction_default
    }

    /// [EIP-6780](https://eips.ethereum.org/EIPS/eip-6780) balance movement and deferred deletion for all supported forks.
    /// The caller enumerates finite beneficiaries before invoking this operation.
    pub fn selfdestruct<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        beneficiary: Address,
        domain: AlgebraRef<'_, D>,
    ) {
        let result: Result<(), std::convert::Infallible> =
            self.selfdestruct_classified_with(address, beneficiary, domain, None, |value, _| {
                let nonzero = value.may_be_nonzero();
                let zero = nonzero && value.may_be_zero();
                Ok((value, zero, nonzero))
            });
        result.unwrap_or_else(|never| match never {});
    }

    /// Execute using paid classification of this store's unchanged owner
    /// balance. The dispatcher retains both zero and nonzero possibilities;
    /// these facts must be prepared before any transfer or lifecycle effect.
    pub(crate) fn selfdestruct_with_balance_truth<D: EvmNumericAlgebra<Element = N>, E>(
        &mut self,
        address: Address,
        beneficiary: Address,
        domain: AlgebraRef<'_, D>,
        zero: bool,
        nonzero: bool,
        prepare: impl FnMut(
            EvmValue<N>,
            crate::domain::SingletonProof,
        ) -> Result<(EvmValue<N>, bool, bool), E>,
    ) -> Result<(), E> {
        self.selfdestruct_classified_with(
            address,
            beneficiary,
            domain,
            Some((zero, nonzero)),
            prepare,
        )
    }

    fn selfdestruct_classified_with<D: EvmNumericAlgebra<Element = N>, E>(
        &mut self,
        address: Address,
        beneficiary: Address,
        domain: AlgebraRef<'_, D>,
        truth: Option<(bool, bool)>,
        mut prepare: impl FnMut(
            EvmValue<N>,
            crate::domain::SingletonProof,
        ) -> Result<(EvmValue<N>, bool, bool), E>,
    ) -> Result<(), E> {
        let balance = self.read_balance(address);
        let created = self.created_in_transaction(address);
        if address != beneficiary {
            let recipient = self.read_balance(beneficiary);
            let (reduced, proof) = domain.apply_detailed_with_singleton(
                revm_bytecode::opcode::ADD,
                &[recipient, balance.clone()],
            );
            let (recipient, zero, nonzero) = prepare(reduced.value, proof)?;
            self.touch_account(beneficiary);
            self.write_balance_with_truth(beneficiary, recipient, zero, nonzero);
            self.write_balance_with_truth(
                address,
                EvmValue::<N>::constant(U256::ZERO),
                true,
                false,
            );
            let (zero, nonzero) = truth.unwrap_or_else(|| {
                let zero = balance.may_be_zero();
                (zero, zero && balance.may_be_nonzero())
            });
            if !zero {
                self.mark_present(beneficiary);
            } else if nonzero && self.existence(beneficiary) != Existence::Present {
                self.existence.insert(beneficiary, Existence::Unknown);
            }
        } else if created != Some(false) {
            let (value, zero, nonzero) = if created == Some(true) {
                (EvmValue::<N>::constant(U256::ZERO), true, false)
            } else {
                prepare(
                    domain.join(&balance, &EvmValue::<N>::constant(U256::ZERO)),
                    crate::domain::SingletonProof::NotQueried,
                )?
            };
            self.write_balance_with_truth(address, value, zero, nonzero);
        }
        if created != Some(false) {
            self.pending_destruction.insert(address, created);
        }
        Ok(())
    }

    /// Commit deferred EIP-6780 deletion and EIP-161 touched-empty clearing.
    /// Uncertain deletion joins retained/deleted states; untouched empty accounts
    /// and accounts proven nonempty remain intact. Retention or cancellation
    /// errors leave the complete Store unchanged.
    pub fn finalize_transaction<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), StoreError> {
        let limits = domain.spec().relations().expressions();
        self.check_existing(limits)?;
        let mut staged = self.clone();
        staged.prepare_unlisted_finalization(domain)?;
        finalization::Plan::new(&staged).apply(&mut staged, domain, &mut || {
            if analysis_control::current_cancellation().is_cancelled() {
                Err(StoreError::State(StateError::Cancelled))
            } else {
                Ok(())
            }
        })?;
        staged.check_existing(limits)?;
        *self = staged;
        Ok(())
    }

    fn clear_transaction(&mut self) {
        // These flags describe an active transaction, never the next transaction.
        self.created.clear();
        self.created_default = Some(false);
        self.pending_destruction.clear();
        self.pending_destruction_default = Some(false);
        self.touched.clear();
        self.touched_default = Some(false);
        self.transient = Plane::<N>::new(EvmValue::<N>::constant(U256::ZERO), true);
    }

    /// Forget storage, balance, code, nonce and lifecycle facts at an account.
    pub fn havoc_account(&mut self, address: Address) {
        self.may_touch_account(address);
        self.persistent.havoc_account(address);
        self.transient.havoc_account(address);
        self.balances.insert(address, EvmValue::<N>::top());
        self.codes.insert(address, Code::Unknown);
        self.nonces.insert(address, EvmValue::<N>::top());
        self.existence.insert(address, Existence::Unknown);
        self.created.insert(address, None);
        self.pending_destruction.insert(address, None);
        self.logs_unknown = true;
    }

    /// Unknown external code can reenter and affect any account in this world.
    pub fn havoc_all(&mut self) {
        self.may_touch_all();
        self.persistent.havoc_all();
        self.transient.havoc_all();
        self.balances.clear();
        self.balance_default = EvmValue::<N>::top();
        self.codes.update_values(|code| *code = Code::Unknown);
        self.nonces
            .update_values(|nonce| *nonce = EvmValue::<N>::top());
        self.existence
            .update_values(|existence| *existence = Existence::Unknown);
        for address in self.codes.keys() {
            self.created.insert(*address, None);
            self.pending_destruction.insert(*address, None);
        }
        self.logs_unknown = true;
        self.created_default = None;
        self.pending_destruction_default = None;
    }

    /// Join complete transaction states, treating missing keys as defaults.
    /// Disagreeing code observations become unknown; the machine's structural
    /// code identity keeps finite executable versions separate during execution.
    fn join_numeric_cache<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
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
        let (created, created_default) = sparse::combine(
            (&self.created, &self.created_default),
            (&other.created, &other.created_default),
            lifecycle::join,
        );
        let (pending_destruction, pending_destruction_default) = sparse::combine(
            (&self.pending_destruction, &self.pending_destruction_default),
            (
                &other.pending_destruction,
                &other.pending_destruction_default,
            ),
            lifecycle::join,
        );
        let (touched, touched_default) = sparse::combine(
            (&self.touched, &self.touched_default),
            (&other.touched, &other.touched_default),
            lifecycle::join,
        );
        Self {
            persistent: self
                .persistent
                .join_numeric_cache(&other.persistent, domain),
            transient: self.transient.join_numeric_cache(&other.transient, domain),
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
            created,
            created_default,
            pending_destruction,
            pending_destruction_default,
            touched,
            touched_default,
            possible_logs,
            logs_unknown,
        }
    }

    /// Add a possible event, joining repeated visits to its fixed source site.
    /// No order or multiplicity is implied by this idempotent event summary.
    pub fn emit_log<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        key: LogKey,
        mut log: EvmLog<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), LogError> {
        for topic in &mut log.topics {
            topic.forget_identity();
        }
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
    pub fn possible_logs(&self) -> &OrderedMap<LogKey, EvmLog<N>> {
        &self.possible_logs
    }

    /// Unknown external code may emit additional events absent from this map.
    pub fn logs_unknown(&self) -> bool {
        self.logs_unknown
    }

    /// Save all mutable state before entering a child frame.
    pub fn snapshot(&self) -> EvmSnapshot<N> {
        EvmSnapshot::<N>::capture(self)
    }

    /// Roll back a reverted or exceptional child frame.
    pub fn restore(&mut self, snapshot: EvmSnapshot<N>) {
        // All supported forks retain RIPEMD-160's touch on revert. This narrow
        // consensus exception does not retain its balance/storage/log effects.
        // revm 43.0.2 JournalEntry::AccountTouched::revert uses the same rule.
        let ripemd = Address::from_word(U256::from(3).into());
        let ripemd_touched = self.touched_in_transaction(ripemd);
        snapshot.restore(self);
        let restored = lifecycle::or(self.touched_in_transaction(ripemd), ripemd_touched);
        if restored != self.touched_default {
            self.touched.insert(ripemd, restored);
        } else {
            self.touched.remove(&ripemd);
        }
    }

    /// Explicit current persistent slots, keyed by storage owner and slot.
    pub fn slots(&self) -> &OrderedMap<(Address, U256), EvmValue<N>> {
        &self.persistent.slots
    }

    /// Explicit transient slots, keyed independently of executable code identity.
    pub fn transient_slots(&self) -> &OrderedMap<(Address, U256), EvmValue<N>> {
        &self.transient.slots
    }
    /// Symbolic operand guarantees for one independent account storage plane.
    pub fn symbolic_facts(&self, transient: bool) -> impl Iterator<Item = (&ExprId, &EvmValue<N>)> {
        if transient {
            self.transient.operands.iter()
        } else {
            self.persistent.operands.iter()
        }
    }

    /// Default current value for unspecified persistent slots of this account.
    pub fn storage_default(&self, address: Address) -> &EvmValue<N> {
        self.persistent
            .defaults
            .get(&address)
            .unwrap_or(&self.persistent.global_default)
    }

    /// Default current value for unspecified transient slots of this account.
    pub fn transient_default(&self, address: Address) -> &EvmValue<N> {
        self.transient
            .defaults
            .get(&address)
            .unwrap_or(&self.transient.global_default)
    }

    /// Persistent default for accounts without a per-account default.
    pub fn global_storage_default(&self) -> &EvmValue<N> {
        &self.persistent.global_default
    }

    /// Transient default for accounts without a per-account default.
    pub fn global_transient_default(&self) -> &EvmValue<N> {
        &self.transient.global_default
    }

    /// Unscoped immutable persistent input template for omitted owners.
    /// Account reads scope this template by their actual owner address.
    pub fn global_storage_state(&self) -> &SlotStateId {
        &self.persistent.base
    }

    /// Unscoped immutable transient input template for omitted owners.
    /// This preserves the source identity without allocating a scoped view.
    pub fn global_transient_state(&self) -> &SlotStateId {
        &self.transient.base
    }

    /// Balance of accounts without an explicit entry.
    pub fn balance_default(&self) -> &EvmValue<N> {
        &self.balance_default
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
            .chain(self.touched.keys())
            .chain(self.persistent.defaults.keys())
            .chain(self.transient.defaults.keys())
            .chain(self.persistent.roots.keys())
            .chain(self.transient.roots.keys())
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
                work.saturating_add(value.work_size())
            });
        let storage_work =
            [&self.persistent, &self.transient]
                .iter()
                .fold(storage_work, |n, plane| {
                    plane
                        .roots
                        .values()
                        .fold(n.saturating_add(plane.base.work_size()), |n, root| {
                            n.saturating_add(root.work_size())
                        })
                        .saturating_add(
                            plane
                                .operands
                                .values()
                                .fold(0usize, |n, value| n.saturating_add(value.work_size())),
                        )
                        .saturating_add(plane.conditional.work_size())
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
            .saturating_add(self.pending_destruction.len())
            .saturating_add(self.touched.len())
            .saturating_add(3);
        self.possible_logs.values().fold(
            lifecycle_work
                .saturating_add(self.persistent.initial_independence.work_size())
                .saturating_add(self.transient.initial_independence.work_size()),
            |work, log| work.saturating_add(log.work_size()),
        )
    }
}

#[cfg(test)]
mod storage_dependencies;
