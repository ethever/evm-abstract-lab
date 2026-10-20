//! Snapshot identity is independent of caller-supplied source descriptions.

use crate::domain::WordElement;
mod environment;
pub use environment::SnapshotEnvironment;

use super::EvmWorld;
use crate::{Fork, domain::EvmValue};
use alloy_primitives::{B256, Keccak256, U256, keccak256};
use serde::Serialize;
use std::io::{self, Write};
use thiserror::Error;

/// Snapshot fingerprinting must finish the complete encoding before returning
/// a digest. Native symbolic graph limits remain part of scalar serialization.
#[derive(Debug, Error)]
pub enum FingerprintError {
    /// The current analysis cancellation interrupted the encoding.
    #[error("world fingerprint encoding cancelled")]
    Cancelled,
    /// A typed scalar or graph could not be encoded within its native policy.
    #[error("world fingerprint encoding failed: {0}")]
    Encoding(serde_json::Error),
    /// A length cannot be represented by the historical u64 encoding.
    #[error("world fingerprint length exceeds u64")]
    LengthOverflow,
}

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
        /// [EIP-155](https://eips.ethereum.org/EIPS/eip-155) chain identifier, retained at full 256-bit width.
        chain_id: U256,
        /// Exact block hash; block numbers and moving tags are not identities.
        block_hash: B256,
    },
}

impl<N: WordElement> EvmWorld<N> {
    /// Source identity survives acquisition of additional cells in this snapshot.
    pub(crate) fn storage_source_token(&self) -> crate::domain::symbolic::ExprId {
        if matches!(self.identity, SnapshotIdentity::Offline { .. }) {
            return self.storage_source.clone();
        }
        let mut bytes = b"evm-abstract-storage-source-v1".to_vec();
        bytes.extend(
            serde_json::to_vec(&(self.fork, &self.identity))
                .expect("typed snapshot identity serializes"),
        );
        crate::domain::symbolic::ExprId::constant(U256::from_be_bytes(keccak256(bytes).0))
    }

    /// Block observations acquired atomically with this RPC snapshot.
    pub fn snapshot_environment(&self) -> Option<&SnapshotEnvironment> {
        self.snapshot_environment.as_ref()
    }

    pub(crate) fn apply_snapshot_environment(
        &self,
        environment: &mut super::EvmEnvironmentValues<N>,
    ) {
        if let Some(snapshot) = &self.snapshot_environment {
            snapshot.apply(environment);
            if environment.chain_id.is_none()
                && let SnapshotIdentity::Chain { chain_id, .. } = self.identity()
            {
                environment.chain_id = Some(EvmValue::<N>::constant(*chain_id));
            }
        }
    }
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
        world.storage_source = world.storage_source_token();
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
    /// Cancellation or rejected symbolic encoding returns a typed error;
    /// neither partial content nor an invented digest identifies a snapshot.
    pub fn fingerprint(&self) -> Result<B256, FingerprintError> {
        let mut hash = Keccak256::new();
        append(&mut hash, b"evm-abstract-world-v2")?;
        append(
            &mut hash,
            &[match self.fork {
                Fork::Cancun => 0,
                Fork::Prague => 1,
                Fork::Osaka => 2,
            }],
        )?;
        match &self.identity {
            SnapshotIdentity::Offline { label } => {
                append(&mut hash, &[0])?;
                append_len(&mut hash, label.len())?;
                append(&mut hash, label.as_bytes())?;
            }
            SnapshotIdentity::Chain {
                chain_id,
                block_hash,
            } => {
                append(&mut hash, &[1])?;
                append(&mut hash, &chain_id.to_be_bytes::<32>())?;
                append(&mut hash, block_hash.as_slice())?;
            }
        }
        append_len(&mut hash, self.accounts.len())?;
        for (address, account) in &self.accounts {
            checkpoint()?;
            append(&mut hash, address.as_slice())?;
            append(
                &mut hash,
                &[match account.existence {
                    super::Existence::Unknown => 0,
                    super::Existence::Present => 1,
                    super::Existence::Absent => 2,
                }],
            )?;
            match &account.code {
                super::Code::Runtime(program) => {
                    append(&mut hash, &[1])?;
                    append_len(&mut hash, program.byte_len())?;
                    append(&mut hash, program.bytes())?;
                }
                super::Code::Delegation(target) => {
                    append(&mut hash, &[1])?;
                    append_len(&mut hash, 23)?;
                    append(&mut hash, &[0xef, 0x01, 0x00])?;
                    append(&mut hash, target.as_slice())?;
                }
                super::Code::Empty => {
                    append(&mut hash, &[1])?;
                    append_len(&mut hash, 0)?;
                }
                super::Code::Unknown => append(&mut hash, &[0])?,
            }
            append_value(&mut hash, &account.balance)?;
            append_value(&mut hash, &account.nonce)?;
            append(&mut hash, &[u8::from(account.storage_unknown)])?;
            append_len(&mut hash, account.storage.len())?;
            for (slot, value) in &account.storage {
                checkpoint()?;
                append(&mut hash, &slot.to_be_bytes::<32>())?;
                append_value(&mut hash, value)?;
            }
        }
        if let Some(environment) = &self.snapshot_environment {
            // Preserve historical fingerprints for offline snapshots without
            // header observations while binding all newly acquired RPC facts.
            append(&mut hash, b"rpc-environment-v1")?;
            encode(&mut FingerprintWriter::hashing(&mut hash), environment)?;
        }
        checkpoint()?;
        Ok(hash.finalize())
    }
}

fn checkpoint() -> Result<(), FingerprintError> {
    if analysis_control::current_cancellation().is_cancelled() {
        Err(FingerprintError::Cancelled)
    } else {
        Ok(())
    }
}

fn append(hash: &mut Keccak256, bytes: &[u8]) -> Result<(), FingerprintError> {
    checkpoint()?;
    for chunk in bytes.chunks(64 * 1024) {
        checkpoint()?;
        hash.update(chunk);
    }
    Ok(())
}

fn append_len(hash: &mut Keccak256, length: usize) -> Result<(), FingerprintError> {
    let length = u64::try_from(length).map_err(|_| FingerprintError::LengthOverflow)?;
    append(hash, &length.to_be_bytes())
}

/// Scalars have a length prefix in the historical encoding. Two bounded passes
/// preserve those bytes without constructing a scalar or whole-world Vec.
fn append_value<N: WordElement>(
    hash: &mut Keccak256,
    value: &EvmValue<N>,
) -> Result<(), FingerprintError> {
    let mut count = FingerprintWriter::counting();
    encode(&mut count, value)?;
    append(hash, &count.length.to_be_bytes())?;
    encode(&mut FingerprintWriter::hashing(hash), value)
}

fn encode(
    writer: &mut FingerprintWriter<'_>,
    value: &impl Serialize,
) -> Result<(), FingerprintError> {
    checkpoint()?;
    serde_json::to_writer(&mut *writer, value).map_err(|error| {
        if analysis_control::current_cancellation().is_cancelled() {
            FingerprintError::Cancelled
        } else if writer.overflow {
            FingerprintError::LengthOverflow
        } else {
            FingerprintError::Encoding(error)
        }
    })
}

struct FingerprintWriter<'a> {
    hash: Option<&'a mut Keccak256>,
    length: u64,
    overflow: bool,
}

impl FingerprintWriter<'_> {
    fn counting() -> Self {
        Self {
            hash: None,
            length: 0,
            overflow: false,
        }
    }
    fn hashing(hash: &mut Keccak256) -> FingerprintWriter<'_> {
        FingerprintWriter {
            hash: Some(hash),
            length: 0,
            overflow: false,
        }
    }
}

impl Write for FingerprintWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if analysis_control::current_cancellation().is_cancelled() {
            // Write::write_all retries Interrupted forever. Cancellation must
            // leave serde's write immediately; encode maps it to Cancelled.
            return Err(io::ErrorKind::Other.into());
        }
        let Some(length) = u64::try_from(bytes.len())
            .ok()
            .and_then(|length| self.length.checked_add(length))
        else {
            self.overflow = true;
            return Err(io::ErrorKind::InvalidData.into());
        };
        if let Some(hash) = &mut self.hash {
            append(hash, bytes).map_err(|_| io::Error::from(io::ErrorKind::Other))?;
        }
        self.length = length;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if analysis_control::current_cancellation().is_cancelled() {
            Err(io::ErrorKind::Other.into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    #[test]
    fn cancellation_does_not_enter_write_all_interrupted_retry() {
        let cancellation = analysis_control::Cancellation::new();
        cancellation.cancel();
        analysis_control::with_cancellation(&cancellation, || {
            let mut writer = FingerprintWriter::counting();
            let error = serde_json::to_writer(&mut writer, &U256::ONE).unwrap_err();
            assert!(error.is_io());
            assert_eq!(writer.length, 0);
            assert!(matches!(
                encode(&mut writer, &U256::ONE),
                Err(FingerprintError::Cancelled)
            ));
        });
    }

    #[test]
    fn encoding_length_overflow_is_typed_before_hash_mutation() {
        let mut writer = FingerprintWriter::counting();
        writer.length = u64::MAX;
        assert!(matches!(
            encode(&mut writer, &true),
            Err(FingerprintError::LengthOverflow)
        ));
        assert_eq!(writer.length, u64::MAX);
    }
}
