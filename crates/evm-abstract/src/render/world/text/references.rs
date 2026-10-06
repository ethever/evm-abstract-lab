//! Stable aliases always resolve to full identities in the report legends.

use super::super::observations;
use crate::{
    analysis::{FrameCode, MachineKey, WorldAnalysis},
    world::Store,
};
use alloy_primitives::{Address, B256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct References {
    pub(super) addresses: BTreeMap<Address, String>,
    pub(super) hashes: BTreeMap<B256, String>,
}

impl References {
    pub(super) fn new(analysis: &WorldAnalysis) -> Self {
        let mut addresses = BTreeSet::from([analysis.entry().address, analysis.entry().caller]);
        addresses.extend(analysis.world().accounts().keys().copied());
        let mut hashes = BTreeSet::new();
        for state in analysis.states() {
            collect_key(&state.key, &mut addresses, &mut hashes);
        }
        for frontier in analysis.frontiers() {
            if let Some(target) = &frontier.target {
                collect_key(target, &mut addresses, &mut hashes);
                hashes.insert(target.code_identity);
            }
        }
        for outcome in analysis.outcomes() {
            collect_store(&outcome.store, &mut addresses, &mut hashes);
        }
        for record in analysis.summaries() {
            if let Some(hash) = record.input.code_hash {
                hashes.insert(hash);
            }
            for result in &record.outputs {
                hashes.insert(result.store.code_identity());
            }
        }
        Self {
            addresses: addresses
                .into_iter()
                .enumerate()
                .map(|(index, address)| (address, format!("A{index}")))
                .collect(),
            hashes: hashes
                .into_iter()
                .enumerate()
                .map(|(index, hash)| (hash, format!("H{index}")))
                .collect(),
        }
    }

    pub(super) fn address(&self, address: Address) -> &str {
        self.addresses
            .get(&address)
            .expect("all displayed addresses have references")
    }

    pub(super) fn hash(&self, hash: B256) -> &str {
        self.hashes
            .get(&hash)
            .expect("all displayed hashes have references")
    }

    pub(super) fn hash_option(&self, hash: Option<B256>) -> String {
        hash.map_or_else(|| "unknown".to_owned(), |hash| self.hash(hash).to_owned())
    }
}

fn collect_key(key: &MachineKey, addresses: &mut BTreeSet<Address>, hashes: &mut BTreeSet<B256>) {
    for frame in &key.frames {
        addresses.extend([frame.code_address, frame.address, frame.caller]);
        hashes.insert(frame.code_hash);
        if let FrameCode::Precompile(address) = frame.mode {
            addresses.insert(address);
        }
    }
}

fn collect_store(store: &Store, addresses: &mut BTreeSet<Address>, hashes: &mut BTreeSet<B256>) {
    for account in observations(store) {
        addresses.insert(account.address);
        if let Some(hash) = account.code_hash {
            hashes.insert(hash);
        }
    }
    for (site, _) in store.possible_logs() {
        addresses.extend([site.address, site.code_address]);
    }
}
