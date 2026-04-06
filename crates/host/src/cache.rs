use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    hash::Hash,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Instant,
};

use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio_util::time::{delay_queue, DelayQueue};

#[derive(Debug)]
struct CacheEntry<V>
where
    V: Clone + Debug + Send + Sync + 'static,
{
    value: V,
    created_at: Instant,
}

impl<V> CacheEntry<V>
where
    V: Clone + Debug + Send + Sync + 'static,
{
    fn new(value: V) -> Self {
        Self {
            value,
            created_at: Instant::now(),
        }
    }
}

#[derive(Debug)]
struct CacheInternal<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    data: HashMap<K, (CacheEntry<V>, delay_queue::Key)>,
    expirations: DelayQueue<K>,
    ttl: Duration,
    lazy_expiration: bool,
}

impl<K, V> CacheInternal<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    fn new(ttl: Duration) -> Self {
        Self {
            data: HashMap::new(),
            expirations: DelayQueue::new(),
            ttl,
            lazy_expiration: true,
        }
    }

    fn insert(&mut self, key: K, value: V) {
        match self.data.entry(key.clone()) {
            Entry::Occupied(mut entry) => {
                self.expirations.remove(&entry.get().1);
                let expiration = self.expirations.insert(key, self.ttl);
                entry.insert((CacheEntry::new(value), expiration));
            }
            Entry::Vacant(entry) => {
                let expiration = self.expirations.insert(key, self.ttl);
                entry.insert((CacheEntry::new(value), expiration));
            }
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        match self.data.entry(key.clone()) {
            Entry::Occupied(entry) if entry.get().0.created_at.elapsed() < self.ttl => {
                Some(entry.get().0.value.clone())
            }
            Entry::Occupied(entry) => {
                if self.lazy_expiration {
                    let (_, expiration) = entry.remove();
                    self.expirations.remove(&expiration);
                }
                None
            }
            Entry::Vacant(_) => None,
        }
    }

    fn poll_purge(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        while let Poll::Ready(Some(entry)) = self.expirations.poll_expired(cx) {
            self.data.remove(entry.get_ref());
        }

        Poll::Ready(())
    }
}

/// A thread-safe TTL cache with lazy expiration and optional purge cycles.
pub(crate) struct Cache<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    internal: Arc<RwLock<CacheInternal<K, V>>>,
    purge_cycles: Option<u32>,
    purge_cycle_count: Arc<AtomicU32>,
}

impl<K, V> Cache<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    pub(crate) fn builder() -> CacheBuilder<K, V> {
        CacheBuilder::default()
    }

    pub(crate) async fn insert(&self, key: K, value: V) {
        self.check_purge().await;
        self.internal.write().await.insert(key, value);
    }

    pub(crate) async fn get(&self, key: &K) -> Option<V> {
        self.internal.write().await.get(key)
    }

    async fn poll_purge_once(&self) {
        let mut cache = self.internal.write().await;
        std::future::poll_fn(|cx| cache.poll_purge(cx)).await;
        self.purge_cycle_count.store(0, Ordering::Relaxed);
    }

    async fn check_purge(&self) {
        if let Some(purge_cycles) = self.purge_cycles {
            if self.purge_cycle_count.fetch_add(1, Ordering::Relaxed) + 1 >= purge_cycles {
                self.poll_purge_once().await;
            }
        }
    }
}

impl<K, V> Clone for Cache<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            internal: Arc::clone(&self.internal),
            purge_cycles: self.purge_cycles,
            purge_cycle_count: Arc::clone(&self.purge_cycle_count),
        }
    }
}

impl<K, V> Debug for Cache<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache")
            .field("purge_cycles", &self.purge_cycles)
            .field(
                "purge_cycle_count",
                &self.purge_cycle_count.load(Ordering::Relaxed),
            )
            .finish()
    }
}

pub(crate) struct CacheBuilder<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    ttl: Duration,
    purge_cycles: Option<u32>,
    lazy_expiration: bool,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> Default for CacheBuilder<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(300),
            purge_cycles: None,
            lazy_expiration: true,
            _marker: PhantomData,
        }
    }
}

impl<K, V> CacheBuilder<K, V>
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
    V: Clone + Debug + Send + Sync + 'static,
{
    pub(crate) fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub(crate) fn with_purge_cycles(mut self, purge_cycles: u32) -> Self {
        self.purge_cycles = Some(purge_cycles);
        self
    }

    pub(crate) fn with_lazy_expiration(mut self, lazy_expiration: bool) -> Self {
        self.lazy_expiration = lazy_expiration;
        self
    }

    pub(crate) fn build(self) -> Cache<K, V> {
        let mut internal = CacheInternal::new(self.ttl);
        internal.lazy_expiration = self.lazy_expiration;

        Cache {
            internal: Arc::new(RwLock::new(internal)),
            purge_cycles: self.purge_cycles,
            purge_cycle_count: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cache;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn returns_cached_value_before_ttl() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .build();

        cache.insert("key".into(), "value".into()).await;

        assert_eq!(cache.get(&"key".to_string()).await, Some("value".into()));
    }

    #[tokio::test]
    async fn expires_cached_value_after_ttl() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(50))
            .with_purge_cycles(1)
            .build();

        cache.insert("key".into(), "value".into()).await;
        sleep(Duration::from_millis(75)).await;

        assert_eq!(cache.get(&"key".to_string()).await, None);
    }
}
