use core::net::SocketAddr;

use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{bail, Context};
use clap::{ArgAction, Parser};
use nkeys::KeyPair;
use regex::Regex;
use tokio::time::{timeout, timeout_at};
use tokio::{select, signal};
use tracing::{warn, Level as TracingLogLevel};
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;
use wasmcloud_core::logging::Level as WasmcloudLogLevel;
use wasmcloud_core::{OtelConfig, OtelProtocol};
use wasmcloud_host::nats::builder::NatsHostBuilder;
use wasmcloud_host::oci::Config as OciConfig;
use wasmcloud_host::workload_identity::WorkloadIdentityConfig;
use wasmcloud_host::WasmbusHostConfig;
use wasmcloud_host::{nats::connect_nats, wasmbus::Features};
use wasmcloud_tracing::configure_observability;

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[clap(name = "wasmcloud")]
#[command(version, about, long_about = None)]
struct Args {
    /// Controls the verbosity of traces emitted from the wasmCloud host
    #[clap(long = "trace-level", default_value_t = TracingLogLevel::INFO, env = "WASMCLOUD_TRACE_LEVEL")]
    pub trace_level: TracingLogLevel,
    /// Controls the verbosity of logs from the wasmCloud host
    #[clap(long = "log-level", alias = "structured-log-level", default_value_t = TracingLogLevel::INFO, env = "WASMCLOUD_LOG_LEVEL")]
    pub log_level: TracingLogLevel,
    /// NATS server host to connect to
    #[clap(
        long = "nats-host",
        default_value = "127.0.0.1",
        env = "WASMCLOUD_NATS_HOST"
    )]
    nats_host: String,
    /// NATS server port to connect to
    #[clap(
        long = "nats-port",
        default_value_t = 4222,
        env = "WASMCLOUD_NATS_PORT"
    )]
    nats_port: u16,
    /// A user JWT to use to authenticate to NATS
    #[clap(
        long = "nats-jwt",
        env = "WASMCLOUD_NATS_JWT",
        requires = "nats_seed",
        conflicts_with = "nats_creds"
    )]
    nats_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS
    #[clap(
        long = "nats-seed",
        env = "WASMCLOUD_NATS_SEED",
        requires = "nats_jwt",
        conflicts_with = "nats_creds"
    )]
    nats_seed: Option<String>,
    /// A NATS credentials file that contains the JWT and seed for authenticating to NATS
    #[clap(long = "nats-creds", env = "WASMCLOUD_NATS_CREDS", conflicts_with_all = ["nats_jwt", "nats_seed"])]
    nats_creds: Option<PathBuf>,
    /// The lattice the host belongs to
    #[clap(
        short = 'x',
        long = "lattice",
        default_value = "default",
        env = "WASMCLOUD_LATTICE"
    )]
    lattice: String,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate its public key
    #[clap(long = "host-seed", env = "WASMCLOUD_HOST_SEED")]
    host_seed: Option<String>,
    /// Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process
    #[clap(long = "provider-shutdown-delay-ms", alias = "provider-shutdown-delay", default_value = "300", env = "WASMCLOUD_PROV_SHUTDOWN_DELAY_MS", value_parser = parse_duration_millis)]
    provider_shutdown_delay: Duration,
    /// Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
    #[clap(long = "allow-latest", env = "WASMCLOUD_OCI_ALLOW_LATEST")]
    allow_latest: bool,
    /// A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
    #[clap(
        long = "allowed-insecure",
        env = "WASMCLOUD_OCI_ALLOWED_INSECURE",
        value_delimiter = ','
    )]
    allowed_insecure: Vec<String>,
    /// NATS Jetstream domain name
    #[clap(
        long = "js-domain",
        alias = "wasmcloud-js-domain",
        env = "WASMCLOUD_JS_DOMAIN"
    )]
    js_domain: Option<String>,
    /// Denotes if a wasmCloud host should issue requests to a config service on startup
    #[clap(long = "config-service-enabled", env = "WASMCLOUD_CONFIG_SERVICE")]
    config_service_enabled: bool,
    /// Denotes if a wasmCloud host should allow starting components from the file system
    #[clap(
        long = "allow-file-load",
        default_value_t = false,
        env = "WASMCLOUD_ALLOW_FILE_LOAD"
    )]
    allow_file_load: bool,
    /// Enable JSON structured logging from the wasmCloud host
    #[clap(
        long = "enable-structured-logging",
        env = "WASMCLOUD_STRUCTURED_LOGGING_ENABLED"
    )]
    enable_structured_logging: bool,
    /// Start the host with a set of labels, can be specified multiple times. This can alternatively be specified via environment variables prefixed with `WASMCLOUD_LABEL_`, e.g. `WASMCLOUD_LABEL_foo=bar`
    #[clap(short = 'l', long = "label")]
    label: Option<Vec<String>>,

    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = "WASMCLOUD_CTL_HOST", hide = true)]
    ctl_host: Option<String>,
    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = "WASMCLOUD_CTL_PORT", hide = true)]
    ctl_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for CTL messages, defaults to the value supplied to --nats-jwt if not supplied
    #[clap(
        long = "ctl-jwt",
        env = "WASMCLOUD_CTL_JWT",
        requires = "ctl_seed",
        hide = true,
        conflicts_with = "ctl_creds"
    )]
    ctl_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for CTL messages, defaults to the value supplied to --nats-seed if not supplied
    #[clap(
        long = "ctl-seed",
        env = "WASMCLOUD_CTL_SEED",
        requires = "ctl_jwt",
        hide = true,
        conflicts_with = "ctl_creds"
    )]
    ctl_seed: Option<String>,
    /// A NATS credentials file to use to authenticate to NATS for CTL messages, defaults to the value supplied to --nats-creds or --nats-jwt and --nats-seed
    #[clap(long = "ctl-creds", env = "WASMCLOUD_CTL_CREDS", hide = true, conflicts_with_all = ["ctl_jwt", "ctl_seed"])]
    ctl_creds: Option<PathBuf>,
    /// Optional flag to require host communication over TLS with a NATS server for CTL messages
    #[clap(long = "ctl-tls", env = "WASMCLOUD_CTL_TLS", hide = true)]
    ctl_tls: bool,
    /// Advanced: A prefix to use for all CTL topics
    #[clap(
        long = "ctl-topic-prefix",
        env = "WASMCLOUD_CTL_TOPIC_PREFIX",
        default_value = "wasmbus.ctl",
        hide = true
    )]
    ctl_topic_prefix: String,

    /// An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "rpc-host", env = "WASMCLOUD_RPC_HOST", hide = true)]
    rpc_host: Option<String>,
    /// A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "rpc-port", env = "WASMCLOUD_RPC_PORT", hide = true)]
    rpc_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for RPC messages, defaults to the value supplied to --nats-jwt if not supplied
    #[clap(
        long = "rpc-jwt",
        env = "WASMCLOUD_RPC_JWT",
        requires = "rpc_seed",
        hide = true,
        conflicts_with = "rpc_creds"
    )]
    rpc_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for RPC messages, defaults to the value supplied to --nats-seed if not supplied
    #[clap(
        long = "rpc-seed",
        env = "WASMCLOUD_RPC_SEED",
        requires = "rpc_jwt",
        hide = true,
        conflicts_with = "rpc_creds"
    )]
    rpc_seed: Option<String>,
    /// A NATS credentials file to use to authenticate to NATS for RPC messages, defaults to the value supplied to --nats-creds or --nats-jwt and --nats-seed
    #[clap(long = "rpc-creds", env = "WASMCLOUD_RPC_CREDS", hide = true, conflicts_with_all = ["rpc_jwt", "rpc_seed"])]
    rpc_creds: Option<PathBuf>,
    /// Timeout in milliseconds for all RPC calls
    #[clap(long = "rpc-timeout-ms", default_value = "2000", env = "WASMCLOUD_RPC_TIMEOUT_MS", value_parser = parse_duration_millis, hide = true)]
    rpc_timeout_ms: Duration,
    /// Optional flag to require host communication over TLS with a NATS server for RPC messages
    #[clap(long = "rpc-tls", env = "WASMCLOUD_RPC_TLS", hide = true)]
    rpc_tls: bool,

    /// If provided, enables policy checks on start actions and component invocations
    #[clap(long = "policy-topic", env = "WASMCLOUD_POLICY_TOPIC")]
    policy_topic: Option<String>,
    /// If provided, allows the host to subscribe to updates on past policy decisions. Requires `policy_topic` to be set.
    #[clap(
        long = "policy-changes-topic",
        env = "WASMCLOUD_POLICY_CHANGES_TOPIC",
        requires = "policy_topic"
    )]
    policy_changes_topic: Option<String>,
    /// If provided, allows to set a custom Max Execution time for the Host in ms.
    #[clap(long = "max-execution-time-ms", default_value = "600000", env = "WASMCLOUD_MAX_EXECUTION_TIME_MS", value_parser = parse_duration_millis)]
    max_execution_time: Duration,
    /// The maximum amount of memory bytes that a component can allocate (default 256 MiB)
    #[clap(long = "max-linear-memory-bytes", default_value_t = 256 * 1024 * 1024, env = "WASMCLOUD_MAX_LINEAR_MEMORY")]
    max_linear_memory: u64,
    /// The maximum byte size of a component binary that can be loaded (default 50 MiB)
    #[clap(long = "max-component-size-bytes", default_value_t = 50 * 1024 * 1024, env = "WASMCLOUD_MAX_COMPONENT_SIZE")]
    max_component_size: u64,
    /// The maximum number of components that can be run simultaneously
    #[clap(
        long = "max-components",
        default_value_t = 10_000,
        env = "WASMCLOUD_MAX_COMPONENTS"
    )]
    max_components: u32,

    /// The maximum number of core instances per component
    #[clap(
        long = "max-core-instances-per-component",
        default_value_t = 30,
        env = "WASMCLOUD_MAX_CORE_INSTANCES_PER_COMPONENT"
    )]
    max_core_instances_per_component: u32,

    /// If provided, allows setting a custom timeout for requesting policy decisions. Defaults to one second. Requires `policy_topic` to be set.
    #[clap(
        long = "policy-timeout-ms",
        env = "WASMCLOUD_POLICY_TIMEOUT",
        requires = "policy_topic",
        value_parser = parse_duration_millis,
    )]
    policy_timeout_ms: Option<Duration>,

    /// If provided, enables interfacing with a secrets backend for secret retrieval over the given topic prefix. Must not be empty.
    #[clap(long = "secrets-topic", env = "WASMCLOUD_SECRETS_TOPIC")]
    secrets_topic_prefix: Option<String>,

    /// Used in tandem with `oci_user` and `oci_password` to override credentials for a specific OCI registry.
    #[clap(
        long = "oci-registry",
        env = "WASMCLOUD_OCI_REGISTRY",
        requires = "oci_user",
        requires = "oci_password"
    )]
    oci_registry: Option<String>,
    /// Username for the OCI registry specified by `oci_registry`.
    #[clap(
        long = "oci-user",
        env = "WASMCLOUD_OCI_REGISTRY_USER",
        requires = "oci_registry",
        requires = "oci_password"
    )]
    oci_user: Option<String>,
    /// Password for the OCI registry specified by `oci_registry`.
    #[clap(
        long = "oci-password",
        env = "WASMCLOUD_OCI_REGISTRY_PASSWORD",
        requires = "oci_registry",
        requires = "oci_user"
    )]
    oci_password: Option<String>,

    /// Determines whether observability should be enabled.
    #[clap(
        long = "enable-observability",
        env = "WASMCLOUD_OBSERVABILITY_ENABLED",
        conflicts_with_all = ["enable_traces", "enable_metrics", "enable_logs"]
    )]
    enable_observability: bool,

    /// Determines whether traces should be enabled.
    #[clap(long = "enable-traces", env = "WASMCLOUD_TRACES_ENABLED", hide = true)]
    enable_traces: Option<bool>,

    /// Determines whether metrics should be enabled.
    #[clap(
        long = "enable-metrics",
        env = "WASMCLOUD_METRICS_ENABLED",
        hide = true
    )]
    enable_metrics: Option<bool>,

    /// Determines whether logs should be enabled.
    #[clap(long = "enable-logs", env = "WASMCLOUD_LOGS_ENABLED", hide = true)]
    enable_logs: Option<bool>,

    /// Overrides the OpenTelemetry endpoint used for emitting traces, metrics and logs.
    #[clap(
        long = "override-observability-endpoint",
        env = "OTEL_EXPORTER_OTLP_ENDPOINT"
    )]
    observability_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting traces.
    #[clap(
        long = "override-traces-endpoint",
        env = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        hide = true
    )]
    traces_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting metrics.
    #[clap(
        long = "override-metrics-endpoint",
        env = "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        hide = true
    )]
    metrics_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting logs.
    #[clap(
        long = "override-logs-endpoint",
        env = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        hide = true
    )]
    logs_endpoint: Option<String>,

    /// Configures whether grpc or http will be used for exporting the enabled telemetry. This defaults to 'http'.
    #[clap(
        long = "observability-protocol",
        env = "WASMCLOUD_OBSERVABILITY_PROTOCOL",
        hide = true
    )]
    observability_protocol: Option<OtelProtocol>,

    /// Path to generate flame graph at
    #[clap(long = "flame-graph", env = "WASMCLOUD_FLAME_GRAPH")]
    flame_graph: Option<String>,

    /// Configures the set of certificate authorities as repeatable set of file paths to load into the OCI and OpenTelemetry clients
    #[arg(
        long = "tls-ca-path",
        env = "WASMCLOUD_TLS_CA_PATH",
        value_delimiter = ','
    )]
    pub tls_ca_paths: Option<Vec<PathBuf>>,

    /// If provided, overrides the default heartbeat interval of every 30 seconds. Provided value is interpreted as seconds.
    #[arg(long = "heartbeat-interval-seconds", env = "WASMCLOUD_HEARTBEAT_INTERVAL", value_parser = parse_duration_secs, hide = true)]
    heartbeat_interval: Option<Duration>,

    /// Experimental features to enable in the host. This is a repeatable option.
    #[arg(
        long = "feature",
        env = "WASMCLOUD_EXPERIMENTAL_FEATURES",
        value_delimiter = ',',
        hide = true
    )]
    experimental_features: Vec<Features>,

    #[clap(
        long = "help-markdown",
        action=ArgAction::SetTrue,
        conflicts_with = "help",
        hide = true
    )]
    help_markdown: bool,

    #[clap(long = "http-admin", env = "WASMCLOUD_HTTP_ADMIN")]
    /// HTTP administration endpoint address
    http_admin: Option<SocketAddr>,

    #[clap(
        long = "enable-component-auction",
        env = "WASMCLOUD_COMPONENT_AUCTION_ENABLED"
    )]
    /// Determines whether component auctions should be enabled (defaults to true)
    enable_component_auction: Option<bool>,

    #[clap(
        long = "enable-provider-auction",
        env = "WASMCLOUD_PROVIDER_AUCTION_ENABLED"
    )]
    /// Determines whether capability provider auctions should be enabled (defaults to true)
    enable_provider_auction: Option<bool>,
}

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let args: Args = Args::parse();

    // Implements clap_markdown for markdown generation of command line documentation.`
    if args.help_markdown {
        clap_markdown::print_help_markdown::<Args>();
        std::process::exit(0);
    }

    if let Some(tls_ca_paths) = args.tls_ca_paths.clone() {
        ensure_certs_for_paths(tls_ca_paths)?;
    }

    let trace_level = WasmcloudLogLevel::from(args.trace_level);
    let otel_config = OtelConfig {
        enable_observability: args.enable_observability,
        enable_traces: args.enable_traces,
        enable_metrics: args.enable_metrics,
        enable_logs: args.enable_logs,
        observability_endpoint: args.observability_endpoint,
        traces_endpoint: args.traces_endpoint,
        metrics_endpoint: args.metrics_endpoint,
        logs_endpoint: args.logs_endpoint,
        protocol: args.observability_protocol.unwrap_or_default(),
        additional_ca_paths: args.tls_ca_paths.clone().unwrap_or_default(),
        trace_level,
        ..Default::default()
    };
    let log_level = WasmcloudLogLevel::from(args.log_level);

    let _guard = match configure_observability(
        "wasmcloud-host",
        &otel_config,
        args.enable_structured_logging,
        args.flame_graph,
        Some(&log_level),
        Some(&otel_config.trace_level),
    ) {
        Ok((dispatch, guard)) => {
            dispatch
                .try_init()
                .context("failed to init observability for host")?;
            Some(guard)
        }
        Err(e) => {
            eprintln!("Failed to configure observability: {e:?}");
            None
        }
    };

    let ctl_nats_url = Url::parse(&format!(
        "nats://{}:{}",
        args.ctl_host.unwrap_or_else(|| args.nats_host.clone()),
        args.ctl_port.unwrap_or(args.nats_port)
    ))
    .context("failed to construct a valid `ctl_nats_url` using `ctl-host` and `ctl-port`")?;
    let rpc_nats_url = Url::parse(&format!(
        "nats://{}:{}",
        args.rpc_host.unwrap_or_else(|| args.nats_host.clone()),
        args.rpc_port.unwrap_or(args.nats_port)
    ))
    .context("failed to construct a valid `rpc_nats_url` using `rpc-host` and `rpc-port`")?;

    let host_key = args
        .host_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct host key pair from seed")?
        .map(Arc::new)
        .unwrap_or_else(|| Arc::new(KeyPair::new_server()));
    let (nats_jwt, nats_key) =
        parse_nats_credentials(args.nats_creds, args.nats_jwt, args.nats_seed)
            .await
            .context("failed to parse NATS credentials from provided arguments")?;
    let (ctl_jwt, ctl_key) = parse_nats_credentials(args.ctl_creds, args.ctl_jwt, args.ctl_seed)
        .await
        .context("failed to parse control interface credentials from provided arguments")?;
    let (rpc_jwt, rpc_key) = parse_nats_credentials(args.rpc_creds, args.rpc_jwt, args.rpc_seed)
        .await
        .context("failed to parse RPC credentials from provided arguments")?;
    let oci_opts = OciConfig {
        additional_ca_paths: args.tls_ca_paths.unwrap_or_default(),
        allow_latest: args.allow_latest,
        allowed_insecure: args.allowed_insecure,
        oci_registry: args.oci_registry,
        oci_user: args.oci_user,
        oci_password: args.oci_password,
    };

    let mut labels = args
        .label
        .unwrap_or_default()
        .iter()
        .map(|labelpair| parse_label(labelpair))
        .collect::<anyhow::Result<HashMap<String, String>, anyhow::Error>>()
        .context("failed to parse labels")?;
    let labels_from_args: HashSet<String> = labels.keys().cloned().collect();
    labels.extend(env::vars().filter_map(|(key, value)| {
        let key = if key.starts_with("WASMCLOUD_LABEL_") {
            key.strip_prefix("WASMCLOUD_LABEL_")?.to_string()
        } else {
            return None;
        };
        if labels_from_args.contains(&key) {
            warn!(
                ?key,
                "label provided via args will override label set via environment variable"
            );
            return None;
        }
        Some((key, value))
    }));
    if let Some(secrets_topic) = args.secrets_topic_prefix.as_deref() {
        anyhow::ensure!(
            validate_nats_subject(secrets_topic).is_ok(),
            "Invalid secrets topic"
        );
    }

    // NOTE(brooksmtownsend): Summing the feature flags "OR"s the multiple flags together.
    let experimental_features: Features = args.experimental_features.into_iter().sum();
    let workload_identity_config = if experimental_features.workload_identity_auth_enabled() {
        Some(WorkloadIdentityConfig::from_env()?)
    } else {
        None
    };
    let ctl_nats = connect_nats(
        ctl_nats_url.as_str(),
        ctl_jwt.or_else(|| nats_jwt.clone()).as_ref(),
        ctl_key.or_else(|| nats_key.clone()),
        args.ctl_tls,
        None,
        workload_identity_config.clone(),
    )
    .await
    .context("failed to establish NATS control connection")?;

    let builder = NatsHostBuilder::new(
        ctl_nats,
        Some(args.ctl_topic_prefix),
        args.lattice.clone(),
        args.js_domain.clone(),
        Some(oci_opts.clone()),
        labels.clone().into_iter().collect(),
        args.config_service_enabled,
        args.enable_component_auction.unwrap_or(true),
        args.enable_provider_auction.unwrap_or(true),
    )
    .await?
    .with_event_publisher(host_key.public_key());

    let builder = if let Some(policy_topic) = args.policy_topic.as_deref() {
        anyhow::ensure!(
            validate_nats_subject(policy_topic).is_ok(),
            "Invalid policy topic"
        );
        builder
            .with_policy_manager(
                host_key.clone(),
                labels.clone(),
                args.policy_topic.clone(),
                args.policy_timeout_ms,
                args.policy_changes_topic.clone(),
            )
            .await?
    } else {
        builder
    };

    let builder = if let Some(secrets_topic) = args.secrets_topic_prefix {
        anyhow::ensure!(
            validate_nats_subject(&secrets_topic).is_ok(),
            "Invalid secrets topic"
        );
        builder.with_secrets_manager(secrets_topic)?
    } else {
        builder
    };

    let (host_builder, nats_ctl_server) = builder
        .build(WasmbusHostConfig {
            lattice: Arc::from(args.lattice.clone()),
            host_key: host_key.clone(),
            config_service_enabled: args.config_service_enabled,
            js_domain: args.js_domain,
            labels,
            provider_shutdown_delay: Some(args.provider_shutdown_delay),
            oci_opts,
            rpc_nats_url,
            rpc_timeout: args.rpc_timeout_ms,
            rpc_jwt: rpc_jwt.or_else(|| nats_jwt.clone()),
            rpc_key: rpc_key.or_else(|| nats_key.clone()),
            rpc_tls: args.rpc_tls,
            allow_file_load: args.allow_file_load,
            log_level,
            enable_structured_logging: args.enable_structured_logging,
            otel_config,
            version: env!("CARGO_PKG_VERSION").to_string(),
            max_execution_time: args.max_execution_time,
            max_linear_memory: args.max_linear_memory,
            max_component_size: args.max_component_size,
            max_components: args.max_components,
            max_core_instances_per_component: args.max_core_instances_per_component,
            heartbeat_interval: args.heartbeat_interval,
            experimental_features,
            http_admin: args.http_admin,
            enable_component_auction: args.enable_component_auction.unwrap_or(true),
            enable_provider_auction: args.enable_provider_auction.unwrap_or(true),
        })
        .await?;
    let (host, shutdown) = host_builder
        .build()
        .await
        .context("failed to initialize host")?;

    // Start the control interface server
    let mut ctl = nats_ctl_server.start(host.clone()).await?;

    #[cfg(unix)]
    let deadline = {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
        select! {
            sig = signal::ctrl_c() => {
                sig.context("failed to wait for Ctrl-C")?;
                None
            },
            _ = terminate.recv() => None,
            deadline = host.stopped() => deadline?,
        }
    };
    #[cfg(not(unix))]
    let deadline = select! {
        sig = signal::ctrl_c() => {
            sig.context("failed to wait for Ctrl-C")?;
            None
        },
        deadline = host.stopped() => deadline?,
    };
    // TODO(brooksmtownsend): Consider a drain of sorts that can wrap up pending persistent work
    ctl.abort_all();
    drop(host);
    if let Some(deadline) = deadline {
        timeout_at(deadline, shutdown)
    } else {
        timeout(DEFAULT_SHUTDOWN_TIMEOUT, shutdown)
    }
    .await
    .context("host shutdown timed out")?
    .context("failed to shutdown host")?;
    Ok(())
}

fn parse_duration_millis(arg: &str) -> anyhow::Result<Duration> {
    arg.parse()
        .map(Duration::from_millis)
        .map_err(|e| anyhow::anyhow!(e))
}

/// Validates that a subject string (e.g. secrets-topic and policy-topic) adheres to the rules and conventions
/// of being a valid NATS subject.
/// This function is specifically for validating subjects to publish to and not intended to be used for
/// validating subjects to subscribe to, as those may include wildcard characters.
fn validate_nats_subject(subject: &str) -> anyhow::Result<()> {
    let re = Regex::new(r"^(?:[A-Za-z0-9_-]+\.)*[A-Za-z0-9_-]+$")
        .context("Failed to compile NATS subject regex")?;
    if re.is_match(subject) && !subject.contains('*') && !subject.contains('>') {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Invalid NATS subject: {}", subject))
    }
}

fn parse_duration_secs(arg: &str) -> anyhow::Result<Duration> {
    arg.parse()
        .map(Duration::from_secs)
        .map_err(|e| anyhow::anyhow!(e))
}

fn parse_label(labelpair: &str) -> anyhow::Result<(String, String)> {
    match labelpair.split('=').collect::<Vec<&str>>()[..] {
        [k, v] => Ok((k.to_string(), v.to_string())),
        _ => bail!("invalid label format `{labelpair}`. Expected `key=value`"),
    }
}

static JWT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN NATS USER JWT-----\n(?<jwt>.*)\n------END NATS USER JWT------").unwrap()
});

static SEED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN USER NKEY SEED-----\n(?<seed>.*)\n------END USER NKEY SEED------")
        .unwrap()
});

async fn parse_nats_credentials(
    nats_creds: Option<PathBuf>,
    nats_jwt: Option<String>,
    nats_seed: Option<String>,
) -> anyhow::Result<(Option<String>, Option<Arc<KeyPair>>)> {
    match (nats_creds, nats_jwt, nats_seed) {
        (Some(creds), None, None) => {
            let contents = tokio::fs::read_to_string(creds).await?;
            Ok(parse_jwt_and_key_from_creds(&contents)?)
        }
        (None, Some(jwt), Some(seed)) => {
            let kp =
                KeyPair::from_seed(&seed).context("failed to construct NATS key pair from seed")?;
            Ok((Some(jwt), Some(Arc::new(kp))))
        }
        _ => Ok((None, None)),
    }
}

fn parse_jwt_and_key_from_creds(
    contents: &str,
) -> anyhow::Result<(Option<String>, Option<Arc<KeyPair>>)> {
    let jwt = JWT_RE
        .captures(contents)
        .map(|capture| capture["jwt"].to_owned())
        .context("failed to parse JWT from NATS credentials")?;
    let kp = SEED_RE
        .captures(contents)
        .and_then(|capture| KeyPair::from_seed(&capture["seed"]).ok())
        .map(Arc::new)
        .context("failed to construct key pair from NATS credentials")?;
    Ok((Some(jwt), Some(kp)))
}

fn ensure_certs_for_paths(paths: Vec<PathBuf>) -> anyhow::Result<()> {
    if wasmcloud_core::tls::load_certs_from_paths(&paths)
        .context("failed to load certificates from the provided path")?
        .is_empty()
    {
        bail!("failed to parse certificates from the provided path");
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_nats_subject_validation() {
        // Valid subjects
        assert!(validate_nats_subject("wasmcloud.secrets").is_ok());
        assert!(validate_nats_subject("simple").is_ok());
        assert!(validate_nats_subject("with_underscore").is_ok());
        assert!(validate_nats_subject("with-hyphen").is_ok());
        assert!(validate_nats_subject("multiple.topic.levels").is_ok());
        assert!(validate_nats_subject("123.456").is_ok());
        assert!(validate_nats_subject("subject.123").is_ok());
        // Invalid subjects
        assert!(validate_nats_subject("").is_err()); // Empty topic
        assert!(validate_nats_subject(".").is_err()); // Just a dot
        assert!(validate_nats_subject(".starts.with.dot").is_err()); // Starts with a dot
        assert!(validate_nats_subject("ends.with.dot.").is_err()); // Ends with a dot
        assert!(validate_nats_subject("double..dot").is_err()); // Double dot
        assert!(validate_nats_subject("contains.*.wildcard").is_err()); // Contains *
        assert!(validate_nats_subject("contains.>.wildcard").is_err()); // Contains >
        assert!(validate_nats_subject("spaced words").is_err()); // Contains space
        assert!(validate_nats_subject("invalid!chars").is_err()); // Contains !
        assert!(validate_nats_subject("invalid@chars").is_err()); // Contains @
    }

    #[tokio::test]
    async fn test_parse_nats_credentials() {
        let expected_jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJPSTZNRlRQMlpPTlVaSlhTSjVGQ01CUVFGR0xIRUlZTkpXWVJTR0xRQkRHS1JKTVlDQlpBIiwiaWF0IjoxNzI0NzczMDMzLCJpc3MiOiJBQUo3S0s3TkFQQURLM0dUSVNPQ1BFUVk1UVFRMk1MUFdVWlVTWVVNN0pRQVYyNExYSUZGQkU0WCIsIm5hbWUiOiJqdXN0LWZvci10ZXN0aW5nIiwic3ViIjoiVUI2NzJSWk9VQkxaNFZWTjdNVlpPNktHS1JCTDJFSTVLQldYUkhUVlBKUlA3UDY0WEc2NU5YRDciLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e30sInN1YnMiOi0xLCJkYXRhIjotMSwicGF5bG9hZCI6LTEsInR5cGUiOiJ1c2VyIiwidmVyc2lvbiI6Mn19.YgaVafvKp_VLmlQsN26zrhtX8yHMpnxjcUtX51ctd8hh_KqqiSdHtHOlFRapHbpHaiFS_kp9e67L0aqdSn87BA";
        let expected_seed = "SUAO2CXJCBHGBKIR5TPLXQH6WV2QEEP3YQLLPNVLYVTNSDCZFJMCBHEIN4";

        // Test that passing in `--nats-creds` a creds file works
        let creds = format!(
            r#"
-----BEGIN NATS USER JWT-----
{expected_jwt}
------END NATS USER JWT------

-----BEGIN USER NKEY SEED-----
{expected_seed}
------END USER NKEY SEED------
"#,
        );
        let tmpdir = tempdir().expect("should have created a temporary directory");
        let nats_creds_path = tmpdir.path().join("nats.creds");
        let mut nats_creds = File::create(nats_creds_path.clone())
            .expect("should have created nats.creds in temporary directory");
        let _ = nats_creds.write_all(creds.as_bytes());
        let _ = nats_creds.flush();

        let (jwt, kp) = parse_nats_credentials(Some(nats_creds_path), None, None)
            .await
            .unwrap();
        assert_eq!(jwt.unwrap(), expected_jwt);
        assert_eq!(kp.unwrap().seed().unwrap(), expected_seed);
        drop(nats_creds);
        tmpdir
            .close()
            .expect("should have closed the temporary directory handle");

        // Test that passing in `--nats-jwt` and `--nats-seed` works
        let (jwt, kp) = parse_nats_credentials(
            None,
            Some(String::from(expected_jwt)),
            Some(String::from(expected_seed)),
        )
        .await
        .unwrap();
        assert_eq!(jwt.unwrap(), expected_jwt);
        assert_eq!(kp.unwrap().seed().unwrap(), expected_seed);

        let expected_jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJPSTZNRlRQMlpPTlVaSlhTSjVGQ01CUVFGR0xIRUlZTkpXWVJTR0xRQkRHS1JKTVlDQlpBIiwiaWF0IjoxNzI0NzczMDMzLCJpc3MiOiJBQUo3S0s3TkFQQURLM0dUSVNPQ1BFUVk1UVFRMk1MUFdVWlVTWVVNN0pRQVYyNExYSUZGQkU0WCIsIm5hbWUiOiJqdXN0LWZvci10ZXN0aW5nIiwic3ViIjoiVUI2NzJSWk9VQkxaNFZWTjdNVlpPNktHS1JCTDJFSTVLQldYUkhUVlBKUlA3UDY0WEc2NU5YRDciLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e30sInN1YnMiOi0xLCJkYXRhIjotMSwicGF5bG9hZCI6LTEsInR5cGUiOiJ1c2VyIiwidmVyc2lvbiI6Mn19.YgaVafvKp_VLmlQsN26zrhtX8yHMpnxjcUtX51ctd8hh_KqqiSdHtHOlFRapHbpHaiFS_kp9e67L0aqdSn87BA";
        let expected_seed = "SUAO2CXJCBHGBKIR5TPLXQH6WV2QEEP3YQLLPNVLYVTNSDCZFJMCBHEIN4";
        let (jwt, kp) = parse_nats_credentials(
            None,
            Some(String::from(expected_jwt)),
            Some(String::from(expected_seed)),
        )
        .await
        .unwrap();
        assert_eq!(jwt.unwrap(), expected_jwt);
        assert_eq!(kp.unwrap().seed().unwrap(), expected_seed);

        // Test that passing in nothing also works
        let (no_nats_jwt, no_nats_key) = parse_nats_credentials(None, None, None).await.unwrap();
        assert!(no_nats_jwt.is_none());
        assert!(no_nats_key.is_none());
    }

    #[test]
    fn test_parse_jwt_and_key_from_creds() {
        let expected_jwt = "eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJPSTZNRlRQMlpPTlVaSlhTSjVGQ01CUVFGR0xIRUlZTkpXWVJTR0xRQkRHS1JKTVlDQlpBIiwiaWF0IjoxNzI0NzczMDMzLCJpc3MiOiJBQUo3S0s3TkFQQURLM0dUSVNPQ1BFUVk1UVFRMk1MUFdVWlVTWVVNN0pRQVYyNExYSUZGQkU0WCIsIm5hbWUiOiJqdXN0LWZvci10ZXN0aW5nIiwic3ViIjoiVUI2NzJSWk9VQkxaNFZWTjdNVlpPNktHS1JCTDJFSTVLQldYUkhUVlBKUlA3UDY0WEc2NU5YRDciLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e30sInN1YnMiOi0xLCJkYXRhIjotMSwicGF5bG9hZCI6LTEsInR5cGUiOiJ1c2VyIiwidmVyc2lvbiI6Mn19.YgaVafvKp_VLmlQsN26zrhtX8yHMpnxjcUtX51ctd8hh_KqqiSdHtHOlFRapHbpHaiFS_kp9e67L0aqdSn87BA";
        let expected_seed = "SUAO2CXJCBHGBKIR5TPLXQH6WV2QEEP3YQLLPNVLYVTNSDCZFJMCBHEIN4";

        let creds = format!(
            r#"
-----BEGIN NATS USER JWT-----
{}
------END NATS USER JWT------

************************* IMPORTANT *************************
NKEY Seed printed below can be used to sign and prove identity.
NKEYs are sensitive and should be treated as secrets.

-----BEGIN USER NKEY SEED-----
{}
------END USER NKEY SEED------

*************************************************************
"#,
            expected_jwt, expected_seed
        );

        let (jwt, kp) = parse_jwt_and_key_from_creds(&creds)
            .expect("should have parsed the creds successfully");

        assert!(jwt.is_some());
        assert!(kp.is_some());
        assert_eq!(jwt.unwrap(), expected_jwt);
        assert_eq!(kp.unwrap().seed().unwrap(), expected_seed);

        // Test error cases
        let creds_missing_jwt = r#"
-----BEGIN NATS USER JWT-----
------END NATS USER JWT------

-----BEGIN USER NKEY SEED-----
SUAO2CXJCBHGBKIR5TPLXQH6WV2QEEP3YQLLPNVLYVTNSDCZFJMCBHEIN4
------END USER NKEY SEED------
"#;
        assert!(parse_jwt_and_key_from_creds(creds_missing_jwt).is_err());

        let creds_missing_seed = r#"
-----BEGIN NATS USER JWT-----
eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJPSTZNRlRQMlpPTlVaSlhTSjVGQ01CUVFGR0xIRUlZTkpXWVJTR0xRQkRHS1JKTVlDQlpBIiwiaWF0IjoxNzI0NzczMDMzLCJpc3MiOiJBQUo3S0s3TkFQQURLM0dUSVNPQ1BFUVk1UVFRMk1MUFdVWlVTWVVNN0pRQVYyNExYSUZGQkU0WCIsIm5hbWUiOiJqdXN0LWZvci10ZXN0aW5nIiwic3ViIjoiVUI2NzJSWk9VQkxaNFZWTjdNVlpPNktHS1JCTDJFSTVLQldYUkhUVlBKUlA3UDY0WEc2NU5YRDciLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e30sInN1YnMiOi0xLCJkYXRhIjotMSwicGF5bG9hZCI6LTEsInR5cGUiOiJ1c2VyIiwidmVyc2lvbiI6Mn19.YgaVafvKp_VLmlQsN26zrhtX8yHMpnxjcUtX51ctd8hh_KqqiSdHtHOlFRapHbpHaiFS_kp9e67L0aqdSn87BA
------END NATS USER JWT------

-----BEGIN USER NKEY SEED-----
------END USER NKEY SEED------
        "#;
        assert!(parse_jwt_and_key_from_creds(creds_missing_seed).is_err());

        let creds_invalid_seed = r#"
-----BEGIN NATS USER JWT-----
eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJPSTZNRlRQMlpPTlVaSlhTSjVGQ01CUVFGR0xIRUlZTkpXWVJTR0xRQkRHS1JKTVlDQlpBIiwiaWF0IjoxNzI0NzczMDMzLCJpc3MiOiJBQUo3S0s3TkFQQURLM0dUSVNPQ1BFUVk1UVFRMk1MUFdVWlVTWVVNN0pRQVYyNExYSUZGQkU0WCIsIm5hbWUiOiJqdXN0LWZvci10ZXN0aW5nIiwic3ViIjoiVUI2NzJSWk9VQkxaNFZWTjdNVlpPNktHS1JCTDJFSTVLQldYUkhUVlBKUlA3UDY0WEc2NU5YRDciLCJuYXRzIjp7InB1YiI6e30sInN1YiI6e30sInN1YnMiOi0xLCJkYXRhIjotMSwicGF5bG9hZCI6LTEsInR5cGUiOiJ1c2VyIiwidmVyc2lvbiI6Mn19.YgaVafvKp_VLmlQsN26zrhtX8yHMpnxjcUtX51ctd8hh_KqqiSdHtHOlFRapHbpHaiFS_kp9e67L0aqdSn87BA
------END NATS USER JWT------

-----BEGIN USER NKEY SEED-----
SUANOPE
------END USER NKEY SEED------
        "#;
        assert!(parse_jwt_and_key_from_creds(creds_invalid_seed).is_err());
    }
}
