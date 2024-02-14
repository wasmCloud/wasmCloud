#![warn(clippy::pedantic)]
#![forbid(clippy::unwrap_used)]

pub mod chunking;
pub mod logging;

use logging::Level;

use core::fmt;

use std::collections::HashMap;
use std::fmt::Display;

use anyhow::{anyhow, bail, ensure, Context};
use nkeys::{KeyPair, KeyPairType};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;
use wascap::prelude::Claims;

/// List of linked actors for a provider
pub type ActorLinks = Vec<LinkDefinition>;
pub type ClusterIssuerKey = String;
pub type ClusterIssuers = Vec<ClusterIssuerKey>;

/// Name of an established link
pub type LinkName = String;

/// The ID of a wRPC-compliant component, which can take many forms:
/// - Signed actor public key
/// - Opaque string (representing possibly a target group)
/// - ...
type ComponentId = String;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckRequest {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HealthCheckResponse {
    /// A flag that indicates the the actor is healthy
    #[serde(default)]
    pub healthy: bool,
    /// A message containing additional information about the actors health
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// initialization data for a capability provider
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HostData {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub lattice_rpc_prefix: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub lattice_rpc_user_jwt: String,
    #[serde(default)]
    pub lattice_rpc_user_seed: String,
    #[serde(default)]
    pub lattice_rpc_url: String,
    #[serde(default)]
    pub provider_key: String,
    #[serde(default)]
    pub invocation_seed: String,
    #[serde(
        serialize_with = "serialize_wit_map",
        deserialize_with = "deserialize_wit_map"
    )]
    pub env_values: HostEnvValues,
    #[serde(default)]
    pub instance_id: String,
    /// initial list of links for provider
    pub link_definitions: ActorLinks,
    /// list of cluster issuers
    pub cluster_issuers: ClusterIssuers,
    /// Optional configuration JSON sent to a given link name of a provider
    /// without an actor context
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_json: Option<String>,
    /// Host-wide default RPC timeout for rpc messages, in milliseconds.  Defaults to 2000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_rpc_timeout_ms: Option<u64>,
    /// True if structured logging is enabled for the host. Providers should use the same setting as the host.
    #[serde(default)]
    pub structured_logging: bool,
    /// The log level providers should log at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<Level>,
    pub otel_config: OtelConfig,
}

/// Environment settings for initializing a capability provider
pub type HostEnvValues = WitMap<String>;

/// Configuration values for Open Telemetry
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OtelConfig {
    /// OTEL_TRACES_EXPORTER https://opentelemetry.io/docs/concepts/sdk-configuration/general-sdk-configuration/#otel_traces_exporter
    pub traces_exporter: Option<String>,
    /// OTEL_EXPORTER_OTLP_ENDPOINT https://opentelemetry.io/docs/concepts/sdk-configuration/otlp-exporter-configuration/#otel_exporter_otlp_endpoint
    pub exporter_otlp_endpoint: Option<String>,
}

pub fn invocation_hash(
    target_url: impl AsRef<str>,
    origin_url: impl AsRef<str>,
    op: impl AsRef<str>,
    msg: impl AsRef<[u8]>,
) -> String {
    let mut hash = Sha256::default();
    hash.update(origin_url.as_ref());
    hash.update(target_url.as_ref());
    hash.update(op.as_ref());
    hash.update(msg.as_ref());
    hex::encode_upper(hash.finalize())
}

/// RPC message to capability provider
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Invocation {
    pub origin: WasmCloudEntity,
    /// Target to which the invocation should be sent (ex. actor public key, opaque string)
    pub target: WrpcTarget,
    #[serde(default)]
    pub operation: String,
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub encoded_claims: String,
    #[serde(default)]
    pub host_id: String,
    /// total message size
    pub content_length: u64,
    /// Open Telemetry tracing support
    #[serde(rename = "traceContext")]
    #[serde(
        default,
        serialize_with = "serialize_wit_map",
        deserialize_with = "deserialize_wit_map"
    )]
    pub trace_context: TraceContext,
}

impl Invocation {
    /// Creates a new invocation. All invocations are signed with the cluster key as a way
    /// of preventing them from being forged over the network when connected to a lattice, and
    /// allows hosts to validate that the invocation is coming from a trusted source.
    ///
    /// # Arguments
    /// * `links` - a map of package name to target name to entity, used internally to disambiguate
    ///            between multiple links to the same provider or for actor-to-actor calls.
    /// * `cluster_key` - the cluster key used to sign the invocation
    /// * `host_key` - the host key of the host that is creating the invocation
    /// * `origin` - the origin of the invocation
    /// * `target` - the target of the invocation
    /// * `operation` - the operation being invoked
    /// * `msg` - the raw bytes of the invocation
    #[allow(clippy::missing_errors_doc)] // TODO: Document errors
    pub fn new(
        cluster_key: &KeyPair,
        host_key: &KeyPair,
        origin: WasmCloudEntity,
        target: WrpcTarget,
        operation: impl Into<String>,
        msg: Vec<u8>,
        trace_context: TraceContext,
    ) -> anyhow::Result<Invocation> {
        let operation = operation.into();
        let (_, operation) = operation
            .rsplit_once('/')
            .context("failed to parse operation")?;
        // TODO: Support per-interface links
        let id = Uuid::from_u128(Ulid::new().into()).to_string();
        // todo(vadossi-cosmonic): WasmCloudEntity had richer information here
        // for building `target_url`, double check if we are fine with just link name and op
        // which we expect to be WIT-ified
        let target_url = target.to_string();
        let claims = jwt::Claims::<jwt::Invocation>::new(
            cluster_key.public_key(),
            id.to_string(),
            &target_url,
            &origin.url(),
            &invocation_hash(&target_url, origin.url(), operation, &msg),
        );
        let encoded_claims = claims
            .encode(cluster_key)
            .context("failed to encode claims")?;

        let operation = operation.to_string();
        Ok(Invocation {
            content_length: msg.len() as _,
            origin,
            target,
            operation,
            msg,
            id,
            encoded_claims,
            host_id: host_key.public_key(),
            trace_context,
        })
    }

    /// A fully-qualified URL indicating the origin of the invocation
    #[must_use]
    pub fn origin_url(&self) -> String {
        self.origin.url()
    }

    /// A fully-qualified URL indicating the target of the invocation
    #[must_use]
    pub fn target_url(&self) -> String {
        self.target.to_string()
    }

    /// The hash of the invocation's target, origin, and raw bytes
    #[must_use]
    pub fn hash(&self) -> String {
        invocation_hash(
            self.target_url(),
            self.origin_url(),
            &self.operation,
            &self.msg,
        )
    }

    /// Validates the current invocation to ensure that the invocation claims have
    /// not been forged, are not expired, etc
    #[allow(clippy::missing_errors_doc)] // TODO: Document errors
    pub fn validate_antiforgery(&self, valid_issuers: &[String]) -> anyhow::Result<()> {
        match KeyPair::from_public_key(&self.host_id) {
            Ok(kp) if kp.key_pair_type() == KeyPairType::Server => (),
            _ => bail!("invalid host ID on invocation: '{}'", self.host_id),
        }

        let token_validation =
            jwt::validate_token::<wascap::prelude::Invocation>(&self.encoded_claims)
                .map_err(|e| anyhow!(e))?;
        ensure!(!token_validation.expired, "invocation claims token expired");
        ensure!(
            !token_validation.cannot_use_yet,
            "attempt to use invocation before claims token allows"
        );
        ensure!(
            token_validation.signature_valid,
            "invocation claims signature invalid"
        );

        let claims = Claims::<wascap::prelude::Invocation>::decode(&self.encoded_claims)
            .map_err(|e| anyhow!(e))?;
        ensure!(
            valid_issuers.contains(&claims.issuer),
            "issuer of this invocation is not among the list of valid issuers"
        );

        let inv_claims = claims
            .metadata
            .context("no wascap metadata found on claims")?;
        ensure!(
            inv_claims.target_url == self.target_url(),
            "invocation claims and invocation target URL do not match"
        );
        ensure!(
            inv_claims.origin_url == self.origin_url(),
            "invocation claims and invocation origin URL do not match"
        );

        // Don't perform the hash validity test when the body has been externalized
        // via object store. This is an optimization that helps us not have to run
        // through the same set of bytes twice. The object store internals have their
        // own hash mechanisms so we'll know the chunked bytes haven't been manipulated
        if !self.msg.is_empty() && inv_claims.invocation_hash != self.hash() {
            bail!(
                "invocation hash does not match signed claims hash ({} / {})",
                inv_claims.invocation_hash,
                self.hash()
            );
        }

        Ok(())
    }
}

/// Response to an invocation
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InvocationResponse {
    /// serialize response message
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub msg: Vec<u8>,
    /// id connecting this response to the invocation
    #[serde(default)]
    pub invocation_id: String,
    /// optional error message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// total message size
    pub content_length: u64,
    #[serde(rename = "traceContext")]
    #[serde(
        default,
        serialize_with = "serialize_wit_map",
        deserialize_with = "deserialize_wit_map"
    )]
    pub trace_context: TraceContext,
}

/// Link definition for binding actor to provider
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LinkDefinition {
    /// actor public key
    #[serde(default)]
    pub actor_id: String,
    /// provider public key
    #[serde(default)]
    pub provider_id: String,
    /// link name
    #[serde(default)]
    pub link_name: String,
    /// contract id
    #[serde(default)]
    pub contract_id: String,
    #[serde(
        serialize_with = "serialize_wit_map",
        deserialize_with = "deserialize_wit_map"
    )]
    pub values: LinkSettings,
}
/// Settings associated with an actor-provider link
pub type LinkSettings = WitMap<String>;

/// Environment settings for initializing a capability provider
pub type TraceContext = WitMap<String>;

// TODO(thomastaylor312): We should probably make the an enum instead of Actor and Provider, but the
// current RPC protocol doesn't support that. Before we fully release, we should consider changing
// this
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WasmCloudEntity {
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub contract_id: String,
}

impl WasmCloudEntity {
    /// The URL of the entity
    #[must_use]
    pub fn url(&self) -> String {
        if self.public_key.to_uppercase().starts_with('M') {
            format!("wasmbus://{}", self.public_key)
        } else {
            format!(
                "wasmbus://{}/{}/{}",
                self.contract_id
                    .replace(':', "/")
                    .replace(' ', "_")
                    .to_lowercase(),
                self.link_name.replace(' ', "_").to_lowercase(),
                self.public_key
            )
        }
    }

    /// Returns true if this entity refers to an actor
    #[must_use]
    pub fn is_actor(&self) -> bool {
        self.link_name.is_empty() || self.contract_id.is_empty()
    }

    /// Returns true if this entity refers to a provider
    #[must_use]
    pub fn is_provider(&self) -> bool {
        !self.is_actor()
    }
}

impl fmt::Display for WasmCloudEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = self.url();
        write!(f, "{url}")
    }
}

// todo(vadossi-cosmonic): duplicated from the one in builtins
/// Call target identifier
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum TargetInterface {
    /// `wasi:blobstore/blobstore`
    WasiBlobstoreBlobstore,
    /// `wasi:http/outgoing-handler`
    WasiHttpOutgoingHandler,
    /// `wasi:keyvalue/atomic`
    WasiKeyvalueAtomic,
    /// `wasi:keyvalue/eventual`
    WasiKeyvalueEventual,
    /// `wasi:logging/logging`
    WasiLoggingLogging,
    /// `wasmcloud:messaging/consumer`
    WasmcloudMessagingConsumer,
    /// Custom interface
    Custom {
        /// Package namespace
        namespace: String,
        /// Package name
        package: String,
        /// Interface name
        interface: String,
    },
}

// todo(vadossi-cosmonic): this default doesn't make much sense
impl Default for TargetInterface {
    fn default() -> Self {
        Self::Custom {
            namespace: String::default(),
            package: String::default(),
            interface: String::default(),
        }
    }
}

impl TargetInterface {
    /// Returns the 3-tuple of (namespace, package, interface) for this interface
    #[must_use]
    pub fn as_parts(&self) -> (&str, &str, &str) {
        match self {
            Self::WasiBlobstoreBlobstore => ("wasi", "blobstore", "blobstore"),
            Self::WasiHttpOutgoingHandler => ("wasi", "http", "outgoing-handler"),
            Self::WasiKeyvalueAtomic => ("wasi", "keyvalue", "atomic"),
            Self::WasiKeyvalueEventual => ("wasi", "keyvalue", "eventual"),
            Self::WasiLoggingLogging => ("wasi", "logging", "logging"),
            Self::WasmcloudMessagingConsumer => ("wasmcloud", "messaging", "consumer"),
            Self::Custom {
                namespace,
                package,
                interface,
            } => (namespace, package, interface),
        }
    }

    /// Build a [`TargetInterface`] from constituent parts
    #[must_use]
    pub fn from_parts(parts: (&str, &str, &str)) -> Self {
        match parts {
            ("wasi", "blobstore", "blobstore") => Self::WasiBlobstoreBlobstore,
            ("wasi", "http", "outgoing-handler") => Self::WasiHttpOutgoingHandler,
            ("wasi", "keyvalue", "atomic") => Self::WasiKeyvalueAtomic,
            ("wasi", "keyvalue", "eventual") => Self::WasiKeyvalueEventual,
            ("wasi", "logging", "logging") => Self::WasiLoggingLogging,
            ("wasmcloud", "messaging", "consumer") => Self::WasmcloudMessagingConsumer,
            (ns, pkg, iface) => Self::Custom {
                namespace: ns.into(),
                package: pkg.into(),
                interface: iface.into(),
            },
        }
    }

    /// Build a target interface, given a properly formatted operation
    ///
    /// # Errors
    ///
    /// Returns `Err` if the operation is not of the form "<package>:<ns>/<interface>.<function>"
    pub fn from_operation(operation: impl AsRef<str>) -> anyhow::Result<Self> {
        let operation = operation.as_ref();
        let (ns_and_pkg, interface_and_func) = operation
            .rsplit_once('/')
            .context("failed to parse operation")?;
        let (interface, _) = interface_and_func
            .split_once('.')
            .context("interface and function should be specified")?;
        let (wit_ns, wit_pkg) = ns_and_pkg
            .rsplit_once(':')
            .context("failed to parse operation for WIT ns/pkg")?;
        Ok(TargetInterface::from_parts((wit_ns, wit_pkg, interface)))
    }
}

// todo(vadossi-cosmonic): duplicated from the one in builtins
#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Interface target to be invoked over `wRPC`
pub struct WrpcTarget {
    /// wRPC component routing identifier
    pub id: ComponentId,
    /// wRPC component interface
    pub interface: TargetInterface,
    /// Link name used to resolve the target
    pub link_name: String,
}

impl Display for WrpcTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (wit_ns, wit_pkg, wit_fn) = self.interface.as_parts();
        let link_name = &self.link_name;
        let id = &self.id;
        write!(f, "{link_name}/{id}/{wit_ns}:{wit_pkg}/{wit_fn}")
    }
}

pub trait HealthCheck {
    // This might not work with codegen and we'll have to impl
    fn health_request(&self) -> HealthCheckResponse;
}

// I don't know if these would be generated or if we'd just include them in the library and then use them in the generated code, but they work around the lack of a map type in wit

/// Representation of maps (AKA associative arrays) that are usable from WIT
///
/// This representation is required because WIT does not natively
/// have support for a map type, so we must use a list of tuples
pub type WitMap<T> = Vec<(String, T)>;

fn serialize_wit_map<S: Serializer, T>(map: &WitMap<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Serialize,
{
    let mut seq = serializer.serialize_map(Some(map.len()))?;
    for (key, val) in map {
        seq.serialize_entry(key, val)?;
    }
    seq.end()
}

fn deserialize_wit_map<'de, D: serde::Deserializer<'de>, T>(
    deserializer: D,
) -> Result<WitMap<T>, D::Error>
where
    T: Deserialize<'de>,
{
    let values = HashMap::<String, T>::deserialize(deserializer)?;
    Ok(values.into_iter().collect())
}
