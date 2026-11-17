//! Bounded immutable publications outlive the scheduler's short task history.
//! Disk groups retain source/settings across engine upgrades; result filenames
//! include the running executable's digest. No task ID is persisted.
mod disk;
#[cfg(test)]
mod tests;

use evm_abstract_protocol::{
    DecompileArtifact, DecompileCacheWarning, DecompileCacheWarningKind, DecompileSettings,
    decompile_model::{Completion, ValidationStatus},
    decompile_sha256,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};
use thiserror::Error;

/// Configurable committed payload limits for a dedicated cache directory.
#[derive(Clone, Debug)]
pub struct Config {
    /// Directory owned by the decompiler cache, not by the analysis engine.
    pub directory: PathBuf,
    /// Maximum retained bytecode/settings groups.
    pub max_entries: usize,
    /// Maximum total committed source, settings and artifact bytes.
    pub max_bytes: u64,
}
impl Config {
    /// Default capacity for an explicitly chosen directory.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            max_entries: 128,
            max_bytes: 1024 * 1024 * 1024,
        }
    }
    /// XDG-compatible default, resolved by the executable rather than library tests.
    pub fn default_directory() -> Result<PathBuf, Error> {
        Self::directory_from(
            std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }
    fn directory_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf, Error> {
        let base = xdg
            .filter(|p| p.is_absolute())
            .or_else(|| home.filter(|p| p.is_absolute()).map(|p| p.join(".cache")))
            .ok_or(Error::Directory)?;
        Ok(base.join("evm-abstract-lab").join("decompiler"))
    }
}

/// Cache failures retain typed causes without changing an analysis result.
#[derive(Debug, Error)]
pub enum Error {
    /// No writable cache location could be selected.
    #[error("set --decompile-cache-dir or configure HOME/XDG_CACHE_HOME")]
    Directory,
    /// Zero limits cannot retain any records.
    #[error("decompiler cache limits must be positive")]
    Limits,
    /// A filesystem operation failed.
    #[error("decompiler cache I/O: {0}")]
    Io(std::io::Error),
    /// A bounded record cannot fit.
    #[error("decompiler cache capacity is too small for this record")]
    Capacity,
    /// Corrupt or mismatched records are never reused.
    #[error("decompiler cache content failed identity validation")]
    Integrity,
    /// Invalid key syntax is rejected before path construction.
    #[error("cache hashes must contain exactly 64 hexadecimal digits")]
    Hash,
    /// No stored source/settings matches the requested link.
    #[error("cached runtime bytecode or settings not found")]
    Missing,
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl Error {
    pub(crate) fn warning(&self) -> DecompileCacheWarning {
        let kind = match self {
            Self::Capacity | Self::Limits => DecompileCacheWarningKind::Capacity,
            Self::Integrity | Self::Hash | Self::Missing => DecompileCacheWarningKind::Integrity,
            Self::Io(_) | Self::Directory => DecompileCacheWarningKind::Io,
        };
        DecompileCacheWarning {
            kind,
            message: format!("{self}; the task result remains available"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub code: String,
    pub settings: String,
}
impl Key {
    pub(crate) fn new(bytes: &[u8], settings: &DecompileSettings) -> Self {
        Self {
            code: decompile_sha256(bytes),
            settings: settings.sha256(),
        }
    }
    fn name(&self) -> String {
        format!("{}-{}", self.code, self.settings)
    }
    fn parse(value: &str) -> Option<Self> {
        let (code, settings) = value.split_once('-')?;
        Some(Self {
            code: normalize_hash(code).ok()?,
            settings: normalize_hash(settings).ok()?,
        })
    }
}
pub(crate) fn normalize_hash(value: &str) -> Result<String, Error> {
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Hash);
    }
    Ok(value.to_ascii_lowercase())
}

struct MemoryEntry {
    bytes: Arc<[u8]>,
    settings: DecompileSettings,
    encoded: Option<Arc<[u8]>>,
    used: u64,
}
struct Memory {
    entries: BTreeMap<Key, MemoryEntry>,
    tick: u64,
    max_entries: usize,
    max_bytes: u64,
}
impl Memory {
    fn stamp(&mut self) -> u64 {
        self.tick = self.tick.saturating_add(1);
        self.tick
    }
    fn bytes(&self) -> u64 {
        self.entries.values().fold(0_u64, |n, e| {
            n.saturating_add(e.bytes.len() as u64)
                .saturating_add(
                    serde_json::to_vec(&e.settings)
                        .expect("settings serialize")
                        .len() as u64,
                )
                .saturating_add(e.encoded.as_ref().map_or(0, |b| b.len() as u64))
        })
    }
    fn prune(&mut self, protect: &Key) {
        while self.entries.len() > self.max_entries || self.bytes() > self.max_bytes {
            let victim = self
                .entries
                .iter()
                .filter(|(k, _)| *k != protect)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone());
            let Some(victim) = victim else { break };
            self.entries.remove(&victim);
        }
    }
}
enum Backend {
    Memory(Mutex<Memory>),
    Disk(disk::Disk),
}

/// Shared cache handle; active jobs can reuse exactly the same immutable Arc.
pub struct Cache {
    backend: Backend,
    live: Mutex<BTreeMap<Key, Weak<DecompileArtifact>>>,
}
impl Cache {
    /// An isolated bounded cache for embedding and tests. No implicit HOME writes.
    pub fn memory() -> Arc<Self> {
        Self::memory_with_limits(128, 1024 * 1024 * 1024).expect("positive defaults")
    }
    /// Explicit in-memory limits, including deterministic eviction in tests.
    pub fn memory_with_limits(max_entries: usize, max_bytes: u64) -> Result<Arc<Self>, Error> {
        if max_entries == 0 || max_bytes == 0 {
            return Err(Error::Limits);
        }
        Ok(Arc::new(Self {
            backend: Backend::Memory(Mutex::new(Memory {
                entries: BTreeMap::new(),
                tick: 0,
                max_entries,
                max_bytes,
            })),
            live: Mutex::new(BTreeMap::new()),
        }))
    }
    /// Open a persistent cache, failing explicitly if its directory is unavailable.
    pub fn persistent(config: Config) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            backend: Backend::Disk(disk::Disk::new(config)?),
            live: Mutex::new(BTreeMap::new()),
        }))
    }
    pub(crate) fn remember(
        &self,
        bytes: &[u8],
        settings: &DecompileSettings,
    ) -> Result<Key, Error> {
        let key = Key::new(bytes, settings);
        match &self.backend {
            Backend::Disk(disk) => disk.remember(&key, bytes, settings)?,
            Backend::Memory(memory) => {
                let mut memory = memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if bytes.len() as u64
                    + serde_json::to_vec(settings)
                        .expect("settings serialize")
                        .len() as u64
                    > memory.max_bytes
                {
                    return Err(Error::Capacity);
                }
                let used = memory.stamp();
                memory
                    .entries
                    .entry(key.clone())
                    .and_modify(|e| e.used = used)
                    .or_insert_with(|| MemoryEntry {
                        bytes: Arc::from(bytes),
                        settings: settings.clone(),
                        encoded: None,
                        used,
                    });
                memory.prune(&key);
            }
        }
        Ok(key)
    }
    pub(crate) fn source(
        &self,
        code: &str,
        config: Option<&str>,
    ) -> Result<(Vec<u8>, Option<DecompileSettings>), Error> {
        let code = normalize_hash(code)?;
        let config = config.map(normalize_hash).transpose()?;
        match &self.backend {
            Backend::Disk(disk) => disk.source(&code, config.as_deref()),
            Backend::Memory(memory) => {
                let mut memory = memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let key = memory
                    .entries
                    .keys()
                    .find(|k| k.code == code && config.as_ref().is_none_or(|c| *c == k.settings))
                    .cloned()
                    .ok_or(Error::Missing)?;
                let used = memory.stamp();
                let entry = memory.entries.get_mut(&key).expect("selected entry");
                entry.used = used;
                Ok((entry.bytes.to_vec(), config.map(|_| entry.settings.clone())))
            }
        }
    }
    pub(crate) fn result(&self, key: &Key) -> Result<Option<Arc<DecompileArtifact>>, Error> {
        if let Some(value) = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .and_then(Weak::upgrade)
        {
            return Ok(Some(value));
        }
        let result = match &self.backend {
            Backend::Disk(disk) => disk.result(key)?,
            Backend::Memory(memory) => {
                let encoded = {
                    let mut memory = memory
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let used = memory.stamp();
                    memory.entries.get_mut(key).and_then(|e| {
                        e.used = used;
                        e.encoded.as_ref().map(Arc::clone)
                    })
                };
                encoded.and_then(|bytes| serde_json::from_slice::<DecompileArtifact>(&bytes).ok())
            }
        };
        let Some(result) = result.filter(|a| valid(key, a)) else {
            return Ok(None);
        };
        let result = Arc::new(result);
        self.remember_live(key, &result);
        Ok(Some(result))
    }
    /// Called on the worker thread before its original artifact is moved to publication.
    pub(crate) fn publish(
        &self,
        key: &Key,
        artifact: &DecompileArtifact,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), Error> {
        if cancelled()
            || !matches!(
                artifact.completion,
                Completion::Complete | Completion::Partial
            )
        {
            return Ok(());
        }
        if !valid(key, artifact) {
            return Err(Error::Integrity);
        }
        match &self.backend {
            Backend::Disk(disk) => disk.publish(key, artifact, cancelled),
            Backend::Memory(memory) => {
                let cap = memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .max_bytes;
                let settings = artifact_settings(artifact);
                let metadata = (artifact.input.runtime_bytecode.len() as u64).saturating_add(
                    serde_json::to_vec(&settings)
                        .expect("settings serialize")
                        .len() as u64,
                );
                let mut output = LimitedWriter {
                    inner: Vec::new(),
                    remaining: cap.checked_sub(metadata).ok_or(Error::Capacity)?,
                };
                serde_json::to_writer(&mut output, artifact).map_err(|e| {
                    if e.io_error_kind() == Some(std::io::ErrorKind::FileTooLarge) {
                        Error::Capacity
                    } else {
                        Error::Integrity
                    }
                })?;
                if cancelled() {
                    return Ok(());
                }
                let mut memory = memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if cancelled() {
                    return Ok(());
                }
                let used = memory.stamp();
                let entry = memory
                    .entries
                    .entry(key.clone())
                    .or_insert_with(|| MemoryEntry {
                        bytes: Arc::from(artifact.input.runtime_bytecode.as_slice()),
                        settings,
                        encoded: None,
                        used,
                    });
                entry.encoded = Some(Arc::from(output.inner));
                entry.used = used;
                memory.prune(key);
                Ok(())
            }
        }
    }
    pub(crate) fn remember_live(&self, key: &Key, artifact: &Arc<DecompileArtifact>) {
        // Callers have admitted this result outside the coordinator. Retaining
        // its weak handle must not replay a large artifact on the owner thread.
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        live.retain(|_, weak| weak.strong_count() != 0);
        live.insert(key.clone(), Arc::downgrade(artifact));
    }
}

fn valid(key: &Key, artifact: &DecompileArtifact) -> bool {
    let settings = artifact_settings(artifact);
    matches!(
        artifact.completion,
        Completion::Complete | Completion::Partial
    ) && artifact.schema_version == evm_decompile::SCHEMA_VERSION
        && artifact.admits_schema()
        && artifact.current().is_some()
        && artifact
            .stages
            .iter()
            .all(|s| s.validation.status == ValidationStatus::Passed)
        && Key::new(&artifact.input.runtime_bytecode, &settings) == *key
}

fn artifact_settings(artifact: &DecompileArtifact) -> DecompileSettings {
    let fork = match artifact.input.fork {
        evm_decompile::Fork::Cancun => evm_abstract_protocol::Fork::Cancun,
        evm_decompile::Fork::Prague => evm_abstract_protocol::Fork::Prague,
        evm_decompile::Fork::Osaka => evm_abstract_protocol::Fork::Osaka,
    };
    DecompileSettings {
        fork,
        config: artifact.config.clone(),
    }
}

struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}
impl<W: std::io::Write> std::io::Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
