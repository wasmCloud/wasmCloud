use std::marker::PhantomData;
use std::sync::Arc;
use std::task::ready;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
    time::Instant,
};
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio_util::time::{delay_queue, DelayQueue};

#[derive(Debug, Clone)]
struct CacheEntry<V>
where
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    value: V,
    created_at: Instant,
}

impl<V> CacheEntry<V>
where
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn new(value: V) -> Self {
        Self {
            value,
            created_at: Instant::now(),
        }
    }

    fn get_value(&self) -> V {
        self.value.clone()
    }

    fn get_created_at(&self) -> Instant {
        self.created_at
    }
}

#[derive(Debug)]
struct CacheInternal<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    data: HashMap<K, (CacheEntry<V>, delay_queue::Key)>,
    expirations: DelayQueue<K>,
    ttl: Duration,
    lazy: bool,
}

impl<K, V> CacheInternal<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn new(ttl: Duration) -> Self {
        Self {
            data: HashMap::new(),
            expirations: DelayQueue::new(),
            ttl,
            lazy: true,
        }
    }

    fn insert(&mut self, key: K, value: V) {
        let delay = self.expirations.insert(key.clone(), self.ttl);
        self.data
            .insert(key.clone(), (CacheEntry::new(value), delay));
    }

    fn get(&mut self, key: &K) -> Option<V> {
        match self.data.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().0.get_created_at().elapsed() < self.ttl =>
            {
                Some(entry.get().0.get_value())
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if self.lazy {
                    let (_, delay_key) = entry.remove();
                    self.expirations.remove(&delay_key);
                }
                None
            }
            std::collections::hash_map::Entry::Vacant(_) => None,
        }
    }

    fn remove(&mut self, key: &K) {
        if let Some((_, cache_key)) = self.data.remove(key) {
            self.expirations.remove(&cache_key);
        }
    }

    fn poll_purge(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        while let Some(entry) = ready!(self.expirations.poll_expired(cx)) {
            self.data.remove(entry.get_ref());
        }

        Poll::Ready(())
    }
}

/// A thread-safe, TTL-based cache with configurable purging strategies
pub struct Cache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    internal: Arc<RwLock<CacheInternal<K, V>>>,
    purge_cycles: Option<u32>,
    purge_cycle_count: AtomicU32,
}

impl<K, V> Cache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    /// Create a new cache with the specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            internal: Arc::new(RwLock::new(CacheInternal::new(ttl))),
            purge_cycles: None,
            purge_cycle_count: AtomicU32::new(0),
        }
    }

    /// Create a new cache builder for configuring cache settings
    pub fn builder() -> CacheBuilder<K, V> {
        CacheBuilder::new()
    }

    /// Insert a key-value pair into the cache
    pub async fn insert(&self, key: K, value: V) {
        self.check_purge().await;
        self.internal.write().await.insert(key, value);
    }

    /// Get a value from the cache, returning None if expired or not found
    pub async fn get(&self, key: &K) -> Option<V> {
        self.internal.write().await.get(key)
    }

    /// Remove a key-value pair from the cache
    pub async fn remove(&self, key: &K) {
        self.internal.write().await.remove(key);
    }

    /// Manually trigger a single purge cycle to remove expired entries
    pub async fn poll_purge_once(&self) {
        let mut cache = self.internal.write().await;
        std::future::poll_fn(|cx| cache.poll_purge(cx)).await;
        self.purge_cycle_count.store(0, Ordering::Relaxed);
    }

    async fn check_purge(&self) {
        if let Some(purge_cycles) = self.purge_cycles {
            self.purge_cycle_count.fetch_add(1, Ordering::Relaxed);
            if self.purge_cycle_count.load(Ordering::Relaxed) >= purge_cycles {
                self.poll_purge_once().await;
            }
        }
    }
}

impl<K, V> std::fmt::Debug for Cache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
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

impl<K, V> Clone for Cache<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            internal: Arc::clone(&self.internal),
            purge_cycles: self.purge_cycles,
            purge_cycle_count: AtomicU32::new(self.purge_cycle_count.load(Ordering::Relaxed)),
        }
    }
}

/// A builder for creating a new cache with configurable settings
///
/// Example:
/// ```
/// use std::time::Duration;
/// use wasmcloud_host::cache::Cache;
///
/// let cache = Cache::<String, i32>::builder()
///     .with_ttl(Duration::from_secs(10))
///     .with_purge_cycles(10)
///     .build();
/// ```
pub struct CacheBuilder<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    ttl: Duration,
    purge_cycles: Option<u32>,
    lazy: bool,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V> Default for CacheBuilder<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(300), // 5 minutes default
            purge_cycles: None,
            lazy: true,
            _phantom: PhantomData,
        }
    }
}

impl<K, V> CacheBuilder<K, V>
where
    K: std::hash::Hash + Eq + Clone + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    /// Create a new cache builder with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the time-to-live (TTL) for cache entries
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set the number of operations after which to run a purge cycle
    /// If None, purging only happens on explicit timer expiration
    pub fn with_purge_cycles(mut self, cycles: u32) -> Self {
        self.purge_cycles = Some(cycles);
        self
    }

    /// Disable automatic purge cycles (default behavior)
    pub fn without_purge_cycles(mut self) -> Self {
        self.purge_cycles = None;
        self
    }

    /// Set whether to use lazy expiration (check expiration on access)
    /// If false, entries are only removed during purge cycles
    pub fn with_lazy_expiration(mut self, lazy: bool) -> Self {
        self.lazy = lazy;
        self
    }

    /// Build the cache with the configured settings
    pub fn build(self) -> Cache<K, V> {
        let mut internal = CacheInternal::new(self.ttl);
        internal.lazy = self.lazy;

        Cache {
            internal: Arc::new(RwLock::new(internal)),
            purge_cycles: self.purge_cycles,
            purge_cycle_count: AtomicU32::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_basic_insert_and_get() {
        let cache = Cache::new(Duration::from_millis(100));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        cache.insert("key2".to_string(), "value2".to_string()).await;

        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );
        assert_eq!(
            cache.get(&"key2".to_string()).await,
            Some("value2".to_string())
        );
        assert_eq!(cache.get(&"nonexistent".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_remove() {
        let cache = Cache::new(Duration::from_millis(100));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        cache.remove(&"key1".to_string()).await;
        assert_eq!(cache.get(&"key1".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let cache = Cache::new(Duration::from_millis(100));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        // Wait for TTL to expire
        sleep(Duration::from_millis(150)).await;
        assert_eq!(cache.get(&"key1".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_ttl_not_expired() {
        let cache = Cache::new(Duration::from_millis(200));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        // Wait less than TTL
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );
    }

    #[tokio::test]
    async fn test_expiration_with_purge_cycles() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .with_purge_cycles(2)
            .with_lazy_expiration(false)
            .build();

        cache.insert("key1".to_string(), "value1".to_string()).await;

        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        sleep(Duration::from_millis(150)).await;
        assert_eq!(cache.get(&"key1".to_string()).await, None);
        // The key should still be in the cache because the purge cycle has not been triggered
        assert_eq!(cache.internal.read().await.data.len(), 1);
        cache.insert("key2".to_string(), "value2".to_string()).await;
        assert_eq!(
            cache.get(&"key2".to_string()).await,
            Some("value2".to_string())
        );
        // There should be one because the key1 is expired and purge cycle was triggered
        assert_eq!(cache.internal.read().await.data.len(), 1);
    }

    #[tokio::test]
    async fn test_expiration_without_purge_cycles() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .without_purge_cycles()
            .with_lazy_expiration(false)
            .build();

        cache.insert("key1".to_string(), "value1".to_string()).await;

        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        sleep(Duration::from_millis(150)).await;
        assert_eq!(cache.get(&"key1".to_string()).await, None);
        // The key should still be in the cache because the purge cycle has not been triggered
        assert_eq!(cache.internal.read().await.data.len(), 1);
        cache.insert("key2".to_string(), "value2".to_string()).await;
        assert_eq!(
            cache.get(&"key2".to_string()).await,
            Some("value2".to_string())
        );
        // The should be two because the key1 is expired and purge cycle was not triggered
        assert_eq!(cache.internal.read().await.data.len(), 2);
    }

    #[tokio::test]
    async fn test_builder_lazy_expiration() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .with_lazy_expiration(true)
            .build();

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        sleep(Duration::from_millis(150)).await;
        assert_eq!(cache.get(&"key1".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_manual_purge() {
        let cache = Cache::new(Duration::from_millis(50));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        cache.insert("key2".to_string(), "value2".to_string()).await;

        // Wait for entries to expire
        sleep(Duration::from_millis(100)).await;

        // Manual purge should remove expired entries
        cache.poll_purge_once().await;

        // Even with lazy expiration, purged entries should be gone
        assert_eq!(cache.get(&"key1".to_string()).await, None);
        assert_eq!(cache.get(&"key2".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_cache_clone() {
        let cache1 = Cache::new(Duration::from_millis(100));
        cache1
            .insert("key1".to_string(), "value1".to_string())
            .await;

        let cache2 = cache1.clone();

        // Both caches should share the same data
        assert_eq!(
            cache2.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        cache2
            .insert("key2".to_string(), "value2".to_string())
            .await;
        assert_eq!(
            cache1.get(&"key2".to_string()).await,
            Some("value2".to_string())
        );
    }

    #[tokio::test]
    async fn test_cache_debug() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .with_purge_cycles(10)
            .build();

        let debug_str = format!("{:?}", cache);
        assert!(debug_str.contains("Cache"));
        assert!(debug_str.contains("purge_cycles"));
        assert!(debug_str.contains("purge_cycle_count"));
    }

    #[tokio::test]
    async fn test_cache_with_different_types() {
        let cache = Cache::new(Duration::from_millis(100));

        cache.insert(1_u32, vec![1, 2, 3]).await;
        cache.insert(2_u32, vec![4, 5, 6]).await;

        assert_eq!(cache.get(&1_u32).await, Some(vec![1, 2, 3]));
        assert_eq!(cache.get(&2_u32).await, Some(vec![4, 5, 6]));
        assert_eq!(cache.get(&3_u32).await, None);
    }

    #[tokio::test]
    async fn test_overwrite_existing_key() {
        let cache = Cache::new(Duration::from_millis(100));

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        cache.insert("key1".to_string(), "value2".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value2".to_string())
        );
    }

    #[tokio::test]
    async fn test_purge_cycles_counter() {
        let cache = Cache::<String, String>::builder()
            .with_ttl(Duration::from_millis(100))
            .with_purge_cycles(10)
            .build();

        // Perform operations that should trigger purge cycle checking
        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(cache.purge_cycle_count.load(Ordering::Relaxed), 1);

        cache.get(&"key1".to_string()).await;
        assert_eq!(cache.purge_cycle_count.load(Ordering::Relaxed), 1);

        cache.insert("key2".to_string(), "value2".to_string()).await;
        assert_eq!(cache.purge_cycle_count.load(Ordering::Relaxed), 2);

        cache.poll_purge_once().await;
        assert_eq!(cache.purge_cycle_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_builder_default() {
        let cache = Cache::<String, String>::builder().build();

        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );
    }

    #[tokio::test]
    async fn test_cache_entry_creation_time() {
        let entry = CacheEntry::new("test_value".to_string());
        let created_at = entry.get_created_at();

        assert_eq!(entry.get_value(), "test_value".to_string());
        assert!(created_at.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let cache = Arc::new(Cache::new(Duration::from_millis(100)));

        let mut handles = vec![];

        // Spawn multiple tasks that access the cache concurrently
        for i in 0..10 {
            let cache_clone = Arc::clone(&cache);
            let handle = tokio::spawn(async move {
                let key = format!("key_{i}");
                let value = format!("value_{i}");

                cache_clone.insert(key.clone(), value.clone()).await;
                let retrieved = cache_clone.get(&key).await;
                assert_eq!(retrieved, Some(value));
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all entries are in the cache
        for i in 0..10 {
            let key = format!("key_{i}");
            let expected_value = format!("value_{i}");
            assert_eq!(cache.get(&key).await, Some(expected_value));
        }
    }
}
