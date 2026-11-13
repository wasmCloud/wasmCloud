//! Embedder contract: these tests use only the runtime's public API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use wash_runtime::{
    cache::{Cache, DefaultCache},
    engine::{CacheKey, CacheValue, Engine},
    host::Host,
    types::{Component, Workload},
};

#[derive(Default)]
struct MemoryCache {
    compiled: Mutex<HashMap<CacheKey, CacheValue>>,
    reads: AtomicUsize,
    writes: AtomicUsize,
    fail_read: AtomicBool,
    fail_write: AtomicBool,
}

#[async_trait::async_trait]
impl Cache for MemoryCache {
    async fn get_compiled(&self, key: &CacheKey) -> Result<Option<CacheValue>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_read.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("backend read unavailable").into());
        }
        Ok(self.compiled.lock().unwrap().get(key).cloned())
    }
    async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> Result<()> {
        if self.fail_write.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("backend write unavailable").into());
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.compiled.lock().unwrap().insert(key.clone(), value);
        Ok(())
    }
}

fn engine(cache: Arc<dyn Cache>) -> Engine {
    Engine::builder()
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .with_cache(cache)
        .build()
        .unwrap()
}

fn workload() -> Workload {
    Workload {
        name: "cached".into(),
        namespace: "test".into(),
        annotations: Default::default(),
        service: None,
        volumes: vec![],
        host_interfaces: vec![],
        components: vec![Component {
            name: "component".into(),
            bytes: wat::parse_str("(component)").unwrap().into(),
            ..Default::default()
        }],
    }
}

#[tokio::test]
async fn custom_retention_is_used_and_live_components_survive_eviction() -> Result<()> {
    let cache = Arc::new(MemoryCache::default());
    let engine = engine(cache.clone());
    let first = engine.initialize_workload("first", workload()).await?;
    let cold_reads = cache.reads.load(Ordering::SeqCst);
    assert!(cold_reads > 0);
    cache.compiled.lock().unwrap().clear();
    let second = engine.initialize_workload("second", workload()).await?;
    assert_eq!(cache.reads.load(Ordering::SeqCst), cold_reads);
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    drop((first, second));
    let third = engine.initialize_workload("third", workload()).await?;
    assert_eq!(cache.writes.load(Ordering::SeqCst), 2);
    drop(third);
    let reads_before_hit = cache.reads.load(Ordering::SeqCst);
    engine.initialize_workload("fourth", workload()).await?;
    assert_eq!(cache.reads.load(Ordering::SeqCst), reads_before_hit + 1);
    assert_eq!(cache.writes.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn sharing_a_backend_does_not_share_compiles_between_engines() -> Result<()> {
    let cache = Arc::new(MemoryCache::default());
    let first = engine(cache.clone());
    let second = engine(cache.clone());
    first.initialize_workload("first", workload()).await?;
    second.initialize_workload("second", workload()).await?;
    let entries = cache.compiled.lock().unwrap();
    assert_eq!(entries.len(), 2);
    let mut keys = entries.keys();
    let a = keys.next().unwrap();
    let b = keys.next().unwrap();
    assert_eq!(a.digest(), b.digest());
    assert_ne!(a.engine(), b.engine());
    Ok(())
}

#[tokio::test]
async fn backend_errors_cost_a_compile_and_never_the_start() -> Result<()> {
    let cache = Arc::new(MemoryCache::default());
    let engine = engine(cache.clone());

    // A failed read is a miss: the start compiles and publishes.
    cache.fail_read.store(true, Ordering::SeqCst);
    drop(
        engine
            .initialize_workload("read-failed", workload())
            .await?,
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);

    // A failed write keeps nothing, and the start still has its component.
    cache.compiled.lock().unwrap().clear();
    cache.fail_write.store(true, Ordering::SeqCst);
    drop(
        engine
            .initialize_workload("write-failed", workload())
            .await?,
    );
    assert_eq!(cache.writes.load(Ordering::SeqCst), 1);
    assert!(cache.compiled.lock().unwrap().is_empty());

    // Once the backend recovers, retention resumes.
    drop(engine.initialize_workload("recovered", workload()).await?);
    assert_eq!(cache.writes.load(Ordering::SeqCst), 2);
    engine.initialize_workload("cached", workload()).await?;
    assert_eq!(cache.writes.load(Ordering::SeqCst), 2);
    Ok(())
}

struct GatedCache {
    first: AtomicBool,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    dropped: AtomicUsize,
    inner: MemoryCache,
}
struct Waiting<'a>(&'a AtomicUsize);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl Cache for GatedCache {
    async fn get_compiled(&self, key: &CacheKey) -> Result<Option<CacheValue>> {
        if self.first.swap(false, Ordering::SeqCst) {
            let _waiting = Waiting(&self.dropped);
            self.entered.add_permits(1);
            self.release.acquire().await?.forget();
        }
        self.inner.get_compiled(key).await
    }
    async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> Result<()> {
        self.inner.put_compiled(key, value).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_pending_cache_read_is_async_and_cancellation_releases_the_flight() -> Result<()> {
    let cache = Arc::new(GatedCache {
        first: AtomicBool::new(true),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        dropped: AtomicUsize::new(0),
        inner: MemoryCache::default(),
    });
    let engine = engine(cache.clone());
    let task = tokio::spawn({
        let engine = engine.clone();
        async move { engine.initialize_workload("cancelled", workload()).await }
    });
    // This can progress on a single-thread executor while the read waits.
    tokio::time::timeout(std::time::Duration::from_secs(5), cache.entered.acquire())
        .await??
        .forget();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(cache.dropped.load(Ordering::SeqCst), 1);
    engine.initialize_workload("retry", workload()).await?;
    assert_eq!(cache.inner.writes.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Holds the first write until released.
struct GatedWrite {
    first: AtomicBool,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    inner: MemoryCache,
}
#[async_trait::async_trait]
impl Cache for GatedWrite {
    async fn get_compiled(&self, key: &CacheKey) -> Result<Option<CacheValue>> {
        self.inner.get_compiled(key).await
    }
    async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> Result<()> {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.release.acquire().await?.forget();
        }
        self.inner.put_compiled(key, value).await
    }
}

/// A backend slow to retain a compile does not hold up the start that produced
/// it. A start arriving before the write lands reuses the compile in hand.
#[tokio::test]
async fn a_slow_write_does_not_delay_the_start() -> Result<()> {
    let cache = Arc::new(GatedWrite {
        first: AtomicBool::new(true),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        inner: MemoryCache::default(),
    });
    let engine = engine(cache.clone());
    let wait = std::time::Duration::from_secs(30);
    let first =
        tokio::time::timeout(wait, engine.initialize_workload("first", workload())).await??;
    assert_eq!(cache.entered.available_permits(), 1, "the write began");
    assert_eq!(cache.inner.writes.load(Ordering::SeqCst), 0);

    let reads = cache.inner.reads.load(Ordering::SeqCst);
    drop(first);
    engine.initialize_workload("second", workload()).await?;
    assert_eq!(cache.inner.reads.load(Ordering::SeqCst), reads);

    cache.release.add_permits(1);
    tokio::time::timeout(wait, async {
        while cache.inner.writes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(cache.inner.writes.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A start cancelled after its compile began leaves that compile in the cache:
/// the retry is answered by it instead of compiling again.
#[tokio::test]
async fn a_cancelled_start_still_publishes_its_compile() -> Result<()> {
    let cache = Arc::new(GatedWrite {
        first: AtomicBool::new(true),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        inner: MemoryCache::default(),
    });
    let engine = engine(cache.clone());
    let task = tokio::spawn({
        let engine = engine.clone();
        async move { engine.initialize_workload("cancelled", workload()).await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(30), cache.entered.acquire())
        .await??
        .forget();
    task.abort();
    // Publication never gates the start, which may have finished already.
    drop(task.await);

    cache.release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while cache.inner.writes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let reads_before_retry = cache.inner.reads.load(Ordering::SeqCst);
    engine.initialize_workload("retry", workload()).await?;
    assert_eq!(cache.inner.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        cache.inner.reads.load(Ordering::SeqCst),
        reads_before_retry + 1
    );
    Ok(())
}

#[test]
fn host_construction_preserves_one_handle_and_rejects_conflicts() -> Result<()> {
    let cache: Arc<dyn Cache> = Arc::new(MemoryCache::default());
    let engine = engine(cache.clone());
    let host = Host::builder()
        .with_engine(engine.clone())
        .with_cache(cache.clone())
        .build()?;
    assert!(Arc::ptr_eq(&cache, &host.cache()));
    assert!(
        Host::builder()
            .with_engine(engine)
            .with_cache(Arc::new(DefaultCache::default()))
            .build()
            .is_err()
    );
    Ok(())
}

/// A compiled-component backend is only that: it implements no OCI trait, and
/// the host's OCI retention stays on the default, configured directory included.
/// The engine's handle is the one taken before the host was built.
#[cfg(feature = "oci")]
#[test]
fn a_compile_backend_leaves_oci_retention_on_the_configured_default() -> Result<()> {
    use wash_runtime::host::HostConfig;

    let directory = tempfile::tempdir()?;
    let engine = engine(Arc::new(MemoryCache::default()));
    let before = engine.cache();
    let host = Host::builder()
        .with_engine(engine)
        .with_config(HostConfig {
            oci_cache_dir: Some(directory.path().to_owned()),
            ..Default::default()
        })
        .build()?;
    assert!(Arc::ptr_eq(&before, &host.cache()));
    let oci = host.oci_cache();
    assert_eq!(oci.oci_reserved_paths(), [directory.path().to_owned()]);
    assert!(oci.oci_needs_maintenance());
    Ok(())
}

/// An explicit OCI backend owns its storage, so a cache directory beside it
/// would go unused. The host refuses to build instead of dropping it silently.
#[cfg(feature = "oci")]
#[test]
fn an_oci_backend_beside_a_cache_directory_is_refused() -> Result<()> {
    use wash_runtime::{
        cache::{DefaultOciCache, OciCache},
        host::HostConfig,
    };

    let directory = tempfile::tempdir()?;
    let backend: Arc<dyn OciCache> = Arc::new(DefaultOciCache::default());
    let host = Host::builder().with_oci_cache(backend.clone()).build()?;
    assert!(Arc::ptr_eq(&backend, &host.oci_cache()));

    let Err(error) = Host::builder()
        .with_oci_cache(backend)
        .with_config(HostConfig {
            oci_cache_dir: Some(directory.path().to_owned()),
            ..Default::default()
        })
        .build()
    else {
        panic!("a directory beside an explicit backend should be refused");
    };
    assert!(format!("{error:#}").contains("oci_cache_dir"), "{error:#}");
    Ok(())
}

/// A compiled-component backend's own storage is closed to guest volumes,
/// even where the host path allowlist would otherwise admit it.
#[tokio::test]
async fn a_backend_reserves_its_paths_from_guest_volumes() -> Result<()> {
    use wash_runtime::types::{HostPathVolume, Volume, VolumeType};

    struct OnDisk(std::path::PathBuf);
    impl Cache for OnDisk {
        fn reserved_paths(&self) -> Vec<std::path::PathBuf> {
            vec![self.0.clone()]
        }
    }
    let root = tempfile::tempdir()?;
    let (storage, data) = (root.path().join("cache"), root.path().join("data"));
    std::fs::create_dir_all(&storage)?;
    std::fs::create_dir_all(&data)?;
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .with_allowed_host_paths([root.path().to_owned()])
        .with_cache(Arc::new(OnDisk(storage.clone())))
        .build()?;
    let mounting = |path: &std::path::Path| Workload {
        volumes: vec![Volume {
            name: "v".into(),
            volume_type: VolumeType::HostPath(HostPathVolume {
                local_path: path.to_string_lossy().into_owned(),
            }),
        }],
        ..workload()
    };
    engine
        .initialize_workload("allowed", mounting(&data))
        .await?;
    assert!(
        engine
            .initialize_workload("reserved", mounting(&storage))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn default_trait_methods_allow_a_cache_to_disable_all_retention() -> Result<()> {
    struct Disabled;
    impl Cache for Disabled {}
    let engine = engine(Arc::new(Disabled));
    engine.initialize_workload("first", workload()).await?;
    engine.initialize_workload("second", workload()).await?;
    Ok(())
}

#[tokio::test]
async fn an_entry_from_the_wrong_engine_is_rejected() -> Result<()> {
    struct Foreign(CacheValue);
    #[async_trait::async_trait]
    impl Cache for Foreign {
        async fn get_compiled(&self, _: &CacheKey) -> Result<Option<CacheValue>> {
            Ok(Some(self.0.clone()))
        }
    }
    let source = engine(Arc::new(MemoryCache::default()));
    let component = wash_runtime::wasmtime::component::Component::new(
        source.inner(),
        wat::parse_str("(component)")?,
    )?;
    let target = engine(Arc::new(Foreign(CacheValue::new(component))));
    let error = target
        .initialize_workload("wrong-engine", workload())
        .await
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("different engine"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn backend_names_support_owned_labels() {
    struct Named(String);
    impl Cache for Named {
        fn name(&self) -> &str {
            &self.0
        }
    }
    let backend: Arc<dyn Cache> = Arc::new(Named(format!("tenant-{}", 42)));
    assert_eq!(backend.name(), "tenant-42");
    assert!(format!("{backend:?}").contains("tenant-42"));
    assert!(MemoryCache::default().name().ends_with("MemoryCache"));
}
