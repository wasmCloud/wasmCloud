//! # Generic multiplexing core for `(implements ..)` routing
//!
//! The per-builtin multiplexed plugins all share the same set of
//! [`BackendProvider`]s keyed by a `config.backend` discriminator, a registry
//! mapping each host-interface name (the implements id) to a backend, and a
//! [`Multiplexer::resolve`] call that resolves a component import name to its
//! backend.
//!
//! ## Connection pool
//!
//! Backends whose provider supplies a [`BackendProvider::pool_key`] (redis,
//! NATS) are cached and shared across workload binds with the same
//! configuration, so identical interfaces reuse one connection. The pool is
//! self-managing: a single-flight cell collapses concurrent binds for the same
//! key into one `instantiate`, an idle entry is reaped after
//! [`Multiplexer::with_idle_ttl`], and the number of cached connections is
//! capped by [`Multiplexer::with_max_connections`] (least-recently-used
//! eviction). The cap bounds *cached* connections; a connection still held by a
//! live bind keeps working after eviction. Eviction only means a future bind
//! re-instantiates it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OnceCell};

use crate::wit::WitInterface;

/// `config` key naming the backend type for a named host interface (e.g.
/// `redis`, `nats`, `filesystem`). Shared by every multiplexed builtin.
pub const BACKEND_CONFIG_KEY: &str = "backend";

/// Default ceiling on pooled (cached) connections, *per backend type* per
/// multiplexer — so e.g. redis and NATS are capped independently (a multiplexer
/// can hold up to this many of each). Counts distinct backend endpoints kept
/// warm, not concurrent operations.
const DEFAULT_MAX_CONNECTIONS: usize = 64;

/// Default idle period after which a pooled connection is reaped.
const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(300);

/// A factory for backends of one backend type, parameterized by the builtin's
/// id type `Id` (an `Arc<dyn …Backend>`). Selected by a named host interface's
/// `config.backend`; `instantiate` builds a backend from that interface's
/// `config` (credentials, url, root, …), so two interfaces of the same type but
/// different config get independent backends.
#[async_trait::async_trait]
pub trait BackendProvider<Id>: Send + Sync {
    /// The `config.backend` discriminator this provider handles.
    fn backend_type(&self) -> &'static str;
    /// Build a backend from a named host interface's config.
    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<Id>;
    /// A pool key for connection reuse, or `None` to never pool.
    ///
    /// Providers backed by a shared external connection (redis, NATS) return
    /// `Some(key)` derived from the connection config (typically the `url`), so
    /// interfaces with the same configuration share one backend and thus one
    /// connection, instead of opening a fresh one per workload bind. Isolated
    /// providers (in-memory) return `None` and always produce a fresh backend.
    fn pool_key(&self, _config: &HashMap<String, String>) -> Option<String> {
        None
    }
}

/// One slot in the connection pool.
struct PoolEntry<Id> {
    /// Single-flight init cell: concurrent binds for the same key await one
    /// `instantiate`; on error the cell stays empty so the next bind retries
    /// (a failed connection is never cached).
    cell: Arc<OnceCell<Id>>,
    /// Wall-clock of the most recent resolve, for idle reaping and LRU eviction.
    last_used: Instant,
}

/// The reusable multiplexing core: a provider registry, a self-managing
/// connection pool, plus the registry-building and import-resolution logic
/// shared by every multiplexed builtin. Construct one per builtin with its
/// namespace/package and default backend type, register providers, then
/// `build_registry` / `resolve`.
pub struct Multiplexer<Id> {
    namespace: &'static str,
    package: &'static str,
    default_backend_type: &'static str,
    providers: HashMap<&'static str, Arc<dyn BackendProvider<Id>>>,
    /// Connection pool: backends shared across workload binds, keyed by
    /// `backend_type` + the provider's `pool_key`. Reaped when idle and capped
    /// per backend type, so it does not grow without bound.
    pool: Arc<Mutex<HashMap<String, PoolEntry<Id>>>>,
    /// Default ceiling on pooled (cached) connections *per backend type*; LRU
    /// eviction within a backend's group past this.
    max_connections: usize,
    /// Per-backend-type overrides of `max_connections` (e.g. `redis` -> 32).
    max_connections_by_backend: HashMap<&'static str, usize>,
    /// Idle period after which a pooled connection is reaped.
    idle_ttl: Duration,
    /// Whether the background idle-reaper task has been spawned yet (spawned
    /// lazily on the first pooled bind, when a tokio runtime is guaranteed).
    reaper_started: AtomicBool,
    /// Per-workload default backend (the unnamed `""` route), recorded at bind so
    /// a plugin's *standard* (non-`(implements)`) binding can serve a **plain**
    /// (unlabeled) import — the shared mechanism that lets every multiplexed
    /// plugin default without requiring a label. A `std::sync::Mutex` because the
    /// critical section is a tiny map touch with no `await` held across it.
    defaults: Arc<std::sync::Mutex<HashMap<Arc<str>, Id>>>,
}

impl<Id: Clone + Send + Sync + 'static> Multiplexer<Id> {
    /// Create a multiplexer for `namespace:package`, defaulting interfaces with
    /// no `config.backend` to `default_backend_type`.
    pub fn new(
        namespace: &'static str,
        package: &'static str,
        default_backend_type: &'static str,
    ) -> Self {
        Self {
            namespace,
            package,
            default_backend_type,
            providers: HashMap::new(),
            pool: Arc::new(Mutex::new(HashMap::new())),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_connections_by_backend: HashMap::new(),
            idle_ttl: DEFAULT_IDLE_TTL,
            reaper_started: AtomicBool::new(false),
            defaults: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Record the workload's default (unnamed `""`) backend from a built
    /// registry, so a plugin's standard binding can serve a plain import for this
    /// workload. No-op if the workload declared no default (`""`) route.
    pub fn set_default(&self, workload_id: impl Into<Arc<str>>, registry: &HashMap<String, Id>) {
        if let Some(default) = registry.get("") {
            self.defaults
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(workload_id.into(), default.clone());
        }
    }

    /// The default backend a plain (unnamed) import should route to for this
    /// workload, if one was recorded at bind via [`Multiplexer::set_default`].
    pub fn default_for(&self, workload_id: &str) -> Option<Id> {
        self.defaults
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(workload_id)
            .cloned()
    }

    /// Register a backend provider keyed by its `backend_type()`.
    pub fn with_provider(mut self, provider: Arc<dyn BackendProvider<Id>>) -> Self {
        self.providers.insert(provider.backend_type(), provider);
        self
    }

    /// Override the default pooled-connection ceiling *per backend type*
    /// (default [`DEFAULT_MAX_CONNECTIONS`]). Once a backend type's pooled
    /// connections would exceed this, its least-recently-used one is evicted;
    /// each backend type is capped independently. Clamped to at least 1. Use
    /// [`with_max_connections_for`](Self::with_max_connections_for) to override
    /// a single type.
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max.max(1);
        self
    }

    /// Override the pooled-connection ceiling for one backend type (e.g.
    /// `("redis", 32)`), taking precedence over
    /// [`with_max_connections`](Self::with_max_connections) for that type.
    /// Clamped to at least 1.
    pub fn with_max_connections_for(mut self, backend_type: &'static str, max: usize) -> Self {
        self.max_connections_by_backend
            .insert(backend_type, max.max(1));
        self
    }

    /// Override the idle period after which a pooled connection is reaped
    /// (default [`DEFAULT_IDLE_TTL`]).
    pub fn with_idle_ttl(mut self, ttl: Duration) -> Self {
        self.idle_ttl = ttl;
        self
    }

    /// Instantiate the backend for a single interface: the provider chosen by
    /// `config.backend` (or the default backend type) builds it from the
    /// interface's config. Backends whose provider supplies a `pool_key` are
    /// shared across binds for the same configuration. Exposed so callers can
    /// post-process (e.g. wrap with a decorator) before inserting into a
    /// registry.
    pub async fn instantiate(&self, iface: &WitInterface) -> anyhow::Result<Id> {
        let backend_type = iface
            .config
            .get(BACKEND_CONFIG_KEY)
            .map(String::as_str)
            .unwrap_or(self.default_backend_type);
        let provider = self.providers.get(backend_type).ok_or_else(|| {
            anyhow::anyhow!(
                "no {}:{} provider registered for backend '{backend_type}'",
                self.namespace,
                self.package
            )
        })?;

        // Unpooled providers (in-memory, filesystem) always build a fresh,
        // isolated backend — nothing to share or reap.
        let Some(pool_key) = provider.pool_key(&iface.config) else {
            return provider.instantiate(&iface.config).await;
        };
        // `backend_type` is a fixed `&'static str` from a closed set with no
        // null byte, so the `\u{0}` separator makes cross-type collisions
        // impossible.
        let key = format!("{backend_type}\u{0}{pool_key}");

        self.ensure_reaper_running();

        // Grab (or create) this key's single-flight cell, holding the pool lock
        // only long enough to touch the map. Never across the instantiate await
        // below, so one slow/hung connect can't block other binds.
        let cell = {
            let mut pool = self.pool.lock().await;
            reap_idle(&mut pool, self.idle_ttl);
            let entry = pool.entry(key.clone()).or_insert_with(|| PoolEntry {
                cell: Arc::new(OnceCell::new()),
                last_used: Instant::now(),
            });
            entry.last_used = Instant::now();
            entry.cell.clone()
        };

        // Single instantiate shared across concurrent binds for this key: the
        // first to arrive runs the connect, the rest await its result. On error
        // the cell is left empty, so a later bind retries rather than caching a
        // failed connection.
        let backend = cell
            .get_or_try_init(|| provider.instantiate(&iface.config))
            .await?
            .clone();

        // Now that the slot is populated, enforce this backend type's ceiling.
        // Eviction is least-recently-used within the backend's group and never
        // touches the entry we just resolved.
        {
            let max = self
                .max_connections_by_backend
                .get(backend_type)
                .copied()
                .unwrap_or(self.max_connections);
            let mut pool = self.pool.lock().await;
            enforce_limit(&mut pool, backend_type, max, &key);
        }
        Ok(backend)
    }

    /// Build the routing registry (host-interface name -> backend) from a
    /// component's matched host interfaces. Unnamed interfaces are keyed by the
    /// empty string and act as the default route. Interfaces for other
    /// packages are ignored.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<HashMap<String, Id>> {
        self.build_registry_with(interfaces, |_, backend| backend)
            .await
    }

    /// Like [`build_registry`](Self::build_registry), but applies `decorate` to
    /// each freshly-instantiated backend before inserting it.
    /// This keeps the namespace/package filtering and name-keying in one
    /// place so builtins that need post-processing don't reimplement the loop.
    pub async fn build_registry_with<'i, F>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
        mut decorate: F,
    ) -> anyhow::Result<HashMap<String, Id>>
    where
        F: FnMut(&WitInterface, Id) -> Id + Send,
    {
        // Collect up front so no input iterator is held across the `await`s
        // below (keeps the returned future `Send`).
        let interfaces: Vec<&WitInterface> = interfaces.into_iter().collect();
        let mut registry = HashMap::new();
        for iface in interfaces {
            if iface.namespace != self.namespace || iface.package != self.package {
                continue;
            }
            let backend = self.instantiate(iface).await?;
            let backend = decorate(iface, backend);
            registry.insert(iface.name.clone().unwrap_or_default(), backend);
        }
        Ok(registry)
    }

    /// Resolve a component import name (the implements id) to a backend, given a
    /// registry from [`Multiplexer::build_registry`]. The import name is matched
    /// directly against the host-interface name. The unnamed default is keyed
    /// `""`, so an unnamed import matches it directly.
    ///
    /// A named label that matches no configured interface is an error: silently
    /// falling back to the default would route that import's traffic to a backend
    /// (and credentials) it never asked for.
    pub fn resolve(
        &self,
        registry: &HashMap<String, Id>,
        import_name: &str,
    ) -> wasmtime::Result<Id> {
        registry.get(import_name).cloned().ok_or_else(|| {
            wasmtime::format_err!(
                "no {}:{} backend bound for import '{import_name}'",
                self.namespace,
                self.package
            )
        })
    }

    /// Spawn the background idle-reaper once, on the first pooled bind (where a
    /// tokio runtime is guaranteed). The task holds only a `Weak` to the pool,
    /// so it self-terminates when the multiplexer is dropped.
    fn ensure_reaper_running(&self) {
        if self.reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let pool = Arc::downgrade(&self.pool);
        let ttl = self.idle_ttl;
        // Tick at half the TTL so an idle entry is reaped within ~1.5×TTL even
        // if the host goes quiet (opportunistic reaping in `instantiate` only
        // fires when there is bind activity).
        let interval = (ttl / 2).max(Duration::from_secs(1));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let Some(pool) = pool.upgrade() else {
                    return; // multiplexer dropped
                };
                reap_idle(&mut *pool.lock().await, ttl);
            }
        });
    }

    /// Test-only view of the current pool size.
    #[cfg(test)]
    async fn pool_len(&self) -> usize {
        self.pool.lock().await.len()
    }

    /// Test-only count of pooled connections for one backend type.
    #[cfg(test)]
    async fn pool_count_for(&self, backend_type: &str) -> usize {
        let prefix = format!("{backend_type}\u{0}");
        self.pool
            .lock()
            .await
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .count()
    }
}

/// Drop initialized pool entries idle longer than `ttl`. In-flight entries
/// (cell not yet initialized) are always kept — they are being connected right
/// now. Dropping an entry releases the pool's reference to the backend; a
/// connection still held by a live bind stays open until that bind drops it.
fn reap_idle<Id>(pool: &mut HashMap<String, PoolEntry<Id>>, ttl: Duration) {
    let now = Instant::now();
    pool.retain(|_, e| !e.cell.initialized() || now.duration_since(e.last_used) < ttl);
}

/// Evict least-recently-used entries of one backend type until that backend's
/// pooled-connection count is within `max`, never evicting `keep` (the entry
/// just resolved). Only the named backend type's group (keys prefixed
/// `{backend_type}\0`) is considered, so each backend type is capped
/// independently.
fn enforce_limit<Id>(
    pool: &mut HashMap<String, PoolEntry<Id>>,
    backend_type: &str,
    max: usize,
    keep: &str,
) {
    let prefix = format!("{backend_type}\u{0}");
    loop {
        let in_group = pool.keys().filter(|k| k.starts_with(&prefix)).count();
        if in_group <= max {
            break;
        }
        let victim = pool
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix) && k.as_str() != keep)
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| k.clone());
        match victim {
            Some(k) => {
                pool.remove(&k);
            }
            None => break, // only `keep` remains in this backend's group
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;

    fn registry(entries: &[(&str, i32)]) -> HashMap<String, i32> {
        entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn mux() -> Multiplexer<i32> {
        Multiplexer::new("wasi", "keyvalue", "in-memory")
    }

    #[test]
    fn resolve_matches_by_import_name_including_the_unnamed_default() {
        let reg = registry(&[("team-a", 1), ("", 99)]);
        let mux = mux();
        assert_eq!(mux.resolve(&reg, "team-a").unwrap(), 1);
        // The unnamed default is keyed "" and matched directly.
        assert_eq!(mux.resolve(&reg, "").unwrap(), 99);
        // An unmatched named label is an error, not a silent fallback to the
        // default — routing it there would use the wrong backend/credentials.
        let err = mux.resolve(&reg, "other").unwrap_err();
        assert!(
            err.to_string().contains("other"),
            "error should name the unmatched import: {err}"
        );
    }

    #[test]
    fn set_default_records_the_unnamed_route_per_workload() {
        let mux = mux();
        // A workload whose registry carries an unnamed (`""`) route gets that
        // backend recorded as its default; a plain import looks it up here.
        mux.set_default("wl-1", &registry(&[("team-a", 1), ("", 99)]));
        assert_eq!(mux.default_for("wl-1"), Some(99));
        // Defaults are per-workload: an unknown workload has none.
        assert_eq!(mux.default_for("wl-unknown"), None);
        // A registry with no unnamed route records nothing (a labeled-only
        // workload has no plain-import default to fall back to).
        mux.set_default("wl-2", &registry(&[("team-a", 1)]));
        assert_eq!(mux.default_for("wl-2"), None);
    }

    #[test]
    fn resolve_errors_without_match_or_default() {
        let reg = registry(&[("team-a", 1)]);
        let err = mux().resolve(&reg, "nope").unwrap_err();
        assert!(
            err.to_string().contains("wasi:keyvalue"),
            "error should name the package: {err}"
        );
    }

    // A provider that counts `instantiate` calls and pools (by `url`) when
    // constructed with `pooled = true`. `Id = Arc<usize>` so backend identity
    // is observable via `Arc::ptr_eq`. `delay` widens the instantiate window so
    // concurrent binds genuinely overlap in the single-flight test.
    struct CountingProvider {
        backend: &'static str,
        count: AtomicUsize,
        pooled: bool,
        delay: Duration,
    }

    impl CountingProvider {
        fn new(pooled: bool) -> Arc<Self> {
            Self::typed("test", pooled, Duration::ZERO)
        }
        fn with_delay(pooled: bool, delay: Duration) -> Arc<Self> {
            Self::typed("test", pooled, delay)
        }
        fn typed(backend: &'static str, pooled: bool, delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                backend,
                count: AtomicUsize::new(0),
                pooled,
                delay,
            })
        }
    }

    #[async_trait::async_trait]
    impl BackendProvider<Arc<usize>> for CountingProvider {
        fn backend_type(&self) -> &'static str {
            self.backend
        }
        async fn instantiate(&self, _: &HashMap<String, String>) -> anyhow::Result<Arc<usize>> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(Arc::new(self.count.fetch_add(1, SeqCst)))
        }
        fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
            self.pooled.then(|| config.get("url").cloned()).flatten()
        }
    }

    fn iface(backend: &str, url: &str) -> WitInterface {
        WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: None,
            config: HashMap::from([
                ("backend".to_string(), backend.to_string()),
                ("url".to_string(), url.to_string()),
            ]),
            name: None,
        }
    }

    fn url_iface(url: &str) -> WitInterface {
        iface("test", url)
    }

    #[tokio::test]
    async fn pooled_provider_shares_backend_for_same_config() {
        let provider = CountingProvider::new(true);
        let mux = Multiplexer::new("wasi", "keyvalue", "test").with_provider(provider.clone());

        let a = mux.instantiate(&url_iface("redis://x")).await.unwrap();
        let b = mux.instantiate(&url_iface("redis://x")).await.unwrap();
        assert!(Arc::ptr_eq(&a, &b), "same config should share one backend");
        assert_eq!(
            provider.count.load(SeqCst),
            1,
            "instantiate should run once for a pooled config"
        );

        // A different url is a different connection.
        let c = mux.instantiate(&url_iface("redis://y")).await.unwrap();
        assert!(!Arc::ptr_eq(&a, &c), "different config = different backend");
    }

    #[tokio::test]
    async fn unpooled_provider_makes_fresh_backends() {
        let provider = CountingProvider::new(false);
        let mux = Multiplexer::new("wasi", "keyvalue", "test").with_provider(provider.clone());
        let a = mux.instantiate(&url_iface("redis://x")).await.unwrap();
        let b = mux.instantiate(&url_iface("redis://x")).await.unwrap();
        assert!(
            !Arc::ptr_eq(&a, &b),
            "no pool key = fresh backend each time"
        );
        assert_eq!(provider.count.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn pooled_instantiate_is_single_flight() {
        // Many concurrent binds for the same key must trigger exactly one
        // connect (the thundering-herd guard), and all share one backend.
        let provider = CountingProvider::with_delay(true, Duration::from_millis(50));
        let mux =
            Arc::new(Multiplexer::new("wasi", "keyvalue", "test").with_provider(provider.clone()));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let mux = mux.clone();
            handles.push(tokio::spawn(async move {
                mux.instantiate(&url_iface("redis://x")).await.unwrap()
            }));
        }
        let mut backends = Vec::new();
        for h in handles {
            backends.push(h.await.unwrap());
        }
        for b in &backends {
            assert!(Arc::ptr_eq(b, &backends[0]), "all binds share one backend");
        }
        assert_eq!(
            provider.count.load(SeqCst),
            1,
            "concurrent binds for one key must instantiate exactly once"
        );
    }

    #[tokio::test]
    async fn pool_evicts_lru_over_connection_limit() {
        let provider = CountingProvider::new(true);
        let mux = Multiplexer::new("wasi", "keyvalue", "test")
            .with_provider(provider.clone())
            .with_max_connections(2);

        let _a = mux.instantiate(&url_iface("redis://a")).await.unwrap();
        let _b = mux.instantiate(&url_iface("redis://b")).await.unwrap();
        // Third distinct connection evicts the LRU entry — `a`, the oldest.
        let _c = mux.instantiate(&url_iface("redis://c")).await.unwrap();
        assert_eq!(mux.pool_len().await, 2, "pool must stay within the limit");
        assert_eq!(
            provider.count.load(SeqCst),
            3,
            "three distinct backends so far"
        );

        // `b` and `c` survived (only the LRU `a` was evicted): re-resolving them
        // is a cache hit, so it does NOT re-instantiate.
        let _b2 = mux.instantiate(&url_iface("redis://b")).await.unwrap();
        let _c2 = mux.instantiate(&url_iface("redis://c")).await.unwrap();
        assert_eq!(
            provider.count.load(SeqCst),
            3,
            "b and c are cache hits, not re-instantiated — so a was the evicted one"
        );

        // `a` was the evicted entry, so re-resolving it instantiates afresh.
        let _a2 = mux.instantiate(&url_iface("redis://a")).await.unwrap();
        assert_eq!(
            provider.count.load(SeqCst),
            4,
            "evicted 'a' re-instantiates"
        );
    }

    #[tokio::test]
    async fn pool_limits_are_per_backend_type() {
        // Two backend types share one multiplexer; "test" is capped at 1 while
        // "other" uses the default. Each type's pool is bounded independently.
        let mux = Multiplexer::new("wasi", "keyvalue", "test")
            .with_provider(CountingProvider::typed("test", true, Duration::ZERO))
            .with_provider(CountingProvider::typed("other", true, Duration::ZERO))
            .with_max_connections(10)
            .with_max_connections_for("test", 1);

        // Two distinct "test" connections -> capped at 1 (LRU evicts the first).
        let _ta = mux.instantiate(&iface("test", "redis://a")).await.unwrap();
        let _tb = mux.instantiate(&iface("test", "redis://b")).await.unwrap();
        // Two "other" connections -> not affected by the "test" cap.
        let _oa = mux.instantiate(&iface("other", "nats://a")).await.unwrap();
        let _ob = mux.instantiate(&iface("other", "nats://b")).await.unwrap();

        assert_eq!(
            mux.pool_count_for("test").await,
            1,
            "'test' backend capped at 1"
        );
        assert_eq!(
            mux.pool_count_for("other").await,
            2,
            "'other' backend uses the default cap, unaffected by the 'test' override"
        );
    }

    #[tokio::test]
    async fn pool_reaps_idle_entries() {
        let provider = CountingProvider::new(true);
        let mux = Multiplexer::new("wasi", "keyvalue", "test")
            .with_provider(provider.clone())
            .with_idle_ttl(Duration::from_millis(20));

        let _a = mux.instantiate(&url_iface("redis://a")).await.unwrap();
        assert_eq!(mux.pool_len().await, 1);

        // After the TTL the idle entry is reaped — by the background reaper
        // and/or opportunistically on the next bind.
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _b = mux.instantiate(&url_iface("redis://b")).await.unwrap();
        assert_eq!(mux.pool_len().await, 1, "idle 'a' should have been reaped");
    }
}
