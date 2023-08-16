#![warn(clippy::pedantic)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{self, Context};
use clap::Parser;
use tokio::{select, signal};
use tracing::Level as LogLevel;
use tracing_subscriber::prelude::*;
use wasmcloud_host::oci::Config as OciConfig;
use wasmcloud_host::url::Url;
use wasmcloud_host::WasmbusHostConfig;

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(version, about, long_about = None)]
struct Args {
    /// Controls the verbosity of logs from the wasmCloud host
    #[clap(long = "log-level", alias = "structured-log-level", default_value_t = LogLevel::INFO, env = "WASMCLOUD_LOG_LEVEL")]
    pub log_level: LogLevel,
    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = "127.0.0.1", env = "NATS_HOST")]
    nats_host: String,
    /// NATS server port to connect to
    #[clap(long = "nats-port", default_value_t = 4222, env = "NATS_PORT")]
    nats_port: u16,
    // TODO: use and implement NATS credsfile auth
    /// NATS credentials file to use when authenticating
    #[clap(long = "nats-credsfile", env = "NATS_CREDSFILE", hide = true)]
    nats_credsfile: Option<PathBuf>,

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
    #[clap(long = "cluster-issuers", env = "WASMCLOUD_CLUSTER_ISSUERS")]
    cluster_issuers: Option<Vec<String>>,
    /// Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process
    #[clap(long = "provider-shutdown-delay", default_value = "300", env = "WASMCLOUD_PROV_SHUTDOWN_DELAY_MS", value_parser = parse_duration)]
    provider_shutdown_delay: Duration,
    /// Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
    #[clap(long = "allow-latest", env = "WASMCLOUD_OCI_ALLOW_LATEST")]
    allow_latest: bool,
    /// A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
    #[clap(long = "allowed-insecure", env = "WASMCLOUD_OCI_ALLOWED_INSECURE")]
    allowed_insecure: Vec<String>,
    /// NATS Jetstream domain name
    #[clap(
        long = "js-domain",
        alias = "wasmcloud-js-domain",
        env = "WASMCLOUD_JS_DOMAIN"
    )]
    js_domain: Option<String>,
    // TODO: use and implement the below args
    /// Denotes if a wasmCloud host should issue requests to a config service on startup
    #[clap(
        long = "config-service-enabled",
        env = "WASMCLOUD_CONFIG_SERVICE",
        hide = true
    )]
    config_service_enabled: bool,
    /// Denotes if a wasmCloud host should allow starting actors from the file system
    #[clap(
        long = "allow-file-load",
        default_value_t = false,
        env = "WASMCLOUD_ALLOW_FILE_LOAD",
        hide = true
    )]
    allow_file_load: bool,
    /// Enables IPV6 addressing for wasmCloud hosts
    #[clap(long = "enable-ipv6", env = "WASMCLOUD_ENABLE_IPV6", hide = true)]
    enable_ipv6: bool,
    /// Enable JSON structured logging from the wasmCloud host
    #[clap(
        long = "enable-structured-logging",
        env = "WASMCLOUD_STRUCTURED_LOGGING_ENABLED"
    )]
    enable_structured_logging: bool,

    // TODO: use and implement RPC variables
    /// An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "rpc-host", env = "WASMCLOUD_RPC_HOST", hide = true)]
    rpc_host: Option<String>,
    /// A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "rpc-port", env = "WASMCLOUD_RPC_PORT", hide = true)]
    rpc_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for RPC messages
    #[clap(
        long = "rpc-jwt",
        env = "WASMCLOUD_RPC_JWT",
        requires = "rpc_seed",
        hide = true
    )]
    rpc_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for RPC messages
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
    /// Optional flag to enable host communication with a NATS server over TLS for RPC messages
    #[clap(long = "rpc-tls", env = "WASMCLOUD_RPC_TLS", hide = true)]
    rpc_tls: bool,

    // TODO: use and implement PROV RPC variables
    /// An IP address or DNS name to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "prov-rpc-host", env = "WASMCLOUD_PROV_RPC_HOST", hide = true)]
    prov_rpc_host: Option<String>,
    /// A port to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "prov-rpc-port", env = "WASMCLOUD_PROV_RPC_PORT", hide = true)]
    prov_rpc_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for Provider RPC messages
    #[clap(
        long = "prov-rpc-jwt",
        env = "WASMCLOUD_PROV_RPC_JWT",
        requires = "prov_rpc_seed",
        hide = true
    )]
    prov_rpc_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for Provider RPC messages
    #[clap(
        long = "prov-rpc-seed",
        env = "WASMCLOUD_PROV_RPC_SEED",
        requires = "prov_rpc_jwt",
        hide = true
    )]
    prov_rpc_seed: Option<String>,
    /// Optional flag to enable host communication with a NATS server over TLS for Provider RPC messages
    #[clap(long = "prov-rpc-tls", env = "WASMCLOUD_PROV_RPC_TLS", hide = true)]
    prov_rpc_tls: bool,

    // TODO: use and implement CTL variables
    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = "WASMCLOUD_CTL_HOST", hide = true)]
    ctl_host: Option<String>,
    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = "WASMCLOUD_CTL_PORT", hide = true)]
    ctl_port: Option<u16>,
    /// A user JWT to use to authenticate to NATS for CTL messages
    #[clap(
        long = "ctl-jwt",
        env = "WASMCLOUD_CTL_JWT",
        requires = "ctl_seed",
        hide = true
    )]
    ctl_jwt: Option<String>,
    /// A seed nkey to use to authenticate to NATS for CTL messages
    #[clap(
        long = "ctl-seed",
        env = "WASMCLOUD_CTL_SEED",
        requires = "ctl_jwt",
        hide = true
    )]
    ctl_seed: Option<String>,
    /// Optional flag to enable host communication with a NATS server over TLS for CTL messages
    #[clap(long = "ctl-tls", env = "WASMCLOUD_CTL_TLS", hide = true)]
    ctl_tls: bool,
    /// A prefix to use for all CTL topics
    #[clap(
        long = "ctl-topic-prefix",
        env = "WASMCLOUD_CTL_TOPIC_PREFIX",
        default_value = "wasmbus.ctl",
        hide = true
    )]
    ctl_topic_prefix: String,

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Args {
        log_level,
        nats_host,
        nats_port,
        lattice_prefix,
        host_seed,
        cluster_seed,
        cluster_issuers,
        provider_shutdown_delay,
        allow_latest,
        allowed_insecure,
        oci_registry,
        oci_user,
        oci_password,
        js_domain,
        ..
    } = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(format!("{log_level},cranelift_codegen=warn"))
            }),
        )
        .init();

    let ctl_nats_url = Url::parse(&format!("nats://{nats_host}:{nats_port}"))
        .context("failed to construct a valid `ctl_nats_url` using `nats-host` and `nats-port`")?;
    let (host, shutdown) = wasmcloud_host::wasmbus::Host::new(WasmbusHostConfig {
        ctl_nats_url,
        lattice_prefix,
        host_seed,
        cluster_seed,
        cluster_issuers,
        js_domain,
        provider_shutdown_delay: Some(provider_shutdown_delay),
        oci_opts: OciConfig {
            allow_latest,
            allowed_insecure,
            oci_registry,
            oci_user,
            oci_password,
        },
    })
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
