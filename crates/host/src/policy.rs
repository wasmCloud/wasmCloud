use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wascap::jwt;

// NOTE: All requests will be v1 until the schema changes, at which point we can change the version
// per-request type
pub(crate) const POLICY_TYPE_VERSION: &str = "v1";

/// A trait for evaluating policy decisions
#[async_trait::async_trait]
pub trait PolicyManager: Send + Sync {
    /// Evaluate whether a component may be started
    async fn evaluate_start_component(
        &self,
        _component_id: &str,
        _image_ref: &str,
        _max_instances: u32,
        _annotations: &BTreeMap<String, String>,
        _claims: Option<&jwt::Claims<jwt::Component>>,
    ) -> anyhow::Result<Response> {
        Ok(Response {
            request_id: Uuid::new_v4().to_string(),
            permitted: true,
            message: None,
        })
    }

    /// Evaluate whether a provider may be started
    async fn evaluate_start_provider(
        &self,
        _provider_id: &str,
        _provider_ref: &str,
        _annotations: &BTreeMap<String, String>,
        _claims: Option<&jwt::Claims<jwt::CapabilityProvider>>,
    ) -> anyhow::Result<Response> {
        Ok(Response {
            request_id: Uuid::new_v4().to_string(),
            permitted: true,
            message: None,
        })
    }

    /// Evaluate whether a component may perform an invocation
    async fn evaluate_perform_invocation(
        &self,
        _component_id: &str,
        _image_ref: &str,
        _annotations: &BTreeMap<String, String>,
        _claims: Option<&jwt::Claims<jwt::Component>>,
        _interface: String,
        _function: String,
    ) -> anyhow::Result<Response> {
        Ok(Response {
            request_id: Uuid::new_v4().to_string(),
            permitted: true,
            message: None,
        })
    }
}

/// A default policy manager that always returns true for all requests
/// This is used when no policy manager is configured
#[derive(Default)]
pub struct DefaultPolicyManager;
impl super::PolicyManager for DefaultPolicyManager {}

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
    /// Whether the claims have expired already. This is included in case the policy server is fulfilled by an component, which cannot access the system clock
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

/// Relevant information about the host that is receiving the invocation, or starting the component or provider
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
                cache_key: String::new(),
            },
        }
    }
}

/// A request for a policy decision
#[derive(Serialize)]
pub(crate) struct Request {
    /// A unique request id. This value is returned in the response
    #[serde(rename = "requestId")]
    #[allow(clippy::struct_field_names)]
    pub(crate) request_id: String,
    /// The kind of policy request being made
    pub(crate) kind: RequestKind,
    /// The version of the policy request body
    pub(crate) version: String,
    /// The policy request body
    pub(crate) request: RequestBody,
    /// Information about the host making the request
    pub(crate) host: HostInfo,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) struct RequestKey {
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

impl From<&jwt::Claims<jwt::Component>> for PolicyClaims {
    fn from(claims: &jwt::Claims<jwt::Component>) -> Self {
        PolicyClaims {
            public_key: claims.subject.to_string(),
            issuer: claims.issuer.to_string(),
            issued_at: claims.issued_at.to_string(),
            expires_at: claims.expires,
            expired: claims.expires.is_some_and(is_expired),
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
            expired: claims.expires.is_some_and(is_expired),
        }
    }
}
