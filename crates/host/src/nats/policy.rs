//! Policy manager implementation that uses NATS to send policy requests
//! to a policy server.

use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use futures::{
    stream::{AbortHandle, Abortable},
    StreamExt,
};
use tokio::spawn;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;

use crate::policy::{
    ComponentInformation, HostInfo, PerformInvocationRequest, PolicyClaims, PolicyManager,
    ProviderInformation, Request, RequestBody, RequestKey, RequestKind, Response,
    POLICY_TYPE_VERSION,
};

#[derive(Debug)]
struct CacheEntry<T>
where
    T: Clone,
{
    value: T,
    last_updated: Instant,
}

impl<T> CacheEntry<T>
where
    T: Clone,
{
    fn new(value: T) -> Self {
        Self {
            value,
            last_updated: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
struct DecisionCache<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    cache: Arc<RwLock<HashMap<K, CacheEntry<V>>>>,
    ttl: Duration,
    _cleanup_handle: tokio::task::AbortHandle,
}

impl<K, V> DecisionCache<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn new(ttl: Duration) -> Self {
        Self::with_cleanup(ttl, ttl / 2)
    }

    fn with_cleanup(ttl: Duration, interval: Duration) -> Self {
        let cache = Arc::new(RwLock::new(HashMap::<K, CacheEntry<V>>::new()));
        let cleanup_cache = Arc::clone(&cache);
        let cleanup_handle = spawn(async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                cleanup_cache
                    .write()
                    .await
                    .retain(|_, entry| entry.last_updated.elapsed() < ttl);
            }
        });
        Self {
            cache,
            ttl,
            _cleanup_handle: cleanup_handle.abort_handle(),
        }
    }

    async fn get(&self, cache_key: &K) -> Option<V> {
        self.cache
            .read()
            .await
            .get(cache_key)
            .filter(|entry| entry.last_updated.elapsed() < self.ttl)
            .map(|entry| entry.value.clone())
    }

    async fn insert(&self, cache_key: K, cache_value: V) {
        self.cache
            .write()
            .await
            .insert(cache_key, CacheEntry::new(cache_value));
    }

    async fn clear_ttl(&self) {
        self.cache
            .write()
            .await
            .retain(|_, entry| entry.last_updated.elapsed() < self.ttl);
    }
}

impl<K, V> Drop for DecisionCache<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        self._cleanup_handle.abort();
    }
}

impl<K, V> Default for DecisionCache<K, V>
where
    K: std::hash::Hash + Eq + std::fmt::Debug + Send + Sync + 'static,
    V: std::fmt::Debug + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

/// Encapsulates making requests for policy decisions, and receiving updated decisions
#[derive(Debug, Clone)]
pub struct NatsPolicyManager {
    nats: async_nats::Client,
    host_info: HostInfo,
    policy_topic: Option<String>,
    policy_timeout: Duration,
    decision_cache: DecisionCache<RequestKey, Response>,
    request_to_key: Arc<RwLock<HashMap<String, RequestKey>>>,
    /// An abort handle for the policy changes subscription
    pub policy_changes: AbortHandle,
}

impl NatsPolicyManager {
    /// Construct a new policy manager. Can fail if policy_changes_topic is set but we fail to subscribe to it
    #[instrument(skip(nats))]
    pub async fn new(
        nats: async_nats::Client,
        host_info: HostInfo,
        policy_topic: Option<String>,
        policy_timeout: Option<Duration>,
        policy_changes_topic: Option<String>,
        decision_cache_ttl: Option<Duration>,
    ) -> anyhow::Result<Self> {
        const DEFAULT_POLICY_TIMEOUT: Duration = Duration::from_secs(1);

        let (policy_changes_abort, policy_changes_abort_reg) = AbortHandle::new_pair();

        let decision_cache_ttl = decision_cache_ttl.unwrap_or(Duration::from_secs(60));

        let manager = NatsPolicyManager {
            nats: nats.clone(),
            host_info,
            policy_topic,
            policy_timeout: policy_timeout.unwrap_or(DEFAULT_POLICY_TIMEOUT),
            decision_cache: DecisionCache::new(decision_cache_ttl),
            request_to_key: Arc::default(),
            policy_changes: policy_changes_abort,
        };

        if let Some(policy_changes_topic) = policy_changes_topic {
            let policy_changes = nats
                .subscribe(policy_changes_topic)
                .await
                .context("failed to subscribe to policy changes")?;

            let _policy_changes = spawn({
                let manager = Arc::new(manager.clone());
                Abortable::new(policy_changes, policy_changes_abort_reg).for_each(move |msg| {
                    let manager = Arc::clone(&manager);
                    async move {
                        if let Err(e) = manager.override_decision(msg).await {
                            error!("failed to process policy decision override: {}", e);
                        }
                    }
                })
            });
        }

        Ok(manager)
    }

    /// Sends a policy request to the policy server and caches the response
    #[instrument(level = "trace", skip_all)]
    pub async fn evaluate_action(&self, request: RequestBody) -> anyhow::Result<Response> {
        let Some(policy_topic) = self.policy_topic.clone() else {
            // Ensure we short-circuit and allow the request if no policy topic is configured
            return Ok(Response {
                request_id: String::new(),
                permitted: true,
                message: None,
            });
        };

        let kind = match request {
            RequestBody::StartComponent(_) => RequestKind::StartComponent,
            RequestBody::StartProvider(_) => RequestKind::StartProvider,
            RequestBody::PerformInvocation(_) => RequestKind::PerformInvocation,
            RequestBody::Unknown => RequestKind::Unknown,
        };
        let cache_key = (&request).into();
        if let Some(entry) = self.decision_cache.get(&cache_key).await {
            trace!(?cache_key, ?entry, "using cached policy decision");
            return Ok(entry.clone());
        }

        let request_id = Uuid::from_u128(Ulid::new().into()).to_string();
        trace!(?cache_key, "requesting policy decision");
        let payload = serde_json::to_vec(&Request {
            request_id: request_id.clone(),
            request,
            kind,
            version: POLICY_TYPE_VERSION.to_string(),
            host: self.host_info.clone(),
        })
        .context("failed to serialize policy request")?;
        let request = async_nats::Request::new()
            .payload(payload.into())
            .timeout(Some(self.policy_timeout));
        let res = self
            .nats
            .send_request(policy_topic, request)
            .await
            .context("policy request failed")?;
        let decision = serde_json::from_slice::<Response>(&res.payload)
            .context("failed to deserialize policy response")?;

        self.decision_cache
            .insert(cache_key.clone(), decision.clone()).await; // cache policy decision
        self.request_to_key
            .write()
            .await
            .insert(request_id, cache_key); // cache request id -> decision key
        Ok(decision)
    }

    #[instrument(skip(self))]
    async fn override_decision(&self, msg: async_nats::Message) -> anyhow::Result<()> {
        let Response {
            request_id,
            permitted,
            message,
        } = serde_json::from_slice(&msg.payload)
            .context("failed to deserialize policy decision override")?;

        debug!(request_id, "received policy decision override");

        let request_to_key = self.request_to_key.read().await;

        if let Some(key) = request_to_key.get(&request_id) {
            self.decision_cache.insert(
                key.clone(),
                Response {
                    request_id: request_id.clone(),
                    permitted,
                    message,
                },
            ).await;
        } else {
            warn!(
                request_id,
                "received policy decision override for unknown request id"
            );
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl PolicyManager for NatsPolicyManager {
    #[instrument(level = "trace", skip_all)]
    /// Use the policy manager to evaluate whether a component may be started
    async fn evaluate_start_component(
        &self,
        component_id: &str,
        image_ref: &str,
        max_instances: u32,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::Component>>,
    ) -> anyhow::Result<Response> {
        let request = ComponentInformation {
            component_id: component_id.to_string(),
            image_ref: image_ref.to_string(),
            max_instances,
            annotations: annotations.clone(),
            claims: claims.map(PolicyClaims::from),
        };
        self.evaluate_action(RequestBody::StartComponent(request))
            .await
    }

    /// Use the policy manager to evaluate whether a provider may be started
    #[instrument(level = "trace", skip_all)]
    async fn evaluate_start_provider(
        &self,
        provider_id: &str,
        provider_ref: &str,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::CapabilityProvider>>,
    ) -> anyhow::Result<Response> {
        let request = ProviderInformation {
            provider_id: provider_id.to_string(),
            image_ref: provider_ref.to_string(),
            annotations: annotations.clone(),
            claims: claims.map(PolicyClaims::from),
        };
        self.evaluate_action(RequestBody::StartProvider(request))
            .await
    }

    /// Use the policy manager to evaluate whether a component may be invoked
    #[instrument(level = "trace", skip_all)]
    async fn evaluate_perform_invocation(
        &self,
        component_id: &str,
        image_ref: &str,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::Component>>,
        interface: String,
        function: String,
    ) -> anyhow::Result<Response> {
        let request = PerformInvocationRequest {
            interface,
            function,
            target: ComponentInformation {
                component_id: component_id.to_string(),
                image_ref: image_ref.to_string(),
                max_instances: 0,
                annotations: annotations.clone(),
                claims: claims.map(PolicyClaims::from),
            },
        };
        self.evaluate_action(RequestBody::PerformInvocation(request))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let cache = DecisionCache::<String, String>::new(Duration::from_secs(10));
        
        // Test insert and get
        cache.insert("key1".to_string(), "value1".to_string()).await;
        let result = cache.get(&"key1".to_string()).await;
        assert_eq!(result, Some("value1".to_string()));
        
        // Test non-existent key
        let result = cache.get(&"nonexistent".to_string()).await;
        assert!(result.is_none());
        
        // Test multiple keys
        cache.insert("key2".to_string(), "value2".to_string()).await;
        cache.insert("key3".to_string(), "value3".to_string()).await;
        
        assert_eq!(cache.get(&"key1".to_string()).await, Some("value1".to_string()));
        assert_eq!(cache.get(&"key2".to_string()).await, Some("value2".to_string()));
        assert_eq!(cache.get(&"key3".to_string()).await, Some("value3".to_string()));
    }

    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let cache = DecisionCache::<String, String>::new(Duration::from_millis(100));
        
        cache.insert("key1".to_string(), "value1".to_string()).await;
        
        // Should exist immediately
        assert!(cache.get(&"key1".to_string()).await.is_some());
        
        // Wait for TTL to expire
        sleep(Duration::from_millis(150)).await;
        
        // Should be expired and return None
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }


    #[tokio::test]
    async fn test_cache_cleanup_task_abort() {
        let cache = DecisionCache::<String, String>::new(Duration::from_secs(1));
        
        // Insert some data
        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert!(cache.get(&"key1".to_string()).await.is_some());
        
        // Drop the cache (should abort cleanup task)
        drop(cache);
        
        // Test passes if no panic or hanging occurs
        // Give a moment for cleanup
        sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_cache_clear_ttl() {
        let cache = DecisionCache::<String, String>::new(Duration::from_millis(100));
        
        // Insert some data
        cache.insert("key1".to_string(), "value1".to_string()).await;
        cache.insert("key2".to_string(), "value2".to_string()).await;
        
        // Wait for expiration
        sleep(Duration::from_millis(150)).await;
        
        // Manually trigger cleanup
        cache.clear_ttl().await;
        
        // Both should be gone after manual cleanup
        assert!(cache.get(&"key1".to_string()).await.is_none());
        assert!(cache.get(&"key2".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_mixed_ttl() {
        let cache = DecisionCache::<String, String>::new(Duration::from_millis(200));
        
        // Insert first item
        cache.insert("key1".to_string(), "value1".to_string()).await;
        
        // Wait a bit
        sleep(Duration::from_millis(100)).await;
        
        // Insert second item
        cache.insert("key2".to_string(), "value2".to_string()).await;
        
        // Wait for first item to expire but not second
        sleep(Duration::from_millis(150)).await;
        
        // First should be expired, second should still exist
        assert!(cache.get(&"key1".to_string()).await.is_none());
        assert_eq!(cache.get(&"key2".to_string()).await, Some("value2".to_string()));
    }

    #[tokio::test]
    async fn test_cache_concurrent_access() {
        let cache = Arc::new(DecisionCache::<String, String>::new(Duration::from_secs(10)));
        let mut handles = vec![];

        // Spawn multiple tasks doing concurrent operations
        for i in 0..10 {
            let cache_clone = Arc::clone(&cache);
            let handle = tokio::spawn(async move {
                let key = format!("key{}", i);
                let value = format!("value{}", i);
                
                cache_clone.insert(key.clone(), value.clone()).await;
                
                // Verify we can read back what we wrote
                let result = cache_clone.get(&key).await;
                assert_eq!(result, Some(value));
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.expect("Task should complete successfully");
        }

        // Verify all keys are present
        for i in 0..10 {
            let key = format!("key{}", i);
            let expected_value = format!("value{}", i);
            assert_eq!(cache.get(&key).await, Some(expected_value));
        }
    }
}
