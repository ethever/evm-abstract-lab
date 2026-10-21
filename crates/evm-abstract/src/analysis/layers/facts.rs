//! Exact, monotone observations about one immutable initial-state set.

use super::catalog::CodeId;
use crate::{Address, U256, world::SnapshotIdentity};
use alloy_primitives::B256;
use serde::Serialize;
use snapshot_state::OrderedMap;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn fresh_id() -> u64 {
    NEXT_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("initial fact identity namespace available")
}

/// Snapshot identity with a private namespace for independently created offline inputs.
///
/// Labels alone never equate offline snapshots. Cloning preserves identity;
/// anchored observations compare the complete chain ID and pinned block hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SnapshotId {
    identity: SnapshotIdentity,
    offline_namespace: Option<u64>,
}

impl SnapshotId {
    /// New offline snapshot, distinct even from another using the same label.
    pub fn offline(label: impl Into<String>) -> Self {
        Self {
            identity: SnapshotIdentity::Offline {
                label: label.into(),
            },
            offline_namespace: Some(fresh_id()),
        }
    }

    /// Exact chain snapshot. A moving tag or block number is insufficient.
    pub fn chain(chain_id: U256, block_hash: B256) -> Self {
        Self {
            identity: SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            },
            offline_namespace: None,
        }
    }

    /// Public chain identity or offline description.
    pub fn identity(&self) -> &SnapshotIdentity {
        &self.identity
    }

    /// Exact process-local namespace of an offline snapshot; chain inputs have none.
    pub fn offline_namespace(&self) -> Option<u64> {
        self.offline_namespace
    }
}

/// Process-local identity of an immutable fact revision, never a chain height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FactRevision(u64);

impl FactRevision {
    /// Exact process-local revision identity, independent of any block height.
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Missing observations are distinct from every known value, including zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Observed<T> {
    /// No supplied fact constrains this input.
    Unobserved,
    /// Exact observation, or a consequence of explicit absence/completeness.
    Known(T),
}

/// Observed account code refers to separately compiled, state-independent semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CodeObservation {
    /// Runtime bytes in a code catalog; no account state is implied.
    Runtime(CodeId),
    /// Exact EIP-7702 code pointer, independent of storage ownership.
    Delegation(Address),
    /// Code was explicitly observed empty.
    Empty,
}

/// Initial-state locations. These keys are also the future engine dependency seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum FactKey {
    /// Account code, independent of existence.
    Code(Address),
    /// Account balance.
    Balance(Address),
    /// Account nonce.
    Nonce(Address),
    /// Account presence in the initial trie.
    Existence(Address),
    /// One concrete persistent slot owned by an account.
    Storage {
        /// Storage owner, which can differ from the executing code address.
        address: Address,
        /// Full-width key.
        slot: U256,
    },
    /// An explicitly complete storage map, whose unspecified slots are zero.
    StorageComplete(Address),
}

impl FactKey {
    /// Account owning this initial observation.
    pub fn address(self) -> Address {
        match self {
            Self::Code(address)
            | Self::Balance(address)
            | Self::Nonce(address)
            | Self::Existence(address)
            | Self::StorageComplete(address)
            | Self::Storage { address, .. } => address,
        }
    }
}

/// A typed exact observation at the declared initial snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Observation {
    /// Code bytes/pointer alone do not establish balance, nonce, slots or existence.
    Code {
        /// Code account.
        address: Address,
        /// Observed code fact.
        code: CodeObservation,
    },
    /// Exact initial balance; zero must be supplied explicitly.
    Balance {
        /// Account address.
        address: Address,
        /// Observed word.
        value: U256,
    },
    /// Exact initial nonce.
    Nonce {
        /// Account address.
        address: Address,
        /// Observed word.
        value: U256,
    },
    /// Explicit presence or absence; absence entails empty code and zero state.
    Existence {
        /// Account address.
        address: Address,
        /// `false` confirms nonexistence, distinct from a missing observation.
        exists: bool,
    },
    /// One exact initial slot, including a known zero.
    Storage {
        /// State owner.
        address: Address,
        /// Full-width slot key.
        slot: U256,
        /// Observed value.
        value: U256,
    },
    /// Declare the map complete; omitted keys are zero, not unobserved.
    StorageComplete {
        /// State owner.
        address: Address,
    },
}

impl Observation {
    /// Location restricted by this observation.
    pub fn key(&self) -> FactKey {
        match *self {
            Self::Code { address, .. } => FactKey::Code(address),
            Self::Balance { address, .. } => FactKey::Balance(address),
            Self::Nonce { address, .. } => FactKey::Nonce(address),
            Self::Existence { address, .. } => FactKey::Existence(address),
            Self::Storage { address, slot, .. } => FactKey::Storage { address, slot },
            Self::StorageComplete { address } => FactKey::StorageComplete(address),
        }
    }
}

/// Invalid refinements install no part of their batch.
#[derive(Debug, Error)]
pub enum FactError {
    /// Facts at different pinned snapshots cannot refine one another.
    #[error("initial observations belong to a different snapshot")]
    Snapshot {
        /// Session's immutable identity.
        expected: Box<SnapshotId>,
        /// Rejected batch identity.
        observed: Box<SnapshotId>,
    },
    /// A known initial fact may not be overwritten or contradicted indirectly.
    #[error("conflicting initial observation at {key:?}")]
    Conflict {
        /// Conflicting location; the original fact revision is unchanged.
        key: FactKey,
    },
}

/// Persistent initial observations. Refinement returns a new value and revision.
#[derive(Clone, Debug)]
pub struct StateFacts {
    snapshot: SnapshotId,
    revision: FactRevision,
    observations: OrderedMap<FactKey, Observation>,
}

/// Successful atomic refinement and the precise keys added by that batch.
#[derive(Clone, Debug)]
pub struct FactRefinement {
    facts: StateFacts,
    added: Vec<FactKey>,
}

impl FactRefinement {
    /// Refined immutable facts; the previous value remains valid independently.
    pub fn facts(&self) -> &StateFacts {
        &self.facts
    }

    /// Newly installed observations. Repeating a fact adds nothing.
    pub fn added(&self) -> &[FactKey] {
        &self.added
    }

    /// Transfer ownership of the refined version.
    pub fn into_facts(self) -> StateFacts {
        self.facts
    }
}

impl StateFacts {
    /// An entirely unobserved initial state at one fixed snapshot.
    pub fn new(snapshot: SnapshotId) -> Self {
        Self {
            snapshot,
            revision: FactRevision(fresh_id()),
            observations: OrderedMap::new(),
        }
    }

    /// Identity is fixed across all refinements of this value.
    pub fn snapshot(&self) -> &SnapshotId {
        &self.snapshot
    }

    /// Cache/invalidation identity. Idempotent batches keep the same revision.
    pub fn revision(&self) -> FactRevision {
        self.revision
    }

    /// Explicit observations, without materializing consequences of absence.
    pub fn observations(&self) -> impl Iterator<Item = (&FactKey, &Observation)> {
        self.observations.iter()
    }

    /// Restrict the same initial-state set atomically, never a live execution Store.
    pub fn refine(
        &self,
        snapshot: &SnapshotId,
        observations: impl IntoIterator<Item = Observation>,
    ) -> Result<FactRefinement, FactError> {
        if snapshot != &self.snapshot {
            return Err(FactError::Snapshot {
                expected: Box::new(self.snapshot.clone()),
                observed: Box::new(snapshot.clone()),
            });
        }
        let mut next = self.clone();
        let mut added = Vec::new();
        for observation in observations {
            let key = observation.key();
            if let Some(previous) = next.observations.get(&key) {
                if previous != &observation {
                    return Err(FactError::Conflict { key });
                }
                continue;
            }
            // Earlier completeness is a negative observation for every missing
            // slot. A later batch cannot turn such an established zero nonzero.
            if let Observation::Storage {
                address,
                slot,
                value,
            } = observation
                && self.storage(address, slot) == Observed::Known(U256::ZERO)
                && value != U256::ZERO
            {
                return Err(FactError::Conflict { key });
            }
            next.observations.insert(key, observation);
            added.push(key);
        }
        next.validate_absence()?;
        if !added.is_empty() {
            next.revision = FactRevision(fresh_id());
        }
        Ok(FactRefinement { facts: next, added })
    }

    fn validate_absence(&self) -> Result<(), FactError> {
        for (key, observation) in &self.observations {
            if self.existence(key.address()) != Observed::Known(false) {
                continue;
            }
            let compatible = match observation {
                Observation::Code { code, .. } => {
                    *code == CodeObservation::Empty
                        || matches!(code, CodeObservation::Runtime(id) if id.hash() == alloy_primitives::keccak256([]))
                }
                Observation::Balance { value, .. }
                | Observation::Nonce { value, .. }
                | Observation::Storage { value, .. } => *value == U256::ZERO,
                Observation::Existence { .. } | Observation::StorageComplete { .. } => true,
            };
            if !compatible {
                return Err(FactError::Conflict { key: *key });
            }
        }
        Ok(())
    }

    /// Account presence, independent of known code, balance, nonce, or storage.
    pub fn existence(&self, address: Address) -> Observed<bool> {
        match self.observations.get(&FactKey::Existence(address)) {
            Some(Observation::Existence { exists, .. }) => Observed::Known(*exists),
            _ => Observed::Unobserved,
        }
    }

    /// Code fact. Confirmed absence entails empty code; missing code stays unknown.
    pub fn code(&self, address: Address) -> Observed<CodeObservation> {
        if self.existence(address) == Observed::Known(false) {
            return Observed::Known(CodeObservation::Empty);
        }
        match self.observations.get(&FactKey::Code(address)) {
            Some(Observation::Code { code, .. }) => Observed::Known(*code),
            _ => Observed::Unobserved,
        }
    }

    /// Balance observation; an absent account has known zero balance.
    pub fn balance(&self, address: Address) -> Observed<U256> {
        if self.existence(address) == Observed::Known(false) {
            return Observed::Known(U256::ZERO);
        }
        match self.observations.get(&FactKey::Balance(address)) {
            Some(Observation::Balance { value, .. }) => Observed::Known(*value),
            _ => Observed::Unobserved,
        }
    }

    /// Nonce observation; code identity never supplies a default nonce.
    pub fn nonce(&self, address: Address) -> Observed<U256> {
        if self.existence(address) == Observed::Known(false) {
            return Observed::Known(U256::ZERO);
        }
        match self.observations.get(&FactKey::Nonce(address)) {
            Some(Observation::Nonce { value, .. }) => Observed::Known(*value),
            _ => Observed::Unobserved,
        }
    }

    /// Whether every omitted initial slot has been explicitly established zero.
    pub fn storage_complete(&self, address: Address) -> bool {
        self.existence(address) == Observed::Known(false)
            || self
                .observations
                .get(&FactKey::StorageComplete(address))
                .is_some()
    }

    /// Initial slot observation, preserving unobserved/known-zero distinctions.
    pub fn storage(&self, address: Address, slot: U256) -> Observed<U256> {
        match self.observations.get(&FactKey::Storage { address, slot }) {
            Some(Observation::Storage { value, .. }) => Observed::Known(*value),
            _ if self.storage_complete(address) => Observed::Known(U256::ZERO),
            _ => Observed::Unobserved,
        }
    }
}
