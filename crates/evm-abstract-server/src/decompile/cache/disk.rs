//! One filesystem lock serializes atomic group publication and quota eviction.
//! Temporary records are individually bounded; only committed group bytes count
//! toward the retained quota. A group always owns its source and exact settings.
use super::{Config, DecompileArtifact, DecompileSettings, Error, Key, LimitedWriter};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct Disk {
    config: Config,
    root: PathBuf,
    engine: String,
}
static TEMPORARY: AtomicU64 = AtomicU64::new(0);
static ENGINE: OnceLock<Result<String, std::io::ErrorKind>> = OnceLock::new();

fn engine() -> Result<String, Error> {
    ENGINE
        .get_or_init(|| {
            let mut file =
                File::open(std::env::current_exe().map_err(|e| e.kind())?).map_err(|e| e.kind())?;
            let mut hash = Sha256::new();
            let mut block = [0_u8; 64 * 1024];
            loop {
                let n = file.read(&mut block).map_err(|e| e.kind())?;
                if n == 0 {
                    break;
                }
                hash.update(&block[..n]);
            }
            hash.update(format!(";cache=1;schema={}", evm_decompile::SCHEMA_VERSION));
            Ok(alloy_primitives::hex::encode(hash.finalize()))
        })
        .clone()
        .map_err(|kind| Error::Io(kind.into()))
}

impl Disk {
    pub(super) fn new(config: Config) -> Result<Self, Error> {
        if config.max_entries == 0 || config.max_bytes == 0 {
            return Err(Error::Limits);
        }
        let root = config.directory.join("v1");
        fs::create_dir_all(&root)?;
        let this = Self {
            config,
            root,
            engine: engine()?,
        };
        let _lock = this.lock()?;
        this.prune(None)?;
        Ok(this)
    }
    fn lock(&self) -> Result<File, Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".lock"))?;
        file.lock()?;
        // Every writer owns this same lock until rename. A matching temporary
        // file visible now therefore belongs to an interrupted publication.
        self.cleanup_temporary()?;
        Ok(file)
    }
    fn group(&self, key: &Key) -> PathBuf {
        self.root.join(key.name())
    }
    fn cleanup_temporary(&self) -> Result<(), Error> {
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir()
                || entry.file_name().to_str().and_then(Key::parse).is_none()
            {
                continue;
            }
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                if child.file_type()?.is_file()
                    && child.file_name().to_str().is_some_and(temporary_name)
                {
                    fs::remove_file(child.path())?;
                }
            }
        }
        Ok(())
    }
    fn artifact(&self, key: &Key) -> PathBuf {
        self.group(key)
            .join(format!("artifact-{}.json", self.engine))
    }
    fn touch(&self, key: &Key) -> Result<(), Error> {
        File::create(self.group(key).join(".stamp"))?.sync_all()?;
        Ok(())
    }
    fn metadata(&self, key: &Key, bytes: &[u8], settings: &DecompileSettings) -> Result<(), Error> {
        let encoded = serde_json::to_vec(settings).map_err(|_| Error::Integrity)?;
        if bytes.len() as u64 + encoded.len() as u64 > self.config.max_bytes {
            return Err(Error::Capacity);
        }
        let dir = self.group(key);
        fs::create_dir_all(&dir)?;
        self.atomic(&dir.join("source.bin"), |file| file.write_all(bytes))?;
        self.atomic(&dir.join("settings.json"), |file| file.write_all(&encoded))?;
        self.touch(key)
    }
    pub(super) fn remember(
        &self,
        key: &Key,
        bytes: &[u8],
        settings: &DecompileSettings,
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        self.metadata(key, bytes, settings)?;
        self.prune(Some(key))
    }
    pub(super) fn source(
        &self,
        code: &str,
        config: Option<&str>,
    ) -> Result<(Vec<u8>, Option<DecompileSettings>), Error> {
        let _lock = self.lock()?;
        let key = if let Some(config) = config {
            Key {
                code: code.into(),
                settings: config.into(),
            }
        } else {
            self.groups()?
                .into_iter()
                .find(|g| g.key.code == code)
                .map(|g| g.key)
                .ok_or(Error::Missing)?
        };
        let bytes = self.read(
            &self.group(&key).join("source.bin"),
            evm_abstract_protocol::MAX_REQUEST_BYTES as u64,
        )?;
        if evm_abstract_protocol::decompile_sha256(&bytes) != code {
            return Err(Error::Integrity);
        }
        let settings = if config.is_some() {
            let text = self.read(
                &self.group(&key).join("settings.json"),
                evm_abstract_protocol::MAX_REQUEST_BYTES as u64,
            )?;
            let settings: DecompileSettings =
                serde_json::from_slice(&text).map_err(|_| Error::Integrity)?;
            if settings.sha256() != key.settings {
                return Err(Error::Integrity);
            }
            Some(settings)
        } else {
            None
        };
        self.touch(&key)?;
        Ok((bytes, settings))
    }
    pub(super) fn result(&self, key: &Key) -> Result<Option<DecompileArtifact>, Error> {
        let _lock = self.lock()?;
        let path = self.artifact(key);
        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() > self.config.max_bytes {
            return Ok(None);
        }
        let mut digest = [0_u8; 65];
        if file.read_exact(&mut digest).is_err() || digest[64] != b'\n' {
            return Ok(None);
        }
        let mut reader = BufReader::new(HashRead {
            inner: file,
            hash: Sha256::new(),
        });
        let result = serde_json::from_reader(&mut reader).ok();
        let actual = alloy_primitives::hex::encode(reader.into_inner().hash.finalize());
        if actual.as_bytes() != &digest[..64] {
            return Ok(None);
        }
        self.touch(key)?;
        Ok(result)
    }
    pub(super) fn publish(
        &self,
        key: &Key,
        artifact: &DecompileArtifact,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        if cancelled() {
            return Ok(());
        }
        let fork = match artifact.input.fork {
            evm_decompile::Fork::Cancun => evm_abstract_protocol::Fork::Cancun,
            evm_decompile::Fork::Prague => evm_abstract_protocol::Fork::Prague,
            evm_decompile::Fork::Osaka => evm_abstract_protocol::Fork::Osaka,
        };
        let settings = DecompileSettings {
            fork,
            config: artifact.config.clone(),
        };
        let metadata = artifact.input.runtime_bytecode.len() as u64
            + serde_json::to_vec(&settings)
                .map_err(|_| Error::Integrity)?
                .len() as u64;
        let limit = self
            .config
            .max_bytes
            .checked_sub(metadata)
            .ok_or(Error::Capacity)?;
        self.metadata(key, &artifact.input.runtime_bytecode, &settings)?;
        let path = self.artifact(key);
        let publication = self.atomic_limited(&path, limit, |file| {
            file.write_all(&[0_u8; 65])?;
            let mut writer = BufWriter::new(HashWrite {
                inner: &mut *file,
                hash: Sha256::new(),
            });
            serde_json::to_writer(&mut writer, artifact).map_err(|e| {
                std::io::Error::new(
                    e.io_error_kind().unwrap_or(std::io::ErrorKind::InvalidData),
                    "cache artifact encoding failed",
                )
            })?;
            writer.flush()?;
            let digest = alloy_primitives::hex::encode(
                writer
                    .into_inner()
                    .map_err(|e| e.into_error())?
                    .hash
                    .finalize(),
            );
            file.inner.seek(SeekFrom::Start(0))?;
            file.inner.write_all(digest.as_bytes())?;
            file.inner.write_all(b"\n")?;
            if cancelled() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cache publication cancelled",
                ));
            }
            Ok(())
        });
        if let Err(error) = publication {
            return if cancelled() { Ok(()) } else { Err(error) };
        }
        for entry in fs::read_dir(self.group(key))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("artifact-")
                && name.ends_with(".json")
                && entry.path() != path
                && entry.file_type()?.is_file()
            {
                fs::remove_file(entry.path())?;
            }
        }
        self.touch(key)?;
        self.prune(Some(key))
    }
    fn read(&self, path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Missing
            } else {
                Error::Io(e)
            }
        })?;
        if file.metadata()?.len() > limit {
            return Err(Error::Capacity);
        }
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(Error::Capacity);
        }
        Ok(bytes)
    }
    fn atomic(
        &self,
        path: &Path,
        write: impl FnOnce(&mut LimitedWriter<File>) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        self.atomic_limited(path, self.config.max_bytes, write)
    }
    fn atomic_limited(
        &self,
        path: &Path,
        limit: u64,
        write: impl FnOnce(&mut LimitedWriter<File>) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        let temporary = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut output = LimitedWriter {
                inner: file,
                remaining: limit,
            };
            write(&mut output)?;
            output.flush()?;
            output.inner.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|e| {
            if e.kind() == std::io::ErrorKind::FileTooLarge {
                Error::Capacity
            } else {
                Error::Io(e)
            }
        })
    }
    fn groups(&self) -> Result<Vec<Group>, Error> {
        let mut groups = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(key) = entry.file_name().to_str().and_then(Key::parse) else {
                continue;
            };
            let mut size = 0_u64;
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                if child.file_type()?.is_file() {
                    size = size.saturating_add(child.metadata()?.len());
                }
            }
            let used = fs::metadata(entry.path().join(".stamp"))
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            groups.push(Group { key, size, used });
        }
        groups.sort_by_key(|g| g.used);
        Ok(groups)
    }
    fn prune(&self, protect: Option<&Key>) -> Result<(), Error> {
        let groups = self.groups()?;
        let mut count = groups.len();
        let mut bytes = groups.iter().fold(0_u64, |n, g| n.saturating_add(g.size));
        for group in groups.iter().filter(|g| Some(&g.key) != protect) {
            if count <= self.config.max_entries && bytes <= self.config.max_bytes {
                break;
            }
            fs::remove_dir_all(self.group(&group.key))?;
            count -= 1;
            bytes = bytes.saturating_sub(group.size);
        }
        if count > self.config.max_entries || bytes > self.config.max_bytes {
            if let Some(key) = protect {
                let artifact = self.artifact(key);
                let _ = fs::remove_file(artifact);
            }
            return Err(Error::Capacity);
        }
        Ok(())
    }
}
struct Group {
    key: Key,
    size: u64,
    used: SystemTime,
}

fn temporary_name(name: &str) -> bool {
    let Some((prefix, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    if !matches!(prefix, "source" | "settings")
        && !prefix
            .strip_prefix("artifact-")
            .is_some_and(|s| super::normalize_hash(s).is_ok())
    {
        return false;
    }
    let Some((pid, sequence)) = suffix.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && !sequence.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && sequence.bytes().all(|b| b.is_ascii_digit())
}

struct HashRead<R> {
    inner: R,
    hash: Sha256,
}
impl<R: Read> Read for HashRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
}
struct HashWrite<W> {
    inner: W,
    hash: Sha256,
}
impl<W: Write> Write for HashWrite<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
