//! Snapshot identity is independent of caller-supplied source descriptions.

use super::World;
use crate::{Fork, domain::Value};
use alloy_primitives::{B256, U256, keccak256};
use serde::Serialize;

/// A fixed chain/block identity or an explicitly unanchored offline fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SnapshotIdentity {
    /// Synthetic facts; the label makes no assertion about a live chain.
    Offline {
        /// Stable fixture label supplied by its author.
        label: String,
    },
    /// Observations declared to belong to this exact chain and block hash.
    Chain {
        /// EIP-155 chain identifier, retained at full 256-bit width.
        chain_id: U256,
        /// Exact block hash; block numbers and moving tags are not identities.
        block_hash: B256,
    },
}

impl World {
    /// Start an explicitly synthetic snapshot with a separate source description.
    pub fn offline(fork: Fork, label: impl Into<String>, provenance: impl Into<String>) -> Self {
        let mut world = Self::new(fork, provenance);
        world.identity = SnapshotIdentity::Offline {
            label: label.into(),
        };
        world
    }

    /// Start an anchored snapshot; insertion separately validates each fact.
    pub fn anchored(
        fork: Fork,
        chain_id: U256,
        block_hash: B256,
        provenance: impl Into<String>,
    ) -> Self {
        let mut world = Self::new(fork, provenance);
        world.identity = SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        };
        world
    }

    /// Typed identity fixed before any execution begins.
    pub fn identity(&self) -> &SnapshotIdentity {
        &self.identity
    }

    /// Deterministic identity of fork, snapshot and all initial observations.
    ///
    /// The encoding uses fixed-width words and lengths, ordered addresses and
    /// slots, and explicit unknown markers. A source description does not
    /// alter an anchored snapshot's fingerprint.
    pub fn fingerprint(&self) -> B256 {
        let mut bytes = b"evm-abstract-world-v2".to_vec();
        bytes.push(match self.fork {
            Fork::Cancun => 0,
            Fork::Prague => 1,
            Fork::Osaka => 2,
        });
        match &self.identity {
            SnapshotIdentity::Offline { label } => {
                bytes.push(0);
                append_len(&mut bytes, label.len());
                bytes.extend_from_slice(label.as_bytes());
            }
            SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            } => {
                bytes.push(1);
                bytes.extend_from_slice(&chain_id.to_be_bytes::<32>());
                bytes.extend_from_slice(block_hash.as_slice());
            }
        }
        append_len(&mut bytes, self.accounts.len());
        for (address, account) in &self.accounts {
            bytes.extend_from_slice(address.as_slice());
            bytes.push(match account.existence {
                super::Existence::Unknown => 0,
                super::Existence::Present => 1,
                super::Existence::Absent => 2,
            });
            match super::raw_code(&account.code) {
                Some(code) => {
                    bytes.push(1);
                    append_len(&mut bytes, code.len());
                    bytes.extend(code);
                }
                None => bytes.push(0),
            }
            append_value(&mut bytes, &account.balance);
            append_value(&mut bytes, &account.nonce);
            bytes.push(u8::from(account.storage_unknown));
            append_len(&mut bytes, account.storage.len());
            for (slot, value) in &account.storage {
                bytes.extend_from_slice(&slot.to_be_bytes::<32>());
                append_value(&mut bytes, value);
            }
        }
        keccak256(bytes)
    }
}

fn append_len(bytes: &mut Vec<u8>, length: usize) {
    bytes.extend_from_slice(&(length as u64).to_be_bytes());
}

fn append_value(bytes: &mut Vec<u8>, value: &Value) {
    let encoded = serde_json::to_vec(value).expect("validated scalar facts serialize infallibly");
    append_len(bytes, encoded.len());
    bytes.extend_from_slice(&encoded);
}
