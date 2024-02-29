use core::time::Duration;

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use futures::{
    stream::{AbortHandle, Abortable},
    StreamExt,
};
use serde::{Deserialize, Serialize, Serializer};
use tokio::spawn;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;

/// Relevant information about the actor or provider making an invocation. This struct is empty for
/// policy decisions related to starting actors or providers. All fields are optional for backwards-compatibility
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
// TODO: convert to an enum where variants use relevant fields, then remove the above comment about backwards-compatibility
pub struct RequestSource {
    /// The public key of the actor or provider
    #[serde(rename = "publicKey")]
    pub public_key: Option<String>,
    /// The contract ID of the provider, or None if the source is an actor
    #[serde(rename = "contractId")]
    pub contract_id: Option<String>,
    /// The link name of the provider, or None if the source is an actor
    #[serde(rename = "linkName")]
    pub link_name: Option<String>,
    /// The list of capabilities of the actor, or None if the source is a provider
    pub capabilities: Vec<String>,
    /// The issuer of the source's claims
    pub issuer: Option<String>,
    /// The time the claims were signed
    #[serde(rename = "issuedOn")]
    pub issued_on: Option<String>,
    /// The time the claims expire, if any
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<u64>,
    /// Whether the claims have expired already. This is included in case the policy server is fulfilled by an actor, which cannot access the system clock
    pub expired: bool,
}

/// Relevant information about the actor that is being invoked, or the actor or provider that is
/// being started. All fields are optional for backwards-compatibility
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Hash)]
// TODO: convert to an enum where variants use relevant fields, then remove the above comment about backwards-compatibility
pub struct RequestTarget {
    /// The public key of the actor or provider
    #[serde(rename = "publicKey")]
    pub public_key: Option<String>,
    /// The issuer of the target's claims
    pub issuer: Option<String>,
    /// The contract ID of the provider, or None if the target is an actor
    #[serde(rename = "contractId")]
    pub contract_id: Option<String>,
    /// The link name of the provider, or None if the target is an actor
    #[serde(rename = "linkName")]
    pub link_name: Option<String>,
}

/// Relevant information about the host that is receiving the invocation, or starting the actor or provider
#[derive(Clone, Debug, Serialize)]
pub struct HostInfo {
    /// The public key of the host
    #[serde(rename = "publicKey")]
    pub public_key: String,
    /// The ID of the lattice the host is running in
    #[serde(rename = "latticeId")]
    pub lattice_id: String,
    /// The labels associated with the host
    pub labels: HashMap<String, String>,
    /// The host's list of issuers it will accept invocations from
    #[serde(rename = "clusterIssuers")]
    pub cluster_issuers: Vec<String>,
}

/// The action being requested
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Hash)]
pub enum Action {
    /// The host is checking whether it may invoke the target actor
    #[serde(rename = "perform_invocation")]
    PerformInvocation,
    /// The host is checking whether it may start the target actor
    #[serde(rename = "start_actor")]
    StartActor,
    /// The host is checking whether it may start the target provider
    #[serde(rename = "start_provider")]
    StartProvider,
}

/// A request for a policy decision
#[derive(Serialize)]
struct Request {
    /// a unique request id. This value is returned in the response
    #[serde(rename = "requestId")]
    #[allow(clippy::struct_field_names)]
    request_id: String,
    // Use a custom serializer to handle the case where the source is None
    #[serde(serialize_with = "serialize_source")]
    source: Option<RequestSource>,
    target: RequestTarget,
    host: HostInfo,
    action: Action,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct RequestKey {
    source: RequestSource,
    target: RequestTarget,
    action: Action,
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
    pub message: Option<String>,
}

/// Policy services expect a source on all requests, even though no data is relevant for the start
/// actions. When source is None, we still serialize an (empty) object
fn serialize_source<S>(source: &Option<RequestSource>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match source {
        Some(source) => source.serialize(serializer),
        None => RequestSource::default().serialize(serializer),
    }
}

fn is_expired(expires: u64) -> bool {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards") // SAFETY: now() should always be greater than UNIX_EPOCH
        .as_secs()
        > expires
}

impl From<jwt::Claims<jwt::Actor>> for RequestSource {
    fn from(claims: jwt::Claims<jwt::Actor>) -> Self {
        RequestSource {
            public_key: Some(claims.subject),
            contract_id: None,
            link_name: None,
            capabilities: claims.metadata.and_then(|m| m.caps).unwrap_or_default(),
            issuer: Some(claims.issuer),
            issued_on: Some(claims.issued_at.to_string()),
            expires_at: claims.expires,
            expired: claims.expires.map(is_expired).unwrap_or_default(),
        }
    }
}

impl From<jwt::Claims<jwt::CapabilityProvider>> for RequestSource {
    fn from(claims: jwt::Claims<jwt::CapabilityProvider>) -> Self {
        RequestSource {
            public_key: Some(claims.subject),
            contract_id: claims.metadata.map(|m| m.capid),
            link_name: None, // Unfortunately, since claims don't include a link name, we can't populate this
            capabilities: vec![],
            issuer: Some(claims.issuer),
            issued_on: Some(claims.issued_at.to_string()),
            expires_at: claims.expires,
            expired: claims.expires.map(is_expired).unwrap_or_default(),
        }
    }
}

impl From<jwt::Claims<jwt::Actor>> for RequestTarget {
    fn from(claims: jwt::Claims<jwt::Actor>) -> Self {
        RequestTarget {
            public_key: Some(claims.subject),
            issuer: Some(claims.issuer),
            contract_id: None,
            link_name: None,
        }
    }
}

impl From<jwt::Claims<jwt::CapabilityProvider>> for RequestTarget {
    fn from(claims: jwt::Claims<jwt::CapabilityProvider>) -> Self {
        RequestTarget {
            public_key: Some(claims.subject),
            issuer: Some(claims.issuer),
            contract_id: claims.metadata.map(|m| m.capid),
            link_name: None, // Unfortunately, since claims don't include a link name, we can't populate this
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

    /// Constructs a
    #[instrument(level = "trace", skip_all)]
    pub async fn evaluate_action(
        &self,
        source: Option<RequestSource>,
        target: RequestTarget,
        action: Action,
    ) -> anyhow::Result<Response> {
        let cache_key = RequestKey {
            source: source.clone().unwrap_or_default(),
            target: target.clone(),
            action: action.clone(),
        };
        if let Some(entry) = self.decision_cache.read().await.get(&cache_key) {
            trace!(?cache_key, ?entry, "using cached policy decision");
            return Ok(entry.clone());
        }

        let request_id = Uuid::from_u128(Ulid::new().into()).to_string();
        let decision = if let Some(policy_topic) = self.policy_topic.clone() {
            trace!(?cache_key, "requesting policy decision");
            let payload = serde_json::to_vec(&Request {
                request_id: request_id.clone(),
                source,
                target,
                host: self.host_info.clone(),
                action,
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
            serde_json::from_slice::<Response>(&res.payload)
                .context("failed to deserialize policy response")?
        } else {
            trace!(
                ?cache_key,
                "no policy topic configured, defaulting to permitted"
            );
            // default to permitted if no policy topic is configured
            Response {
                request_id: request_id.clone(),
                permitted: true,
                message: None,
            }
        };
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
