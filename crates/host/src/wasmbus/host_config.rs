use crate::OciConfig;

use core::net::SocketAddr;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nkeys::KeyPair;
use url::Url;
use wasmcloud_core::{logging::Level as LogLevel, OtelConfig};
use wasmcloud_runtime::{
    DEFAULT_MAX_CORE_INSTANCES_PER_COMPONENT, MAX_COMPONENTS, MAX_COMPONENT_SIZE, MAX_LINEAR_MEMORY,
};

use crate::wasmbus::experimental::Features;

/// wasmCloud Host configuration
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct Host {
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
    pub host_key: Arc<KeyPair>,
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
    /// The semver version of the host. This is used by a consumer of this crate to indicate the
    /// host version (which may differ from the crate version)
    pub version: String,
    /// The maximum execution time for a component instance
    pub max_execution_time: Duration,
    /// The maximum linear memory that a component instance can allocate
    pub max_linear_memory: u64,
    /// The maximum size of a component binary that can be loaded
    pub max_component_size: u64,
    /// The maximum number of components that can be run simultaneously
    pub max_components: u32,
    /// The maximum number of core instances that are allowed in a given component
    pub max_core_instances_per_component: u32,
    /// The interval at which the Host will send heartbeats
    pub heartbeat_interval: Option<Duration>,
    /// Experimental features that can be enabled in the host
    pub experimental_features: Features,
    /// HTTP administration endpoint address
    pub http_admin: Option<SocketAddr>,
    /// Whether component auctions are enabled
    pub enable_component_auction: bool,
    /// Whether capability provider auctions are enabled
    pub enable_provider_auction: bool,
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
            rpc_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse RPC NATS URL"),
            rpc_timeout: Duration::from_millis(2000),
            rpc_jwt: None,
            rpc_key: None,
            rpc_tls: false,
            lattice: "default".into(),
            js_domain: None,
            labels: HashMap::default(),
            host_key: Arc::new(KeyPair::new_server()),
            provider_shutdown_delay: None,
            oci_opts: OciConfig::default(),
            allow_file_load: false,
            enable_structured_logging: false,
            log_level: LogLevel::Info,
            config_service_enabled: false,
            otel_config: OtelConfig::default(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            max_execution_time: Duration::from_millis(10 * 60 * 1000),
            // 10 MB
            max_linear_memory: MAX_LINEAR_MEMORY,
            // 50 MB
            max_component_size: MAX_COMPONENT_SIZE,
            max_core_instances_per_component: DEFAULT_MAX_CORE_INSTANCES_PER_COMPONENT,
            max_components: MAX_COMPONENTS,
            heartbeat_interval: None,
            experimental_features: Features::default(),
            http_admin: None,
            enable_component_auction: true,
            enable_provider_auction: true,
        }
    }
}
