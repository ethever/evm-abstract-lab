use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "evm-decompiler-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> Config {
        Config {
            directory: self.0.clone(),
            max_entries: 8,
            max_bytes: 2 * 1024 * 1024,
        }
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn artifact(code: &[u8]) -> DecompileArtifact {
    evm_decompile::decompile(
        &evm_decompile::Input::new(code.to_vec(), evm_decompile::Fork::Osaka),
        &DecompileSettings::default().config,
    )
}

#[test]
fn default_directory_ignores_empty_or_relative_environment_locations() {
    let home = std::env::temp_dir().join("cache-home");
    let xdg = std::env::temp_dir().join("cache-xdg");
    let expected = home.join(".cache/evm-abstract-lab/decompiler");
    for value in [None, Some(PathBuf::new()), Some(PathBuf::from("relative"))] {
        assert_eq!(
            Config::directory_from(value, Some(home.clone())).unwrap(),
            expected
        );
    }
    assert_eq!(
        Config::directory_from(Some(xdg.clone()), None).unwrap(),
        xdg.join("evm-abstract-lab/decompiler")
    );
    assert!(matches!(
        Config::directory_from(None, Some(PathBuf::new())),
        Err(Error::Directory)
    ));
}

#[test]
fn disk_restart_retains_sources_settings_and_results_without_job_ids() {
    let directory = Directory::new();
    let settings = DecompileSettings::default();
    let value = artifact(&[0]);
    let cache = Cache::persistent(directory.config()).unwrap();
    let key = cache.remember(&[0], &settings).unwrap();
    cache.publish(&key, &value, || false).unwrap();
    drop(cache);
    let cache = Cache::persistent(directory.config()).unwrap();
    let (bytes, saved) = cache.source(&key.code, Some(&key.settings)).unwrap();
    assert_eq!(bytes, [0]);
    assert_eq!(saved, Some(settings));
    let result = cache.result(&key).unwrap().unwrap();
    assert_eq!(*result, value);
    assert!(Arc::ptr_eq(&result, &cache.result(&key).unwrap().unwrap()));
}

#[test]
fn partial_is_preserved_but_cancellation_limits_and_wrong_inputs_are_not_results() {
    let directory = Directory::new();
    for cache in [
        Cache::memory(),
        Cache::persistent(directory.config()).unwrap(),
    ] {
        let settings = DecompileSettings::default();
        let mut value = artifact(&[0x5f, 0x35, 0x56]);
        let key = cache
            .remember(&value.input.runtime_bytecode, &settings)
            .unwrap();
        assert_eq!(value.completion, Completion::Partial);
        cache.publish(&key, &value, || false).unwrap();
        assert_eq!(
            cache.result(&key).unwrap().unwrap().completion,
            Completion::Partial
        );
        let next = cache.remember(&[0], &settings).unwrap();
        value = artifact(&[0]);
        cache.publish(&next, &value, || true).unwrap();
        assert!(cache.result(&next).unwrap().is_none());
        for completion in [Completion::Cancelled, Completion::Limited] {
            value.completion = completion;
            cache.publish(&next, &value, || false).unwrap();
            assert!(cache.result(&next).unwrap().is_none());
        }
        value.completion = Completion::Complete;
        value.input.runtime_bytecode = vec![1];
        assert!(matches!(
            cache.publish(&next, &value, || false),
            Err(Error::Integrity)
        ));
    }
}

#[test]
fn entries_are_bounded_and_bare_hash_does_not_restore_an_arbitrary_policy() {
    let directory = Directory::new();
    let mut config = directory.config();
    config.max_entries = 1;
    for cache in [
        Cache::memory_with_limits(1, 2 * 1024 * 1024).unwrap(),
        Cache::persistent(config).unwrap(),
    ] {
        let mut custom = DecompileSettings::default();
        custom.config.limits.max_work = 123;
        let key = cache.remember(&[0], &custom).unwrap();
        let (_, settings) = cache.source(&key.code, None).unwrap();
        assert_eq!(settings, None);
        assert_eq!(
            cache.source(&key.code, Some(&key.settings)).unwrap().1,
            Some(custom)
        );
        cache
            .remember(&[0x5b, 0], &DecompileSettings::default())
            .unwrap();
        assert!(matches!(cache.source(&key.code, None), Err(Error::Missing)));
        assert!(matches!(cache.source("../bad", None), Err(Error::Hash)));
    }
}

#[test]
fn corrupt_and_other_engine_records_recompute_without_losing_the_saved_source() {
    let directory = Directory::new();
    let settings = DecompileSettings::default();
    let value = artifact(&[0]);
    let cache = Cache::persistent(directory.config()).unwrap();
    let key = cache.remember(&[0], &settings).unwrap();
    cache.publish(&key, &value, || false).unwrap();
    let group = directory.0.join("v1").join(key.name());
    let file = fs::read_dir(&group)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("artifact-")
        })
        .unwrap();
    let mut bytes = fs::read(&file).unwrap();
    let at = bytes.windows(7).position(|w| w == b"\"work\":").unwrap() + 7;
    bytes[at] = if bytes[at] == b'9' { b'8' } else { b'9' };
    fs::write(&file, &bytes).unwrap();
    assert!(cache.result(&key).unwrap().is_none());
    cache.publish(&key, &value, || false).unwrap();
    fs::rename(
        &file,
        group.join(format!("artifact-{}.json", "0".repeat(64))),
    )
    .unwrap();
    assert!(cache.result(&key).unwrap().is_none());
    assert_eq!(cache.source(&key.code, Some(&key.settings)).unwrap().0, [0]);
}

#[test]
fn oversized_replacement_keeps_its_previous_result_and_other_good_groups() {
    let directory = Directory::new();
    let settings = DecompileSettings::default();
    let first = artifact(&[0]);
    let second = artifact(&[0x5b, 0]);
    let cap = serde_json::to_vec(&first).unwrap().len() as u64
        + serde_json::to_vec(&second).unwrap().len() as u64
        + 2 * serde_json::to_vec(&settings).unwrap().len() as u64
        + 1024;
    let mut config = directory.config();
    config.max_bytes = cap;
    for cache in [
        Cache::memory_with_limits(8, cap).unwrap(),
        Cache::persistent(config).unwrap(),
    ] {
        let a = cache
            .remember(&first.input.runtime_bytecode, &settings)
            .unwrap();
        let b = cache
            .remember(&second.input.runtime_bytecode, &settings)
            .unwrap();
        cache.publish(&a, &first, || false).unwrap();
        cache.publish(&b, &second, || false).unwrap();
        let mut oversized = first.clone();
        oversized.diagnostics.push(evm_decompile::Diagnostic {
            severity: evm_decompile::Severity::Info,
            code: evm_decompile::DiagnosticCode::OptimizationRefused,
            stage: None,
            entity: None,
            source: None,
            message: "x".repeat(cap as usize),
        });
        assert!(matches!(
            cache.publish(&a, &oversized, || false),
            Err(Error::Capacity)
        ));
        assert_eq!(*cache.result(&a).unwrap().unwrap(), first);
        assert_eq!(*cache.result(&b).unwrap().unwrap(), second);
    }
}

#[test]
fn restart_removes_interrupted_temporary_files_before_enforcing_quota() {
    let directory = Directory::new();
    let value = artifact(&[0]);
    let settings = DecompileSettings::default();
    let mut config = directory.config();
    config.max_bytes = serde_json::to_vec(&value).unwrap().len() as u64
        + serde_json::to_vec(&settings).unwrap().len() as u64
        + 256;
    let cache = Cache::persistent(config.clone()).unwrap();
    let key = cache.remember(&[0], &settings).unwrap();
    cache.publish(&key, &value, || false).unwrap();
    drop(cache);
    let orphan = directory
        .0
        .join("v1")
        .join(key.name())
        .join(format!("artifact-{}.tmp-9999-1", "0".repeat(64)));
    std::fs::File::create(&orphan)
        .unwrap()
        .set_len(config.max_bytes * 2)
        .unwrap();
    let cache = Cache::persistent(config).unwrap();
    assert!(!orphan.exists());
    assert_eq!(*cache.result(&key).unwrap().unwrap(), value);
}

#[test]
fn completing_an_evicted_inflight_group_reintroduces_its_source_and_result() {
    let directory = Directory::new();
    let mut config = directory.config();
    config.max_entries = 1;
    let settings = DecompileSettings::default();
    let value = artifact(&[0]);
    for cache in [
        Cache::memory_with_limits(1, 2 * 1024 * 1024).unwrap(),
        Cache::persistent(config).unwrap(),
    ] {
        let key = cache.remember(&[0], &settings).unwrap();
        cache.remember(&[0x5b, 0], &settings).unwrap();
        assert!(matches!(cache.source(&key.code, None), Err(Error::Missing)));
        cache.publish(&key, &value, || false).unwrap();
        assert_eq!(cache.source(&key.code, None).unwrap().0, [0]);
        assert_eq!(*cache.result(&key).unwrap().unwrap(), value);
    }
}
