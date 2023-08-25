#![warn(clippy::pedantic)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::{self, Context};
use clap::Parser;
use nkeys::KeyPair;
use tokio::{select, signal};
use tracing::Level as TracingLogLevel;
use wasmcloud_core::logging::Level as WasmcloudLogLevel;
use wasmcloud_core::OtelConfig;
use wasmcloud_host::oci::Config as OciConfig;
use wasmcloud_host::url::Url;
use wasmcloud_host::WasmbusHostConfig;
use wasmcloud_tracing::configure_tracing;

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(version, about, long_about = None)]
struct Args {
    /// Controls the verbosity of logs from the wasmCloud host
    #[clap(long = "log-level", alias = "structured-log-level", default_value_t = TracingLogLevel::INFO, env = "WASMCLOUD_LOG_LEVEL")]
    pub log_level: TracingLogLevel,
    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = "127.0.0.1", env = "NATS_HOST")]
    nats_host: String,
    /// NATS server port to connect to
    #[clap(long = "nats-port", default_value_t = 4222, env = "NATS_PORT")]
    nats_port: u16,
    /// A user JWT to use to authenticate to NATS
    #[clap(long = "nats-jwt", env = "NATS_JWT", requires = "nats_seed")]
    nats_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS
    #[clap(long = "nats-seed", env = "NATS_SEED", requires = "nats_jwt")]
    nats_seed: Option<String>,

    /// The lattice the host belongs to
    #[clap(
        short = 'x',
        long = "lattice-prefix",
        default_value = "default",
        env = "WASMCLOUD_LATTICE_PREFIX"
    )]
    lattice_prefix: String,
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

    /// An IP address or DNS name to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "prov-rpc-host", env = "WASMCLOUD_PROV_RPC_HOST", hide = true)]
    prov_rpc_host: Option<String>,
    /// A port to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "prov-rpc-port", env = "WASMCLOUD_PROV_RPC_PORT", hide = true)]
    prov_rpc_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for Provider RPC messages, defaults to the value supplied to --nats-jwt if not supplied
    #[clap(
        long = "prov-rpc-jwt",
        env = "WASMCLOUD_PROV_RPC_JWT",
        requires = "prov_rpc_seed",
        hide = true
    )]
    prov_rpc_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for Provider RPC messages, defaults to the value supplied to --nats-seed if not supplied
    #[clap(
        long = "prov-rpc-seed",
        env = "WASMCLOUD_PROV_RPC_SEED",
        requires = "prov_rpc_jwt",
        hide = true
    )]
    prov_rpc_seed: Option<String>,
    /// Optional flag to require host communication over TLS with a NATS server for Provider RPC messages
    #[clap(long = "prov-rpc-tls", env = "WASMCLOUD_PROV_RPC_TLS", hide = true)]
    prov_rpc_tls: bool,

    // TODO: use and implement policy
    #[clap(long = "policy-topic", env = "WASMCLOUD_POLICY_TOPIC", hide = true)]
    policy_topic: Option<String>,
    #[clap(
        long = "policy-changes-topic",
        env = "WASMCLOUD_POLICY_CHANGES_TOPIC",
        hide = true
    )]
    policy_changes_topic: Option<String>,
    #[clap(long = "policy-timeout-ms", env = "WASMCLOUD_POLICY_TIMEOUT", value_parser = parse_duration, hide = true)]
    policy_timeout_ms: Option<Duration>,

    /// Used in tandem with `oci_user` and `oci_password` to override credentials for a specific OCI registry.
    #[clap(
        long = "oci-registry",
        env = "OCI_REGISTRY",
        requires = "oci_user",
        requires = "oci_password"
    )]
    oci_registry: Option<String>,
    /// Username for the OCI registry specified by `oci_registry`.
    #[clap(
        long = "oci-user",
        env = "OCI_REGISTRY_USER",
        requires = "oci_registry",
        requires = "oci_password"
    )]
    oci_user: Option<String>,
    /// Password for the OCI registry specified by `oci_registry`.
    #[clap(
        long = "oci-password",
        env = "OCI_REGISTRY_PASSWORD",
        requires = "oci_registry",
        requires = "oci_user"
    )]
    oci_password: Option<String>,

    /// Specifies which exporter to use for traces. Only "otlp" is supported at this time
    #[clap(
        long = "otel-traces-exporter",
        env = "OTEL_TRACES_EXPORTER",
        default_value = "otlp"
    )]
    otel_traces_exporter: Option<String>,

    /// Specifies the endpoint to use for the OTLP exporter
    #[clap(
        long = "otel-exporter-otlp-endpoint",
        env = "OTEL_EXPORTER_OTLP_ENDPOINT"
    )]
    otel_exporter_otlp_endpoint: Option<String>,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    let args: Args = Args::parse();

    let otel_config = OtelConfig {
        traces_exporter: args.otel_traces_exporter,
        exporter_otlp_endpoint: args.otel_exporter_otlp_endpoint,
    };
    let log_level = WasmcloudLogLevel::from(args.log_level);
    if let Err(e) = configure_tracing(
        "wasmCloud Host".to_string(),
        &otel_config,
        args.enable_structured_logging,
        Some(&log_level),
    ) {
        eprintln!("Failed to configure tracing: {e}");
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
    let prov_rpc_nats_url = Url::parse(&format!(
        "nats://{}:{}",
        args.prov_rpc_host.unwrap_or(args.nats_host),
        args.prov_rpc_port.unwrap_or(args.nats_port)
    ))
    .context(
        "failed to construct a valid `prov_rpc_nats_url` using `prov-rpc-host` and `prov-rpc-port`",
    )?;

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
    let prov_rpc_key = args
        .prov_rpc_seed
        .as_deref()
        .map(KeyPair::from_seed)
        .transpose()
        .context("failed to construct provider RPC key pair from seed")?
        .map(Arc::new);
    let oci_opts = OciConfig {
        allow_latest: args.allow_latest,
        allowed_insecure: args.allowed_insecure,
        oci_registry: args.oci_registry,
        oci_user: args.oci_user,
        oci_password: args.oci_password,
    };
    let (host, shutdown) = Box::pin(wasmcloud_host::wasmbus::Host::new(WasmbusHostConfig {
        ctl_nats_url,
        lattice_prefix: args.lattice_prefix,
        host_key,
        cluster_key,
        cluster_issuers: args.cluster_issuers,
        config_service_enabled: args.config_service_enabled,
        js_domain: args.js_domain,
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
        prov_rpc_nats_url,
        prov_rpc_jwt: args.prov_rpc_jwt.or_else(|| args.nats_jwt.clone()),
        prov_rpc_key: prov_rpc_key.or_else(|| nats_key.clone()),
        prov_rpc_tls: args.prov_rpc_tls,
        allow_file_load: args.allow_file_load,
        log_level,
        enable_structured_logging: args.enable_structured_logging,
        otel_config,
    }))
    .await
    .context("failed to initialize host")?;
    select! {
        sig = signal::ctrl_c() => sig.context("failed to wait for Ctrl-C")?,
        _ = host.stopped() => {},
    };
    shutdown.await.context("failed to shutdown host")?;
    Ok(())
}

fn parse_duration(arg: &str) -> anyhow::Result<Duration> {
    arg.parse()
        .map(Duration::from_millis)
        .map_err(|e| anyhow::anyhow!(e))
}
