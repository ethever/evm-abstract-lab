//! Content epochs bound names and captured code without inspecting Store keys.

use super::{ProcessOwner, ProcessOwnerError};
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

/// Class key only; code/source/static/caller roles still belong to frame shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessOwnerKey {
    /// Digest of the complete frozen names and code vocabulary.
    pub epoch: B256,
    /// Physical owner class, independent of logical ADDRESS uncertainty.
    pub owner: ProcessOwner,
}

/// Immutable process vocabulary. Discoveries require a new content epoch.
#[derive(Debug)]
pub struct OwnerCatalogue {
    named: BTreeSet<Address>,
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
        pay(budget, 8)?;
        if names.len() > limits.names || codes.len() > limits.codes {
            return Err(ProcessOwnerError::Capacity);
        }
        // Fixed key widths and a full tree-height bound cover collection,
        // membership, digest bytes and allocation before any of them run.
        let entries = names.len().saturating_add(codes.len());
        pay(
            budget,
            entries
                .saturating_add(1)
                .saturating_mul((usize::BITS as usize + 1).saturating_mul(128)),
        )?;
        let named: BTreeSet<_> = names.iter().copied().collect();
        let codes: BTreeSet<_> = codes.iter().copied().collect();
        for code in &codes {
            if !named.contains(&code.source) {
                return Err(ProcessOwnerError::UnnamedCode(code.source));
            }
        }
        let mut bytes = b"evm-process-owner-v1".to_vec();
        bytes.extend_from_slice(&(named.len() as u128).to_be_bytes());
        for owner in &named {
            bytes.extend_from_slice(owner.as_slice());
        }
        bytes.extend_from_slice(&(codes.len() as u128).to_be_bytes());
        for code in &codes {
            bytes.extend_from_slice(code.source.as_slice());
            bytes.extend_from_slice(code.hash.as_slice());
            bytes.push(match code.fork {
                Fork::Cancun => 0,
                Fork::Prague => 1,
                Fork::Osaka => 2,
            });
        }
        let catalogue = Arc::new(Self {
            named,
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
        self.named
            .iter()
            .copied()
            .map(ProcessOwner::Named)
            .chain(std::iter::once(ProcessOwner::Residual))
    }

    /// Validate an explicitly established class, without inventing an owner.
    pub fn key(&self, owner: ProcessOwner) -> Result<ProcessOwnerKey, ProcessOwnerError> {
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
