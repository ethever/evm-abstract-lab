//! Content epochs bound names and captured code without inspecting Store keys.

use super::{ProcessOwner, ProcessOwnerError};
use crate::analysis::ProcessSource;
use crate::{
    Fork,
    resource::WorkBudget,
    resource::clone_payment::{checkpoint, pay},
};
use alloy_primitives::{Address, B256, keccak256};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};

/// Captured executable identity; storage owner is independently classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapturedProcessCode {
    /// Account supplying these bytes (including delegated code sources).
    pub source: Address,
    /// Hash of the original captured executable bytes.
    pub hash: B256,
    /// The decoder/interpreter rules for those bytes.
    pub fork: Fork,
}

/// Admission bounds, applied before allocating vocabulary containers.
#[derive(Clone, Copy, Debug)]
pub struct OwnerCatalogueLimits {
    /// Maximum supplied names, including mandatory captured accounts.
    pub names: usize,
    /// Maximum supplied captured code versions.
    pub codes: usize,
}

/// Private Store coordinates are not established physical target names.
#[derive(Clone, Copy, Debug)]
pub struct ProcessNamespace {
    /// Coordinate of the supplied state template, never an ADDRESS operand.
    pub coordinate: Address,
    /// Exact supplied or resolved executable origin of the namespace root.
    pub source: ProcessSource,
}

/// Class key only; code/source/static/caller roles still belong to frame shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessOwnerKey {
    /// Digest of the complete frozen names and code vocabulary.
    pub epoch: B256,
    /// State namespace or physical owner class, independent of ADDRESS.
    pub owner: ProcessOwner,
}

/// Immutable process vocabulary. Discoveries require a new content epoch.
#[derive(Debug)]
pub struct OwnerCatalogue {
    named: BTreeSet<Address>,
    coordinates: BTreeSet<Address>,
    namespace: Option<ProcessNamespace>,
    codes: BTreeSet<CapturedProcessCode>,
    epoch: B256,
}

impl OwnerCatalogue {
    /// Freeze explicit inputs. Never derives names from an evolving Store.
    /// All supplied captured code sources must also be supplied as names.
    pub fn freeze(
        names: &[Address],
        codes: &[CapturedProcessCode],
        limits: OwnerCatalogueLimits,
        budget: &mut WorkBudget,
    ) -> Result<Arc<Self>, ProcessOwnerError> {
        Self::freeze_with_namespace(names, codes, None, limits, budget)
    }

    /// Freeze physical names separately from a supplied root state namespace.
    /// The equal numeric physical address stays in the outside complement.
    pub fn freeze_with_namespace(
        names: &[Address],
        codes: &[CapturedProcessCode],
        namespace: Option<ProcessNamespace>,
        limits: OwnerCatalogueLimits,
        budget: &mut WorkBudget,
    ) -> Result<Arc<Self>, ProcessOwnerError> {
        pay(budget, 8)?;
        if names.len() > limits.names || codes.len() > limits.codes {
            return Err(ProcessOwnerError::Capacity);
        }
        // Fixed key widths and a full tree-height bound cover collection,
        // membership, digest bytes and allocation before any of them run.
        let entries = names.len().saturating_add(codes.len()).saturating_add(2);
        pay(
            budget,
            entries
                .saturating_add(1)
                .saturating_mul((usize::BITS as usize + 1).saturating_mul(128)),
        )?;
        let named: BTreeSet<_> = names.iter().copied().collect();
        if namespace.is_some_and(|namespace| named.contains(&namespace.coordinate)) {
            return Err(ProcessOwnerError::Shape);
        }
        let mut coordinates = named.clone();
        if let Some(namespace) = namespace {
            coordinates.insert(namespace.coordinate);
        }
        let codes: BTreeSet<_> = codes.iter().copied().collect();
        for code in &codes {
            if !named.contains(&code.source) {
                return Err(ProcessOwnerError::UnnamedCode(code.source));
            }
        }
        let mut bytes = b"evm-process-owner-v2".to_vec();
        bytes.push(u8::from(namespace.is_some()));
        if let Some(namespace) = namespace {
            bytes.extend_from_slice(namespace.coordinate.as_slice());
            encode_source(namespace.source, &mut bytes);
        }
        bytes.extend_from_slice(&(named.len() as u128).to_be_bytes());
        for owner in &named {
            bytes.extend_from_slice(owner.as_slice());
        }
        bytes.extend_from_slice(&(codes.len() as u128).to_be_bytes());
        for code in &codes {
            bytes.extend_from_slice(code.source.as_slice());
            bytes.extend_from_slice(code.hash.as_slice());
            bytes.push(fork_tag(code.fork));
        }
        let catalogue = Arc::new(Self {
            named,
            coordinates,
            namespace,
            codes,
            epoch: keccak256(bytes),
        });
        checkpoint(budget)?;
        Ok(catalogue)
    }

    /// Fixed names, including every supplied captured code account.
    pub fn named(&self) -> &BTreeSet<Address> {
        &self.named
    }

    /// Internal state coordinates include the private root template. They are
    /// not a partition of physical addresses and must not drive CALL resolution.
    pub(crate) fn coordinates(&self) -> &BTreeSet<Address> {
        &self.coordinates
    }

    /// The one supplied namespace, separate from the physical complement.
    pub fn namespace(&self) -> Option<ProcessNamespace> {
        self.namespace
    }

    /// Fixed code versions; this does not replace a frame's captured Program.
    pub fn codes(&self) -> &BTreeSet<CapturedProcessCode> {
        &self.codes
    }

    /// New names, source hashes or forks create a different epoch.
    pub fn epoch(&self) -> B256 {
        self.epoch
    }

    /// Admit an already captured source/hash/fork before making it a process
    /// body. Discovery outside this fixed set requires epoch replacement, or
    /// continued use of the old opaque complement without that precise body.
    pub fn admit_code(
        &self,
        captured: CapturedProcessCode,
        budget: &mut WorkBudget,
    ) -> Result<(), ProcessOwnerError> {
        pay(budget, (usize::BITS as usize + 1).saturating_mul(8))?;
        if !self.codes.contains(&captured) {
            return Err(ProcessOwnerError::UncapturedCode(captured));
        }
        Ok(())
    }

    /// Classify an established physical address. A missing logical address
    /// alone cannot justify calling this with any particular physical address.
    pub fn classify(&self, physical: Address) -> ProcessOwner {
        if self.named.contains(&physical) {
            ProcessOwner::Named(physical)
        } else {
            ProcessOwner::Residual
        }
    }

    /// A fully unresolved target includes the named aliases and the complement.
    /// This borrowed iterator allocates nothing and has a frozen finite bound.
    pub fn unknown_targets(&self) -> impl Iterator<Item = ProcessOwner> + '_ {
        self.namespace
            .iter()
            .map(|_| ProcessOwner::RootNamespace)
            .chain(
                self.named
                    .iter()
                    .copied()
                    .map(ProcessOwner::Named)
                    .chain(std::iter::once(ProcessOwner::Residual)),
            )
    }

    /// Validate an explicitly established class, without inventing an owner.
    pub fn key(&self, owner: ProcessOwner) -> Result<ProcessOwnerKey, ProcessOwnerError> {
        if owner == ProcessOwner::RootNamespace && self.namespace.is_none() {
            return Err(ProcessOwnerError::Shape);
        }
        if let ProcessOwner::Named(address) = owner
            && !self.named.contains(&address)
        {
            return Err(ProcessOwnerError::UnnamedOwner(address));
        }
        Ok(ProcessOwnerKey {
            epoch: self.epoch,
            owner,
        })
    }
}

fn fork_tag(fork: Fork) -> u8 {
    match fork {
        Fork::Cancun => 0,
        Fork::Prague => 1,
        Fork::Osaka => 2,
    }
}

// All variants have a fixed finite width. Root origin participates in the
// catalogue epoch even when a supplied marker selects unobserved code.
fn encode_source(source: ProcessSource, bytes: &mut Vec<u8>) {
    let (tag, account, hash, fork) = match source {
        ProcessSource::Runtime { source, hash, fork } => (0, Some(source), Some(hash), fork),
        ProcessSource::InputProgram { hash, fork } => (1, None, Some(hash), fork),
        ProcessSource::InputDelegation { target, hash, fork } => {
            (2, Some(target), Some(hash), fork)
        }
        ProcessSource::InitCode { hash, fork } => (3, None, Some(hash), fork),
        ProcessSource::AnyRuntime { fork } => (4, None, None, fork),
        ProcessSource::AnyInitCode { fork } => (5, None, None, fork),
        ProcessSource::Empty { fork } => (6, None, None, fork),
        ProcessSource::Precompile { address, fork } => (7, Some(address), None, fork),
        ProcessSource::InvalidDelegation { source, hash, fork } => {
            (8, Some(source), Some(hash), fork)
        }
    };
    bytes.push(tag);
    if let Some(account) = account {
        bytes.extend_from_slice(account.as_slice());
    }
    if let Some(hash) = hash {
        bytes.extend_from_slice(hash.as_slice());
    }
    bytes.push(fork_tag(fork));
}
