use crate::OciConfig;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nkeys::KeyPair;
use url::Url;
use wasmcloud_core::{logging::Level as LogLevel, OtelConfig};

/// wasmCloud Host configuration
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct Host {
    /// NATS URL to connect to for control interface connection
    pub ctl_nats_url: Url,
    /// Authentication JWT for control interface connection, must be specified with `ctl_seed`
    pub ctl_jwt: Option<String>,
    /// Authentication key pair for control interface connection, must be specified with `ctl_jwt`
    pub ctl_key: Option<Arc<KeyPair>>,
    /// Whether to require TLS for control interface connection
    pub ctl_tls: bool,
    /// The topic prefix to use for control interface subscriptions, defaults to `wasmbus.ctl`
    pub ctl_topic_prefix: String,
    /// NATS URL to connect to for component RPC
    pub rpc_nats_url: Url,
    /// Timeout period for all RPC calls
    pub rpc_timeout: Duration,
    /// Authentication JWT for RPC connection, must be specified with `rpc_seed`
    pub rpc_jwt: Option<String>,
    /// Authentication key pair for RPC connection, must be specified with `rpc_jwt`
    pub rpc_key: Option<Arc<KeyPair>>,
    /// Whether to require TLS for RPC connection
    pub rpc_tls: bool,
    /// The lattice the host belongs to
    pub lattice: Arc<str>,
    /// The domain to use for host Jetstream operations
    pub js_domain: Option<String>,
    /// Labels (key-value pairs) to add to the host
    pub labels: HashMap<String, String>,
    /// The server key pair used by this host to generate its public key
    pub host_key: Option<Arc<KeyPair>>,
    /// The amount of time to wait for a provider to gracefully shut down before terminating it
    pub provider_shutdown_delay: Option<Duration>,
    /// Configuration for downloading artifacts from OCI registries
    pub oci_opts: OciConfig,
    /// Whether to allow loading component or provider components from the filesystem
    pub allow_file_load: bool,
    /// Whether or not structured logging is enabled
    pub enable_structured_logging: bool,
    /// Log level to pass to capability providers to use. Should be parsed from a [`tracing::Level`]
    pub log_level: LogLevel,
    /// Whether to enable loading supplemental configuration
    pub config_service_enabled: bool,
    /// configuration for OpenTelemetry tracing
    pub otel_config: OtelConfig,
    /// configuration for wasmCloud policy service
    pub policy_service_config: PolicyService,
    /// The semver version of the host. This is used by a consumer of this crate to indicate the
    /// host version (which may differ from the crate version)
    pub version: String,
}

/// Configuration for wasmCloud policy service
#[derive(Clone, Debug, Default)]
pub struct PolicyService {
    /// The topic to request policy decisions on
    pub policy_topic: Option<String>,
    /// An optional topic to receive updated policy decisions on
    pub policy_changes_topic: Option<String>,
    /// The timeout for policy requests
    pub policy_timeout_ms: Option<Duration>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            ctl_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse control NATS URL"),
            ctl_jwt: None,
            ctl_key: None,
            ctl_tls: false,
            ctl_topic_prefix: "wasmbus.ctl".to_string(),
            rpc_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse RPC NATS URL"),
            rpc_timeout: Duration::from_millis(2000),
            rpc_jwt: None,
            rpc_key: None,
            rpc_tls: false,
            lattice: "default".into(),
            js_domain: None,
            labels: HashMap::default(),
            host_key: None,
            provider_shutdown_delay: None,
            oci_opts: OciConfig::default(),
            allow_file_load: false,
            enable_structured_logging: false,
            log_level: LogLevel::Info,
            config_service_enabled: false,
            otel_config: OtelConfig::default(),
            policy_service_config: PolicyService::default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}
