use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{self, bail, Context};
use clap::Parser;
use nkeys::KeyPair;
use tokio::time::{timeout, timeout_at};
use tokio::{select, signal};
use tracing::{warn, Level as TracingLogLevel};
use wasmcloud_core::logging::Level as WasmcloudLogLevel;
use wasmcloud_core::OtelConfig;
use wasmcloud_host::oci::Config as OciConfig;
use wasmcloud_host::url::Url;
use wasmcloud_host::wasmbus::host_config::PolicyService as PolicyServiceConfig;
use wasmcloud_host::WasmbusHostConfig;
use wasmcloud_tracing::configure_observability;

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(version, about, long_about = None)]
struct Args {
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
    #[clap(long = "nats-jwt", env = "WASMCLOUD_NATS_JWT", requires = "nats_seed")]
    nats_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS
    #[clap(long = "nats-seed", env = "WASMCOULD_NATS_SEED", requires = "nats_jwt")]
    nats_seed: Option<String>,
    /// The lattice the host belongs to
    #[clap(
        short = 'x',
        long = "lattice",
        alias = "lattice-prefix", // TODO(pre-1.0): remove me
        default_value = "default",
        env = "WASMCLOUD_LATTICE"
    )]
    lattice: String,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate its public key
    #[clap(long = "host-seed", env = "WASMCLOUD_HOST_SEED")]
    host_seed: Option<String>,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    #[clap(long = "cluster-seed", env = "WASMCLOUD_CLUSTER_SEED")]
    cluster_seed: Option<String>,
    /// A comma-delimited list of public keys that can be used as issuers on signed invocations
    #[clap(
        long = "cluster-issuers",
        env = "WASMCLOUD_CLUSTER_ISSUERS",
        value_delimiter = ','
    )]
    cluster_issuers: Option<Vec<String>>,
    /// Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process
    #[clap(long = "provider-shutdown-delay", default_value = "300", env = "WASMCLOUD_PROV_SHUTDOWN_DELAY_MS", value_parser = parse_duration)]
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
    /// Denotes if a wasmCloud host should allow starting actors from the file system
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
        hide = true
    )]
    ctl_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for CTL messages, defaults to the value supplied to --nats-seed if not supplied
    #[clap(
        long = "ctl-seed",
        env = "WASMCLOUD_CTL_SEED",
        requires = "ctl_jwt",
        hide = true
    )]
    ctl_seed: Option<String>,
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
        hide = true
    )]
    rpc_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for RPC messages, defaults to the value supplied to --nats-seed if not supplied
    #[clap(
        long = "rpc-seed",
        env = "WASMCLOUD_RPC_SEED",
        requires = "rpc_jwt",
        hide = true
    )]
    rpc_seed: Option<String>,
    /// Timeout in milliseconds for all RPC calls
    #[clap(long = "rpc-timeout-ms", default_value = "2000", env = "WASMCLOUD_RPC_TIMEOUT_MS", value_parser = parse_duration, hide = true)]
    rpc_timeout_ms: Duration,
    /// Optional flag to require host communication over TLS with a NATS server for RPC messages
    #[clap(long = "rpc-tls", env = "WASMCLOUD_RPC_TLS", hide = true)]
    rpc_tls: bool,

    /// If provided, enables policy checks on start actions and actor invocations
    #[clap(long = "policy-topic", env = "WASMCLOUD_POLICY_TOPIC")]
    policy_topic: Option<String>,
    /// If provided, allows the host to subscribe to updates on past policy decisions. Requires `policy_topic` to be set.
    #[clap(
        long = "policy-changes-topic",
        env = "WASMCLOUD_POLICY_CHANGES_TOPIC",
        requires = "policy_topic"
    )]
    policy_changes_topic: Option<String>,
    /// If provided, allows setting a custom timeout for requesting policy decisions. Defaults to one second. Requires `policy_topic` to be set.
    #[clap(
        long = "policy-timeout-ms",
        env = "WASMCLOUD_POLICY_TIMEOUT",
        requires = "policy_topic",
        value_parser = parse_duration,
    )]
    policy_timeout_ms: Option<Duration>,

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

    /// Overrides the OpenTelemetry endpoint used for emitting traces, metrics and logs. This can also be set with `OTEL_EXPORTER_OTLP_ENDPOINT`.
    #[clap(long = "override-observability-endpoint")]
    observability_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting traces. This can also be set with `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`.
    #[clap(long = "override-traces-endpoint", hide = true)]
    traces_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting metrics. This can also be set with `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`.
    #[clap(long = "override-metrics-endpoint", hide = true)]
    metrics_endpoint: Option<String>,

    /// Overrides the OpenTelemetry endpoint used for emitting logs. This can also be set with `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`.
    #[clap(long = "override-logs-endpoint", hide = true)]
    logs_endpoint: Option<String>,
}

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    // TODO(pre-1.0): remove me
    if let Ok(lattice) = std::env::var("WASMCLOUD_LATTICE_PREFIX") {
        eprintln!(
            "The `WASMCLOUD_LATTICE_PREFIX` environment variable is deprecated and will be removed. Please use `WASMCLOUD_LATTICE` instead."
        );
        std::env::set_var("WASMCLOUD_LATTICE", lattice);
    }

    let args: Args = Args::parse();

    let otel_config = OtelConfig {
        enable_observability: args.enable_observability,
        enable_traces: args.enable_traces,
        enable_metrics: args.enable_metrics,
        enable_logs: args.enable_logs,
        observability_endpoint: args.observability_endpoint,
        traces_endpoint: args.traces_endpoint,
        metrics_endpoint: args.metrics_endpoint,
        logs_endpoint: args.logs_endpoint,
    };
    let log_level = WasmcloudLogLevel::from(args.log_level);

    if let Err(e) = configure_observability(
        "wasmcloud-host",
        &otel_config,
        args.enable_structured_logging,
        Some(&log_level),
    ) {
        eprintln!("Failed to configure observability: {e}");
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
        .map(Arc::new);
    let cluster_key = args
        .cluster_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct cluster key pair from seed")?
        .map(Arc::new);
    let nats_key = args
        .nats_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct NATS key pair from seed")?
        .map(Arc::new);
    let ctl_key = args
        .ctl_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct control interface key pair from seed")?
        .map(Arc::new);
    let rpc_key = args
        .rpc_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct RPC key pair from seed")?
        .map(Arc::new);
    let oci_opts = OciConfig {
        allow_latest: args.allow_latest,
        allowed_insecure: args.allowed_insecure,
        oci_registry: args.oci_registry,
        oci_user: args.oci_user,
        oci_password: args.oci_password,
    };
    let policy_service_config = PolicyServiceConfig {
        policy_topic: args.policy_topic,
        policy_changes_topic: args.policy_changes_topic,
        policy_timeout_ms: args.policy_timeout_ms,
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
    let (host, shutdown) = Box::pin(wasmcloud_host::wasmbus::Host::new(WasmbusHostConfig {
        ctl_nats_url,
        lattice: args.lattice,
        host_key,
        cluster_key,
        cluster_issuers: args.cluster_issuers,
        config_service_enabled: args.config_service_enabled,
        js_domain: args.js_domain,
        labels,
        provider_shutdown_delay: Some(args.provider_shutdown_delay),
        oci_opts,
        ctl_jwt: args.ctl_jwt.or_else(|| args.nats_jwt.clone()),
        ctl_key: ctl_key.or_else(|| nats_key.clone()),
        ctl_tls: args.ctl_tls,
        ctl_topic_prefix: args.ctl_topic_prefix,
        rpc_nats_url,
        rpc_timeout: args.rpc_timeout_ms,
        rpc_jwt: args.rpc_jwt.or_else(|| args.nats_jwt.clone()),
        rpc_key: rpc_key.or_else(|| nats_key.clone()),
        rpc_tls: args.rpc_tls,
        allow_file_load: args.allow_file_load,
        log_level,
        enable_structured_logging: args.enable_structured_logging,
        otel_config,
        policy_service_config,
    }))
    .await
    .context("failed to initialize host")?;
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

fn parse_duration(arg: &str) -> anyhow::Result<Duration> {
    arg.parse()
        .map(Duration::from_millis)
        .map_err(|e| anyhow::anyhow!(e))
}

fn parse_label(labelpair: &str) -> anyhow::Result<(String, String)> {
    match labelpair.split('=').collect::<Vec<&str>>()[..] {
        [k, v] => Ok((k.to_string(), v.to_string())),
        _ => bail!("invalid label format `{labelpair}`. Expected `key=value`"),
    }
}
