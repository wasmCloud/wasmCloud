use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures::{
    stream::{AbortHandle, Abortable},
    StreamExt,
};
use serde::{Deserialize, Serialize};
use tokio::spawn;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;

// NOTE: All requests will be v1 until the schema changes, at which point we can change the version
// per-request type
const POLICY_TYPE_VERSION: &str = "v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
/// Claims associated with a policy request, if embedded inside the component or provider
pub struct PolicyClaims {
    /// The public key of the component
    #[serde(rename = "publicKey")]
    pub public_key: String,
    /// The issuer key of the component
    pub issuer: String,
    /// The time the claims were signed
    #[serde(rename = "issuedAt")]
    pub issued_at: String,
    /// The time the claims expire, if any
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<u64>,
    /// Whether the claims have expired already. This is included in case the policy server is fulfilled by an actor, which cannot access the system clock
    pub expired: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
/// Relevant policy information for evaluating a component
pub struct ComponentInformation {
    /// The unique identifier of the component
    #[serde(rename = "componentId")]
    pub component_id: String,
    /// The image reference of the component
    #[serde(rename = "imageRef")]
    pub image_ref: String,
    /// The requested maximum number of concurrent instances for this component
    #[serde(rename = "maxInstances")]
    pub max_instances: u32,
    /// Annotations associated with the component
    pub annotations: BTreeMap<String, String>,
    /// Claims, if embedded, within the component
    pub claims: Option<PolicyClaims>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
/// Relevant policy information for evaluating a provider
pub struct ProviderInformation {
    /// The unique identifier of the provider
    #[serde(rename = "providerId")]
    pub provider_id: String,
    /// The image reference of the provider
    #[serde(rename = "imageRef")]
    pub image_ref: String,
    /// Annotations associated with the provider
    pub annotations: BTreeMap<String, String>,
    /// Claims, if embedded, within the provider
    pub claims: Option<PolicyClaims>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
/// A request to invoke a component function
pub struct PerformInvocationRequest {
    /// The interface of the invocation
    pub interface: String,
    /// The function of the invocation
    pub function: String,
    /// Target of the invocation
    pub target: ComponentInformation,
}

/// Relevant information about the host that is receiving the invocation, or starting the actor or provider
#[derive(Clone, Debug, Serialize)]
pub struct HostInfo {
    /// The public key ID of the host
    #[serde(rename = "publicKey")]
    pub public_key: String,
    /// The name of the lattice the host is running in
    #[serde(rename = "lattice")]
    pub lattice: String,
    /// The labels associated with the host
    pub labels: HashMap<String, String>,
}

/// The action being requested
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Hash)]
pub enum RequestKind {
    /// The host is checking whether it may invoke the target component
    #[serde(rename = "performInvocation")]
    PerformInvocation,
    /// The host is checking whether it may start the target component
    #[serde(rename = "startComponent")]
    StartComponent,
    /// The host is checking whether it may start the target provider
    #[serde(rename = "startProvider")]
    StartProvider,
    /// An unknown or unsupported request type
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Hash)]
#[serde(untagged)]
/// The body of a policy request, typed by the request kind
pub enum RequestBody {
    /// A request to invoke a function on a component
    PerformInvocation(PerformInvocationRequest),
    /// A request to start a component on a host
    StartComponent(ComponentInformation),
    /// A request to start a provider on a host
    StartProvider(ProviderInformation),
    /// Request body has an unknown type
    Unknown,
}

impl From<&RequestBody> for RequestKey {
    fn from(val: &RequestBody) -> RequestKey {
        match val {
            RequestBody::StartComponent(ref req) => RequestKey {
                kind: RequestKind::StartComponent,
                cache_key: format!("{}_{}", req.component_id, req.image_ref),
            },
            RequestBody::StartProvider(ref req) => RequestKey {
                kind: RequestKind::StartProvider,
                cache_key: format!("{}_{}", req.provider_id, req.image_ref),
            },
            RequestBody::PerformInvocation(ref req) => RequestKey {
                kind: RequestKind::PerformInvocation,
                cache_key: format!(
                    "{}_{}_{}_{}",
                    req.target.component_id, req.target.image_ref, req.interface, req.function
                ),
            },
            RequestBody::Unknown => RequestKey {
                kind: RequestKind::Unknown,
                cache_key: "".to_string(),
            },
        }
    }
}

/// A request for a policy decision
#[derive(Serialize)]
struct Request {
    /// A unique request id. This value is returned in the response
    #[serde(rename = "requestId")]
    #[allow(clippy::struct_field_names)]
    request_id: String,
    /// The kind of policy request being made
    kind: RequestKind,
    /// The version of the policy request body
    version: String,
    /// The policy request body
    request: RequestBody,
    /// Information about the host making the request
    host: HostInfo,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct RequestKey {
    /// The kind of request being made
    kind: RequestKind,
    /// Information about this request combined to form a unique string.
    /// For example, a StartComponent request can be uniquely cached based
    /// on the component_id and image_ref, so this cache_key is a concatenation
    /// of those values
    cache_key: String,
}

/// A policy decision response
#[derive(Clone, Debug, Deserialize)]
pub struct Response {
    /// The request id copied from the request
    #[serde(rename = "requestId")]
    pub request_id: String,
    /// Whether the request is permitted
    pub permitted: bool,
    /// An optional error explaining why the request was denied. Suitable for logging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn is_expired(expires: u64) -> bool {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards") // SAFETY: now() should always be greater than UNIX_EPOCH
        .as_secs()
        > expires
}

impl From<&jwt::Claims<jwt::Actor>> for PolicyClaims {
    fn from(claims: &jwt::Claims<jwt::Actor>) -> Self {
        PolicyClaims {
            public_key: claims.subject.to_string(),
            issuer: claims.issuer.to_string(),
            issued_at: claims.issued_at.to_string(),
            expires_at: claims.expires,
            expired: claims.expires.map(is_expired).unwrap_or_default(),
        }
    }
}

impl From<&jwt::Claims<jwt::CapabilityProvider>> for PolicyClaims {
    fn from(claims: &jwt::Claims<jwt::CapabilityProvider>) -> Self {
        PolicyClaims {
            public_key: claims.subject.to_string(),
            issuer: claims.issuer.to_string(),
            issued_at: claims.issued_at.to_string(),
            expires_at: claims.expires,
            expired: claims.expires.map(is_expired).unwrap_or_default(),
        }
    }
}

/// Encapsulates making requests for policy decisions, and receiving updated decisions
#[derive(Debug)]
pub struct Manager {
    nats: async_nats::Client,
    host_info: HostInfo,
    policy_topic: Option<String>,
    policy_timeout: Duration,
    decision_cache: Arc<RwLock<HashMap<RequestKey, Response>>>,
    request_to_key: Arc<RwLock<HashMap<String, RequestKey>>>,
    /// An abort handle for the policy changes subscription
    pub policy_changes: AbortHandle,
}

impl Manager {
    /// Construct a new policy manager. Can fail if policy_changes_topic is set but we fail to subscribe to it
    #[instrument(skip(nats))]
    pub async fn new(
        nats: async_nats::Client,
        host_info: HostInfo,
        policy_topic: Option<String>,
        policy_timeout: Option<Duration>,
        policy_changes_topic: Option<String>,
    ) -> anyhow::Result<Arc<Self>> {
        const DEFAULT_POLICY_TIMEOUT: Duration = Duration::from_secs(1);

        let (policy_changes_abort, policy_changes_abort_reg) = AbortHandle::new_pair();

        let manager = Manager {
            nats: nats.clone(),
            host_info,
            policy_topic,
            policy_timeout: policy_timeout.unwrap_or(DEFAULT_POLICY_TIMEOUT),
            decision_cache: Arc::default(),
            request_to_key: Arc::default(),
            policy_changes: policy_changes_abort,
        };
        let manager = Arc::new(manager);

        if let Some(policy_changes_topic) = policy_changes_topic {
            let policy_changes = nats
                .subscribe(policy_changes_topic)
                .await
                .context("failed to subscribe to policy changes")?;

            let _policy_changes = spawn({
                let manager = Arc::clone(&manager);
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

    #[instrument(level = "trace", skip_all)]
    /// Use the policy manager to evaluate whether a component may be started
    pub async fn evaluate_start_component(
        &self,
        component_id: impl AsRef<str>,
        image_ref: impl AsRef<str>,
        max_instances: u32,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::Actor>>,
    ) -> anyhow::Result<Response> {
        let request = ComponentInformation {
            component_id: component_id.as_ref().to_string(),
            image_ref: image_ref.as_ref().to_string(),
            max_instances,
            annotations: annotations.clone(),
            claims: claims.map(PolicyClaims::from),
        };
        self.evaluate_action(RequestBody::StartComponent(request))
            .await
    }

    /// Use the policy manager to evaluate whether a provider may be started
    #[instrument(level = "trace", skip_all)]
    pub async fn evaluate_start_provider(
        &self,
        provider_id: impl AsRef<str>,
        provider_ref: impl AsRef<str>,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::CapabilityProvider>>,
    ) -> anyhow::Result<Response> {
        let request = ProviderInformation {
            provider_id: provider_id.as_ref().to_string(),
            image_ref: provider_ref.as_ref().to_string(),
            annotations: annotations.clone(),
            claims: claims.map(PolicyClaims::from),
        };
        self.evaluate_action(RequestBody::StartProvider(request))
            .await
    }

    /// Use the policy manager to evaluate whether a component may be invoked
    #[instrument(level = "trace", skip_all)]
    pub async fn evaluate_perform_invocation(
        &self,
        component_id: impl AsRef<str>,
        image_ref: impl AsRef<str>,
        annotations: &BTreeMap<String, String>,
        claims: Option<&jwt::Claims<jwt::Actor>>,
        interface: String,
        function: String,
    ) -> anyhow::Result<Response> {
        let request = PerformInvocationRequest {
            interface,
            function,
            target: ComponentInformation {
                component_id: component_id.as_ref().to_string(),
                image_ref: image_ref.as_ref().to_string(),
                max_instances: 0,
                annotations: annotations.clone(),
                claims: claims.map(PolicyClaims::from),
            },
        };
        self.evaluate_action(RequestBody::PerformInvocation(request))
            .await
    }

    /// Sends a policy request to the policy server and caches the response
    #[instrument(level = "trace", skip_all)]
    pub async fn evaluate_action(&self, request: RequestBody) -> anyhow::Result<Response> {
        let Some(policy_topic) = self.policy_topic.clone() else {
            // Ensure we short-circuit and allow the request if no policy topic is configured
            return Ok(Response {
                request_id: "".to_string(),
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
