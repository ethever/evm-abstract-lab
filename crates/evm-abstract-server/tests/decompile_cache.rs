//! Cached openings retain normal task IDs/publications without repeating recovery.
use evm_abstract::analysis::control::Control;
use evm_abstract_protocol::{
    self as api, DecompileHashSource, DecompileOpenRequest, DecompileSettings,
    DecompileSourceInput, JobState,
};
use evm_abstract_server::{
    decompile::cache::{Cache, Config as CacheConfig},
    jobs::{Config, Executor, Pool},
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
struct Counting(Arc<AtomicUsize>);
impl Executor for Counting {
    fn execute(
        &self,
        _: api::AnalyzeRequest,
        _: &Control,
    ) -> Result<api::AnalysisReport, api::ApiError> {
        panic!("decompiler open must not run world analysis")
    }
    fn decompile(
        &self,
        request: api::DecompileRequest,
        control: &Control,
        observer: &evm_abstract_server::decompile::Observer,
    ) -> Result<api::DecompileArtifact, api::ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let fork = match request.fork {
            api::Fork::Cancun => evm_decompile::Fork::Cancun,
            api::Fork::Prague => evm_decompile::Fork::Prague,
            api::Fork::Osaka => evm_decompile::Fork::Osaka,
        };
        Ok(evm_decompile::decompile_with_progress(
            &evm_decompile::Input::new(evm_bytecode::parse_hex(&request.bytecode).unwrap(), fork),
            &DecompileSettings::from_request(&request).config,
            control.cancellation(),
            |p| observer.observe(p),
        ))
    }
}
fn pool(cache: Arc<Cache>, count: Arc<AtomicUsize>) -> Pool {
    Pool::with_executor_and_cache(
        Config {
            workers: 1,
            queue_capacity: 2,
            retained_jobs: 1,
        },
        Counting(count),
        cache,
    )
    .unwrap()
}
fn code(text: &str, settings: Option<DecompileSettings>) -> DecompileOpenRequest {
    DecompileOpenRequest {
        source: DecompileSourceInput::Bytecode(text.into()),
        settings,
    }
}
fn link(source: &api::DecompileSource) -> DecompileOpenRequest {
    DecompileOpenRequest {
        source: DecompileSourceInput::Sha256(DecompileHashSource {
            sha256: source.sha256.clone(),
            config_sha256: Some(source.config_sha256.clone()),
        }),
        settings: None,
    }
}
async fn terminal(pool: &Pool, id: api::JobId) -> api::DecompileJobSnapshot {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = pool.decompile_status(id).unwrap();
        if matches!(
            snapshot.state,
            JobState::Completed | JobState::Failed(_) | JobState::Cancelled
        ) {
            return snapshot;
        }
        assert!(Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-cache-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn cache(&self) -> Arc<Cache> {
        Cache::persistent(CacheConfig::new(self.0.clone())).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn cache_hit_has_a_fresh_publication_and_reuses_the_same_immutable_arc() {
    let count = Arc::new(AtomicUsize::new(0));
    let pool = pool(Cache::memory(), Arc::clone(&count));
    let first = pool.open_decompile(code("00", None)).await.unwrap();
    assert!(!first.source.cache_hit);
    terminal(&pool, first.snapshot.id).await;
    let original = pool.decompile_result(first.snapshot.id).unwrap();
    let hit = pool.open_decompile(code("0x00", None)).await.unwrap();
    assert!(hit.source.cache_hit);
    assert_eq!(hit.snapshot.state, JobState::Completed);
    assert_ne!(hit.snapshot.id, first.snapshot.id);
    let restored = pool.decompile_result(hit.snapshot.id).unwrap();
    assert_eq!(restored.id, hit.snapshot.id);
    assert!(Arc::ptr_eq(&original.artifact, &restored.artifact));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        pool.decompile_status(first.snapshot.id).unwrap_err().code,
        api::ApiErrorCode::TaskNotFound
    );
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn completed_result_survives_no_result_fetch_task_eviction_and_store_restart() {
    let directory = Directory::new();
    let count = Arc::new(AtomicUsize::new(0));
    let first = {
        let pool = pool(directory.cache(), Arc::clone(&count));
        let first = pool.open_decompile(code("00", None)).await.unwrap();
        terminal(&pool, first.snapshot.id).await;
        let other = pool.open_decompile(code("5b00", None)).await.unwrap();
        terminal(&pool, other.snapshot.id).await;
        assert_eq!(
            pool.decompile_result(first.snapshot.id).unwrap_err().code,
            api::ApiErrorCode::TaskNotFound
        );
        pool.shutdown().unwrap();
        first
    };
    let pool = pool(directory.cache(), Arc::clone(&count));
    let hit = pool.open_decompile(link(&first.source)).await.unwrap();
    assert!(hit.source.cache_hit);
    assert_ne!(hit.snapshot.id, first.snapshot.id);
    assert_eq!(
        pool.decompile_result(hit.snapshot.id)
            .unwrap()
            .artifact
            .input
            .runtime_bytecode,
        [0]
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn full_settings_are_distinct_and_bare_hash_chooses_defaults() {
    let count = Arc::new(AtomicUsize::new(0));
    let pool = pool(Cache::memory(), Arc::clone(&count));
    let mut custom = DecompileSettings {
        fork: api::Fork::Cancun,
        ..DecompileSettings::default()
    };
    custom.config.readability.profile = api::decompile_model::ReadabilityProfile::Linear;
    let first = pool
        .open_decompile(code("00", Some(custom.clone())))
        .await
        .unwrap();
    terminal(&pool, first.snapshot.id).await;
    let bare = pool
        .open_decompile(DecompileOpenRequest {
            source: DecompileSourceInput::Sha256(DecompileHashSource {
                sha256: first.source.sha256.clone(),
                config_sha256: None,
            }),
            settings: None,
        })
        .await
        .unwrap();
    assert!(!bare.source.cache_hit);
    assert_eq!(
        DecompileSettings::from_request(&bare.request),
        DecompileSettings::default()
    );
    terminal(&pool, bare.snapshot.id).await;
    let saved = pool.open_decompile(link(&first.source)).await.unwrap();
    assert!(saved.source.cache_hit);
    assert_eq!(DecompileSettings::from_request(&saved.request), custom);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let mut ambiguous = link(&first.source);
    ambiguous.settings = Some(DecompileSettings::default());
    assert_eq!(
        pool.open_decompile(ambiguous).await.unwrap_err().code,
        api::ApiErrorCode::InvalidRequest
    );
    let unknown = DecompileOpenRequest {
        source: DecompileSourceInput::Sha256(DecompileHashSource {
            sha256: "f".repeat(64),
            config_sha256: None,
        }),
        settings: None,
    };
    assert_eq!(
        pool.open_decompile(unknown).await.unwrap_err().code,
        api::ApiErrorCode::NotFound
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    pool.shutdown().unwrap();
}

#[tokio::test]
async fn limited_attempts_are_not_reused_and_capacity_warnings_keep_valid_results() {
    let count = Arc::new(AtomicUsize::new(0));
    let pool = pool(
        Cache::memory_with_limits(8, 2048).unwrap(),
        Arc::clone(&count),
    );
    let first = pool.open_decompile(code("00", None)).await.unwrap();
    let complete = terminal(&pool, first.snapshot.id).await;
    assert_eq!(complete.state, JobState::Completed);
    assert_eq!(
        complete.cache_warning.unwrap().kind,
        api::DecompileCacheWarningKind::Capacity
    );
    assert_eq!(
        pool.decompile_result(first.snapshot.id)
            .unwrap()
            .artifact
            .completion,
        api::decompile_model::Completion::Complete
    );
    let mut settings = DecompileSettings::default();
    settings.config.limits.max_work = 0;
    let limited = pool
        .open_decompile(code("00", Some(settings)))
        .await
        .unwrap();
    terminal(&pool, limited.snapshot.id).await;
    assert_eq!(
        pool.decompile_result(limited.snapshot.id)
            .unwrap()
            .artifact
            .completion,
        api::decompile_model::Completion::Limited
    );
    let again = pool.open_decompile(link(&limited.source)).await.unwrap();
    assert!(!again.source.cache_hit);
    terminal(&pool, again.snapshot.id).await;
    assert_eq!(count.load(Ordering::SeqCst), 3);
    pool.shutdown().unwrap();
}

#[derive(Clone)]
struct CancelGate(std::sync::mpsc::SyncSender<()>);
impl Executor for CancelGate {
    fn execute(
        &self,
        _: api::AnalyzeRequest,
        _: &Control,
    ) -> Result<api::AnalysisReport, api::ApiError> {
        panic!("not a world task")
    }
    fn decompile(
        &self,
        request: api::DecompileRequest,
        control: &Control,
        _: &evm_abstract_server::decompile::Observer,
    ) -> Result<api::DecompileArtifact, api::ApiError> {
        self.0.send(()).unwrap();
        while !control.cancellation().is_cancelled() {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(evm_decompile::decompile_with_cancel(
            &evm_decompile::Input::new(
                evm_bytecode::parse_hex(&request.bytecode).unwrap(),
                evm_decompile::Fork::Osaka,
            ),
            &DecompileSettings::from_request(&request).config,
            control.cancellation(),
        ))
    }
}

#[tokio::test]
async fn identical_inflight_openings_keep_independent_cancellation_and_no_cached_cancelled_result()
{
    let (started, receiver) = std::sync::mpsc::sync_channel(1);
    let pool = Pool::with_executor_and_cache(
        Config {
            workers: 1,
            queue_capacity: 1,
            retained_jobs: 4,
        },
        CancelGate(started),
        Cache::memory(),
    )
    .unwrap();
    let first = pool.open_decompile(code("00", None)).await.unwrap();
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = pool.open_decompile(link(&first.source)).await.unwrap();
    assert!(!second.source.cache_hit);
    assert_ne!(first.snapshot.id, second.snapshot.id);
    assert_eq!(second.snapshot.state, JobState::Queued);
    pool.cancel_decompile(first.snapshot.id).unwrap();
    assert_eq!(
        terminal(&pool, first.snapshot.id).await.state,
        JobState::Cancelled
    );
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        pool.decompile_status(second.snapshot.id).unwrap().state,
        JobState::Running
    );
    pool.cancel_decompile(second.snapshot.id).unwrap();
    assert_eq!(
        terminal(&pool, second.snapshot.id).await.state,
        JobState::Cancelled
    );
    let next = pool.open_decompile(link(&first.source)).await.unwrap();
    assert!(!next.source.cache_hit);
    pool.cancel_decompile(next.snapshot.id).unwrap();
    terminal(&pool, next.snapshot.id).await;
    pool.shutdown().unwrap();
}
