//! Policy manager implementation that uses NATS to send policy requests
//! to a policy server.

use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

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

/// Encapsulates making requests for policy decisions, and receiving updated decisions
#[derive(Debug, Clone)]
pub struct NatsPolicyManager {
    nats: async_nats::Client,
    host_info: HostInfo,
    policy_topic: Option<String>,
    policy_timeout: Duration,
    decision_cache: Arc<RwLock<HashMap<RequestKey, Response>>>,
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
    ) -> anyhow::Result<Self> {
        const DEFAULT_POLICY_TIMEOUT: Duration = Duration::from_secs(1);

        let (policy_changes_abort, policy_changes_abort_reg) = AbortHandle::new_pair();

        let manager = NatsPolicyManager {
            nats: nats.clone(),
            host_info,
            policy_topic,
            policy_timeout: policy_timeout.unwrap_or(DEFAULT_POLICY_TIMEOUT),
            decision_cache: Arc::default(),
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
        if let Some(entry) = self.decision_cache.read().await.get(&cache_key) {
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
            .write()
            .await
            .insert(cache_key.clone(), decision.clone()); // cache policy decision
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

        let mut decision_cache = self.decision_cache.write().await;
        let request_to_key = self.request_to_key.read().await;

        if let Some(key) = request_to_key.get(&request_id) {
            decision_cache.insert(
                key.clone(),
                Response {
                    request_id: request_id.clone(),
                    permitted,
                    message,
                },
            );
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
