use crate::lib::app::{load_app_manifest, AppManifest, AppManifestSource};
use crate::lib::cli::{CommandOutput, OutputKind};
use crate::lib::common::{CommandGroupUsage, WASMCLOUD_HOST_VERSION_T};
use crate::lib::config::{
    create_nats_client_from_opts, host_pid_file, DEFAULT_NATS_TIMEOUT_MS, WADM_PID_FILE,
    WASH_DIRECTORIES,
};
use crate::lib::context::fs::ContextDir;
use crate::lib::context::ContextManager;
use crate::lib::generate::emoji;
use crate::lib::start::{
    ensure_nats_server, ensure_wadm, ensure_wasmcloud, find_wasmcloud_binary, nats_pid_path,
    new_patch_or_pre_1_0_0_minor_version_after_version_string, parse_version_string,
    start_nats_server, start_wadm, start_wasmcloud_host, NatsConfig, WadmConfig,
    GITHUB_WASMCLOUD_ORG, GITHUB_WASMCLOUD_WADM_REPO, GITHUB_WASMCLOUD_WASMCLOUD_REPO,
    NATS_SERVER_BINARY, NATS_SERVER_CONF,
};
use anyhow::{anyhow, bail, Context, Result};
use async_nats::Client;
use clap::Parser;
use semver::Version;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Write;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sysinfo::System;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
};
use tracing::{debug, warn};
use wasmcloud_control_interface::{Client as CtlClient, ClientBuilder as CtlClientBuilder};

use crate::app::deploy_model_from_manifest;
use crate::appearance::spinner::Spinner;
use crate::config::{
    configure_host_env, DEFAULT_ALLOW_FILE_LOAD, DEFAULT_LATTICE, DEFAULT_MAX_EXECUTION_TIME_MS,
    DEFAULT_NATS_HOST, DEFAULT_NATS_PORT, DEFAULT_NATS_WEBSOCKET_PORT,
    DEFAULT_PROV_SHUTDOWN_DELAY_MS, DEFAULT_RPC_TIMEOUT_MS, DEFAULT_STRUCTURED_LOG_LEVEL,
    NATS_SERVER_VERSION, WADM_VERSION, WASMCLOUD_ALLOW_FILE_LOAD, WASMCLOUD_CLUSTER_ISSUERS,
    WASMCLOUD_CLUSTER_SEED, WASMCLOUD_CONFIG_SERVICE, WASMCLOUD_CTL_CREDSFILE, WASMCLOUD_CTL_HOST,
    WASMCLOUD_CTL_JWT, WASMCLOUD_CTL_PORT, WASMCLOUD_CTL_SEED, WASMCLOUD_CTL_TLS,
    WASMCLOUD_CTL_TLS_CA_FILE, WASMCLOUD_CTL_TLS_FIRST, WASMCLOUD_ENABLE_IPV6,
    WASMCLOUD_HOST_LOG_PATH, WASMCLOUD_HOST_PATH, WASMCLOUD_HOST_SEED, WASMCLOUD_HOST_VERSION,
    WASMCLOUD_JS_DOMAIN, WASMCLOUD_LATTICE, WASMCLOUD_LOG_LEVEL, WASMCLOUD_MAX_EXECUTION_TIME_MS,
    WASMCLOUD_OCI_ALLOWED_INSECURE, WASMCLOUD_OCI_ALLOW_LATEST, WASMCLOUD_POLICY_TOPIC,
    WASMCLOUD_PROV_SHUTDOWN_DELAY_MS, WASMCLOUD_RPC_CREDSFILE, WASMCLOUD_RPC_HOST,
    WASMCLOUD_RPC_JWT, WASMCLOUD_RPC_PORT, WASMCLOUD_RPC_SEED, WASMCLOUD_RPC_TIMEOUT_MS,
    WASMCLOUD_RPC_TLS, WASMCLOUD_RPC_TLS_CA_FILE, WASMCLOUD_RPC_TLS_FIRST, WASMCLOUD_SECRETS_TOPIC,
    WASMCLOUD_STRUCTURED_LOGGING_ENABLED,
};

use crate::down::stop_nats;

#[derive(Parser, Debug, Clone)]
pub struct UpCommand {
    /// Launch NATS and wasmCloud detached from the current terminal as background processes
    #[clap(short = 'd', long = "detached", alias = "detach")]
    pub detached: bool,

    #[clap(flatten)]
    pub nats_opts: NatsOpts,

    #[clap(flatten)]
    pub wasmcloud_opts: WasmcloudOpts,

    #[clap(flatten)]
    pub wadm_opts: WadmOpts,
}

#[derive(Parser, Debug, Clone)]
pub struct NatsOpts {
    /// Optional path to a NATS credentials file to authenticate and extend existing NATS infrastructure.
    #[clap(
        long = "nats-credsfile",
        env = "NATS_CREDSFILE",
        requires = "nats_remote_url"
    )]
    pub nats_credsfile: Option<PathBuf>,
    /// Optional path to a NATS config file
    /// NOTE: If your configuration changes the address or port to listen on from 0.0.0.0:4222, ensure you set --nats-host and --nats-port
    #[clap(
        long = "nats-config-file",
        env = "NATS_CONFIG",
        requires = "nats_host",
        requires = "nats_port"
    )]
    pub nats_configfile: Option<PathBuf>,

    /// Optional remote URL of existing NATS infrastructure to extend.
    #[clap(long = "nats-remote-url", env = "NATS_REMOTE_URL")]
    pub nats_remote_url: Option<String>,

    /// If a connection can't be established, exit and don't start a NATS server. Will be ignored if a `remote_url` and credsfile are specified
    #[clap(
        long = "nats-connect-only",
        env = "NATS_CONNECT_ONLY",
        conflicts_with = "nats_remote_url"
    )]
    pub connect_only: bool,

    /// NATS server version to download, e.g. `v2.11.3`. See <https://github.com/nats-io/nats-server/releases>/ for releases
    #[clap(long = "nats-version", default_value = NATS_SERVER_VERSION, env = "NATS_VERSION")]
    pub nats_version: String,

    /// NATS server host to connect to
    #[clap(long = "nats-host", env = "WASMCLOUD_NATS_HOST")]
    pub nats_host: Option<String>,

    /// NATS server port to connect to. This will be used as the NATS listen port if `--nats-connect-only` isn't set
    #[clap(long = "nats-port", env = "WASMCLOUD_NATS_PORT")]
    pub nats_port: Option<u16>,

    /// NATS websocket port to use. TLS is not supported. This is required for the wash ui to connect from localhost
    #[clap(
        long = "nats-websocket-port",
        env = "NATS_WEBSOCKET_PORT",
        default_value = DEFAULT_NATS_WEBSOCKET_PORT
    )]
    pub nats_websocket_port: u16,

    /// NATS Server Jetstream domain for extending superclusters
    #[clap(long = "nats-js-domain", env = "NATS_JS_DOMAIN")]
    pub nats_js_domain: Option<String>,
}

impl From<NatsOpts> for NatsConfig {
    fn from(other: NatsOpts) -> Self {
        let host = other
            .nats_host
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string());
        let port = other.nats_port.unwrap_or_else(|| {
            DEFAULT_NATS_PORT
                .parse()
                .expect("failed to parse default NATS port")
        });
        Self {
            host,
            port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{port}")),
            js_domain: other.nats_js_domain,
            remote_url: other.nats_remote_url,
            credentials: other.nats_credsfile,
            websocket_port: other.nats_websocket_port,
            config_path: other.nats_configfile,
        }
    }
}

#[derive(Parser, Debug, Clone, Default)]
pub struct WasmcloudOpts {
    /// wasmcloud host version to download, e.g. `v1.4.2`.
    ///
    /// defaults to the [`WASMCLOUD_HOST_VERSION`] if not provided
    /// or the latest patch version after that when `wash up` issued,
    /// see [wasmCloud releases](https://github.com/wasmCloud/wasmCloud/releases).
    #[clap(long = "wasmcloud-version", env = "WASMCLOUD_VERSION")]
    pub wasmcloud_version: Option<String>,

    /// A unique identifier for a lattice, frequently used within NATS topics to isolate messages among different lattices
    #[clap(
        short = 'x',
        long = "lattice",
        env = WASMCLOUD_LATTICE
    )]
    pub lattice: Option<String>,

    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate it's public key
    #[clap(long = "host-seed", env = WASMCLOUD_HOST_SEED)]
    pub host_seed: Option<String>,

    /// An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "rpc-host", env = WASMCLOUD_RPC_HOST)]
    pub rpc_host: Option<String>,

    /// A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "rpc-port", env = WASMCLOUD_RPC_PORT)]
    pub rpc_port: Option<u16>,

    /// A seed nkey to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-seed", env = WASMCLOUD_RPC_SEED, requires = "rpc_jwt")]
    pub rpc_seed: Option<String>,

    /// Timeout in milliseconds for all RPC calls
    #[clap(long = "rpc-timeout-ms", default_value = DEFAULT_RPC_TIMEOUT_MS, env = WASMCLOUD_RPC_TIMEOUT_MS)]
    pub rpc_timeout_ms: Option<u64>,

    /// A user JWT to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-jwt", env = WASMCLOUD_RPC_JWT, requires = "rpc_seed")]
    pub rpc_jwt: Option<String>,

    /// Optional flag to enable host communication with a NATS server over TLS for RPC messages
    #[clap(long = "rpc-tls", env = WASMCLOUD_RPC_TLS)]
    pub rpc_tls: bool,

    /// Optional flag to enable performing TLS handshake before expecting the server greeting for RPC messages
    #[clap(long = "rpc-tls-first", env = WASMCLOUD_RPC_TLS_FIRST, requires = "rpc_tls")]
    pub rpc_tls_first: bool,

    /// A TLS CA file to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-tls-ca-file", env = WASMCLOUD_RPC_TLS_CA_FILE)]
    pub rpc_tls_ca_file: Option<PathBuf>,

    /// Convenience flag for RPC authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "rpc-credsfile", env = WASMCLOUD_RPC_CREDSFILE)]
    pub rpc_credsfile: Option<PathBuf>,

    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = WASMCLOUD_CTL_HOST)]
    pub ctl_host: Option<String>,

    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = WASMCLOUD_CTL_PORT)]
    pub ctl_port: Option<u16>,

    /// A seed nkey to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED, requires = "ctl_jwt")]
    pub ctl_seed: Option<String>,

    /// A user JWT to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT, requires = "ctl_seed")]
    pub ctl_jwt: Option<String>,

    /// Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "ctl-credsfile", env = WASMCLOUD_CTL_CREDSFILE)]
    pub ctl_credsfile: Option<PathBuf>,

    /// Optional flag to enable host communication with a NATS server over TLS for CTL messages
    #[clap(long = "ctl-tls", env = WASMCLOUD_CTL_TLS)]
    pub ctl_tls: bool,

    /// Optional flag to enable performing TLS handshake before expecting the server greeting for CTL messages
    #[clap(long = "ctl-tls-first", env = WASMCLOUD_CTL_TLS_FIRST, requires = "ctl_tls")]
    pub ctl_tls_first: bool,

    /// A TLS CA file to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-tls-ca-file", env = WASMCLOUD_CTL_TLS_CA_FILE)]
    pub ctl_tls_ca_file: Option<PathBuf>,

    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    #[clap(long = "cluster-seed", env = WASMCLOUD_CLUSTER_SEED)]
    pub cluster_seed: Option<String>,

    /// A comma-delimited list of public keys that can be used as issuers on signed invocations
    #[clap(long = "cluster-issuers", env = WASMCLOUD_CLUSTER_ISSUERS)]
    pub cluster_issuers: Option<Vec<String>>,

    /// Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process
    #[clap(long = "provider-delay", default_value = DEFAULT_PROV_SHUTDOWN_DELAY_MS, env = WASMCLOUD_PROV_SHUTDOWN_DELAY_MS)]
    pub provider_delay: u32,

    /// Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
    #[clap(long = "allow-latest", env = WASMCLOUD_OCI_ALLOW_LATEST)]
    pub allow_latest: bool,

    /// A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
    #[clap(long = "allowed-insecure", env = WASMCLOUD_OCI_ALLOWED_INSECURE)]
    pub allowed_insecure: Option<Vec<String>>,

    /// Jetstream domain name, configures a host to properly connect to a NATS supercluster
    #[clap(long = "wasmcloud-js-domain", env = WASMCLOUD_JS_DOMAIN)]
    pub wasmcloud_js_domain: Option<String>,

    /// Denotes if a wasmCloud host should issue requests to a config service on startup
    #[clap(long = "config-service-enabled", env = WASMCLOUD_CONFIG_SERVICE)]
    pub config_service_enabled: bool,

    /// Denotes if a wasmCloud host should allow starting components from the file system
    #[clap(long = "allow-file-load", default_value = DEFAULT_ALLOW_FILE_LOAD, env = WASMCLOUD_ALLOW_FILE_LOAD)]
    pub allow_file_load: Option<bool>,

    /// Enable JSON structured logging from the wasmCloud host
    #[clap(
        long = "enable-structured-logging",
        env = WASMCLOUD_STRUCTURED_LOGGING_ENABLED
    )]
    pub enable_structured_logging: bool,

    /// A label to apply to the host, in the form of `key=value`. This flag can be repeated to supply multiple labels
    #[clap(short = 'l', long = "label", alias = "labels")]
    pub label: Option<Vec<String>>,

    /// Controls the verbosity of JSON structured logs from the wasmCloud host
    #[clap(long = "log-level", alias = "structured-log-level", default_value = DEFAULT_STRUCTURED_LOG_LEVEL, env = WASMCLOUD_LOG_LEVEL)]
    pub structured_log_level: String,

    /// Enables IPV6 addressing for wasmCloud hosts
    #[clap(long = "enable-ipv6", env = WASMCLOUD_ENABLE_IPV6)]
    pub enable_ipv6: bool,

    /// If enabled, wasmCloud will not be downloaded if it's not installed
    #[clap(long = "wasmcloud-start-only")]
    pub start_only: bool,

    /// If enabled, allows starting additional wasmCloud hosts on this machine
    #[clap(long = "multi-local")]
    pub multi_local: bool,

    /// Defines the Max Execution time (in ms) that the host runtime will execute for
    #[clap(long = "max-execution-time-ms", alias = "max-time-ms", env = WASMCLOUD_MAX_EXECUTION_TIME_MS, default_value = DEFAULT_MAX_EXECUTION_TIME_MS)]
    pub max_execution_time: u64,

    /// If provided, enables interfacing with a secrets backend for secret retrieval over the given topic prefix.
    #[clap(long = "secrets-topic", env = WASMCLOUD_SECRETS_TOPIC)]
    pub secrets_topic: Option<String>,

    /// If provided, enables policy checks on start actions and component invocations
    #[clap(long = "policy-topic", env = WASMCLOUD_POLICY_TOPIC)]
    pub policy_topic: Option<String>,

    /// Path to which to log information from the wasmCloud host
    #[clap(long = "host-log-path", env = WASMCLOUD_HOST_LOG_PATH)]
    pub host_log_path: Option<PathBuf>,

    /// Path to a binary that should be used to start the wasmCloud host
    #[clap(long = "host-path", env = WASMCLOUD_HOST_PATH)]
    pub host_path: Option<PathBuf>,
}

impl WasmcloudOpts {
    pub async fn into_ctl_client(self, auction_timeout_ms: Option<u64>) -> Result<CtlClient> {
        let lattice = self.lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string());
        let ctl_host = self
            .ctl_host
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string());
        let ctl_port = self
            .ctl_port
            .map_or_else(|| DEFAULT_NATS_PORT.to_string(), |p| p.to_string())
            .to_string();
        let auction_timeout_ms = auction_timeout_ms.unwrap_or(DEFAULT_NATS_TIMEOUT_MS);

        let nc = create_nats_client_from_opts(
            &ctl_host,
            &ctl_port,
            self.ctl_jwt,
            self.ctl_seed,
            self.ctl_credsfile,
            self.ctl_tls_ca_file,
            self.ctl_tls_first,
        )
        .await
        .context("Failed to create NATS client")?;

        let mut builder = CtlClientBuilder::new(nc)
            .lattice(lattice)
            .auction_timeout(tokio::time::Duration::from_millis(auction_timeout_ms));

        if let Some(rpc_timeout_ms) = self.rpc_timeout_ms {
            builder = builder.timeout(tokio::time::Duration::from_millis(rpc_timeout_ms));
        }

        if let Ok(topic_prefix) = std::env::var("WASMCLOUD_CTL_TOPIC_PREFIX") {
            builder = builder.topic_prefix(topic_prefix);
        }

        let ctl_client = builder.build();

        Ok(ctl_client)
    }
}

#[derive(Parser, Debug, Clone)]
pub struct WadmOpts {
    /// wadm version to download, e.g. `v0.18.0`.
    ///
    /// defaults to the [`WADM_VERSION`] if not provided
    /// or the latest patch version after that when `wash up` issued,
    /// see [wadm releases](https://github.com/wasmCloud/wadm/releases).
    #[clap(long = "wadm-version", env = "WADM_VERSION")]
    pub wadm_version: Option<String>,

    /// If enabled, wadm will not be downloaded or run as a part of the up command
    #[clap(long = "disable-wadm")]
    pub disable_wadm: bool,

    /// The `JetStream` domain to use for wadm
    #[clap(long = "wadm-js-domain", env = "WADM_JS_DOMAIN")]
    pub wadm_js_domain: Option<String>,

    /// The path to a wadm application manifest to run while the host is up
    #[clap(long = "wadm-manifest", env = "WADM_MANIFEST")]
    pub wadm_manifest: Option<PathBuf>,
}

#[derive(Debug, PartialEq, PartialOrd)]
enum WasmCloudHostState {
    NotRunning,
    Starting,
    Running,
    MultipleRunning,
}

pub async fn handle_command(command: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    handle_up(command, output_kind).await
}

pub async fn handle_up(cmd: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = WASH_DIRECTORIES.create_downloads_dir()?;
    let spinner = Spinner::new(&output_kind)?;

    let ctx = ContextDir::new()?
        .load_default_context()
        .context("failed to load context")?;
    // falling back to the context's ctl_ connection won't always be right, but we have to pick one, since the context values are not optional
    let nats_host = cmd.nats_opts.nats_host.clone().unwrap_or(ctx.ctl_host);
    let nats_port = cmd.nats_opts.nats_port.unwrap_or(ctx.ctl_port);
    let wasmcloud_opts = WasmcloudOpts {
        lattice: Some(cmd.wasmcloud_opts.lattice.unwrap_or(ctx.lattice)),
        ctl_host: Some(cmd.wasmcloud_opts.ctl_host.unwrap_or(nats_host.clone())),
        ctl_port: Some(cmd.wasmcloud_opts.ctl_port.unwrap_or(nats_port)),
        ctl_jwt: cmd.wasmcloud_opts.ctl_jwt.or(ctx.ctl_jwt),
        ctl_seed: cmd.wasmcloud_opts.ctl_seed.or(ctx.ctl_seed),
        ctl_credsfile: cmd.wasmcloud_opts.ctl_credsfile.or(ctx.ctl_credsfile),
        rpc_host: Some(cmd.wasmcloud_opts.rpc_host.unwrap_or(nats_host.clone())),
        rpc_port: Some(cmd.wasmcloud_opts.rpc_port.unwrap_or(nats_port)),
        rpc_timeout_ms: Some(cmd.wasmcloud_opts.rpc_timeout_ms.unwrap_or(ctx.rpc_timeout)),
        rpc_jwt: cmd.wasmcloud_opts.rpc_jwt.or(ctx.rpc_jwt),
        rpc_seed: cmd.wasmcloud_opts.rpc_seed.or(ctx.rpc_seed),
        rpc_credsfile: cmd.wasmcloud_opts.rpc_credsfile.or(ctx.rpc_credsfile),
        max_execution_time: cmd.wasmcloud_opts.max_execution_time,
        secrets_topic: cmd.wasmcloud_opts.secrets_topic,
        policy_topic: cmd.wasmcloud_opts.policy_topic,
        cluster_seed: cmd
            .wasmcloud_opts
            .cluster_seed
            .or_else(|| ctx.cluster_seed.map(|seed| seed.to_string())),
        wasmcloud_js_domain: cmd.wasmcloud_opts.wasmcloud_js_domain.or(ctx.js_domain),
        wasmcloud_version: cmd.wasmcloud_opts.wasmcloud_version.clone(),
        ..cmd.wasmcloud_opts
    };
    let host_env = configure_host_env(wasmcloud_opts.clone()).await?;
    let nats_listen_address = format!("{nats_host}:{nats_port}");

    let nats_client = nats_client_from_wasmcloud_opts(&wasmcloud_opts).await;

    // Avoid downloading + starting NATS if the user already runs their own server and we can connect.
    let should_run_nats = !cmd.nats_opts.connect_only && nats_client.is_err();
    // Ignore connect_only if this server has a remote as we have to start a leafnode in that scenario
    let supplied_remote_credentials = cmd.nats_opts.nats_remote_url.is_some();

    let nats_bin = if should_run_nats || supplied_remote_credentials {
        // Download NATS if not already installed
        spinner.update_spinner_message(" Downloading NATS ...".to_string());
        let nats_binary = ensure_nats_server(&cmd.nats_opts.nats_version, &install_dir).await?;

        spinner.update_spinner_message(" Starting NATS ...".to_string());

        let nats_config = NatsConfig {
            host: nats_host.clone(),
            port: nats_port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{nats_port}")),
            js_domain: cmd.nats_opts.nats_js_domain,
            remote_url: cmd.nats_opts.nats_remote_url,
            credentials: cmd.nats_opts.nats_credsfile.clone(),
            websocket_port: cmd.nats_opts.nats_websocket_port,
            config_path: cmd.nats_opts.nats_configfile,
        };
        let nats_log_path = install_dir.join("nats.log");
        start_nats(
            &install_dir,
            &nats_binary,
            nats_config,
            &nats_log_path,
            CommandGroupUsage::UseParent,
        )
        .await?;
        Some(nats_binary)
    } else {
        // The user is running their own NATS server, so we don't need to download or start one
        None
    };

    // Based on the options provided for wasmCloud, form a client connection to NATS.
    // If this fails, we should return early since wasmCloud wouldn't be able to connect either
    let client = nats_client_from_wasmcloud_opts(&wasmcloud_opts).await?;

    // Start building the CommandOutput providing some useful information like pids, ports, and logfiles
    let mut out_json = HashMap::new();
    let mut out_text = String::new();
    let lattice = wasmcloud_opts
        .clone()
        .lattice
        .context("missing lattice prefix")?;
    let host_started = Arc::new(AtomicBool::new(false));
    let host_log_path = wasmcloud_opts
        .host_log_path
        .clone()
        .unwrap_or_else(|| install_dir.join("wasmcloud.log"));
    let ctl_client = wasmcloud_opts.clone().into_ctl_client(None).await?;

    if !cmd.wasmcloud_opts.multi_local
        && tokio::fs::try_exists(host_pid_file()?)
            .await
            .is_ok_and(|exists| exists)
    {
        // Check if host is running.
        let host_state = running_host_count(&ctl_client).await?;
        if host_state == WasmCloudHostState::NotRunning {
            eprintln!("🟨 Pid file {:?} exists but no hosts are running. Removing Pid file and proceeding with \"wash up\"",
            host_pid_file()?);
            tokio::fs::remove_file(host_pid_file()?).await?;
        } else if host_state == WasmCloudHostState::MultipleRunning {
            bail!("🟨 Multiple hosts are running. Please use --multi-local to start another");
        } else {
            // Host is already running or starting, so interactive mode cannot continue. Close out as if
            // detached.
            spinner.finish_and_clear();
            if let Some(ref manifest_path) = cmd.wadm_opts.wadm_manifest {
                eprintln!("🟨 Wasmcloud host is already running. Deploying wadm manifest in detached mode.");
                out_json.insert("deployed_wadm_manifest_path".into(), json!(manifest_path));
                // Host has already started, no need to wait.
                match process_wadm_manifest(
                    client.clone(),
                    lattice.clone(),
                    host_started.clone(),
                    host_state,
                    ctl_client,
                    manifest_path.clone(),
                    true,
                )
                .await
                {
                    Ok(_) => out_text.push_str("Deployed wadm manifest"),
                    Err(e) => {
                        let _ = write!(out_text, "Deployment failed {e}");
                    }
                };
            }
            out_text.push_str("🛁 wash up completed successfully, already running");
            out_json.insert("success".to_string(), json!(true));
            let _ = write!(
                out_text,
                "\n🕸  NATS is running in the background at {nats_listen_address}"
            );

            let _ = write!(
                out_text,
                "\n📜 Logs for the host are being written to {}",
                host_log_path.to_string_lossy()
            );
            let _ = write!(out_text, "\n\n⬇️  To stop wasmCloud, run \"wash down\"");
            return Ok(CommandOutput::new(out_text, out_json));
        }
    }

    let wadm_process = if !cmd.wadm_opts.disable_wadm
        && !is_wadm_running(
            &nats_host,
            nats_port,
            cmd.nats_opts.nats_credsfile.clone(),
            &lattice,
        )
        .await
        .unwrap_or(false)
    {
        spinner.update_spinner_message(" Starting wadm ...".to_string());
        let config = WadmConfig {
            structured_logging: wasmcloud_opts.enable_structured_logging,
            js_domain: cmd.wadm_opts.wadm_js_domain.clone(),
            nats_server_url: nats_listen_address.clone(),
            nats_credsfile: cmd.nats_opts.nats_credsfile,
        };
        // Start wadm, redirecting output to a log file
        let wadm_log_path = install_dir.join("wadm.log");
        let wadm_log_file = tokio::fs::File::create(&wadm_log_path)
            .await?
            .into_std()
            .await;

        let wadm_path =
            install_patch_or_default_wadm_version(&cmd.wadm_opts.wadm_version, &install_dir).await;
        match wadm_path {
            Ok(wadm_bin_path) => {
                let wadm_child = start_wadm(
                    &install_dir,
                    &wadm_bin_path,
                    wadm_log_file,
                    Some(config),
                    CommandGroupUsage::UseParent,
                )
                .await;
                if let Err(e) = &wadm_child {
                    eprintln!("🟨 Couldn't start wadm: {e}");
                    None
                } else {
                    Some(wadm_child.unwrap())
                }
            }
            Err(e) => {
                let wadm_version: String = cmd
                    .wadm_opts
                    .wadm_version
                    .unwrap_or(WADM_VERSION.to_string());
                eprintln!("🟨 Couldn't download wadm {wadm_version}: {e}");
                if e.to_string().contains("Text file busy") {
                    eprintln!("🛟 Please ensure there aren't any leftover wadm processes");
                }
                None
            }
        }
    } else {
        None
    };

    let wasmcloud_version =
        get_wasmcloud_patch_version_or_default(wasmcloud_opts.wasmcloud_version).await;

    // Download wasmCloud if not already installed
    let wasmcloud_bin_path = match wasmcloud_opts.host_path {
        // If an override was provided we can use it
        Some(path) => {
            debug!(
                wasmcloud_bin_path = %path.display(),
                "using custom wasmcloud binary path"
            );
            path
        }
        // If start only was not specified, we can download the binary
        None if !wasmcloud_opts.start_only => {
            spinner.update_spinner_message(" Downloading wasmCloud ...".to_string());

            ensure_wasmcloud(&wasmcloud_version, &install_dir).await?
        }
        // If no override was provided, we must attempt to find the binary
        None => {
            if let Some(path) = find_wasmcloud_binary(&install_dir, &wasmcloud_version).await {
                path
            } else {
                // Ensure we clean up the NATS server and wadm if we can't start wasmCloud
                if let Some(child) = wadm_process {
                    stop_wadm(child, &install_dir).await?;
                }
                if nats_bin.is_some() {
                    let nats_bin = install_dir.join(NATS_SERVER_BINARY);
                    stop_nats(install_dir, nats_bin).await?;
                }
                bail!("wasmCloud was not installed, exiting without downloading as --wasmcloud-start-only was set");
            }
        }
    };

    // Redirect output (which is on stderr) to a log file in detached mode, or use the terminal
    spinner.update_spinner_message(" Starting wasmCloud ...".to_string());
    let stderr: Stdio = if cmd.detached {
        tokio::fs::File::create(&host_log_path)
            .await?
            .into_std()
            .await
            .into()
    } else {
        Stdio::piped()
    };

    let mut wasmcloud_child = match start_wasmcloud_host(
        &wasmcloud_bin_path,
        std::process::Stdio::null(),
        stderr,
        host_env,
    )
    .await
    {
        Ok(child) => child,
        Err(e) => {
            // Ensure we clean up the NATS server and wadm if we can't start wasmCloud
            if let Some(child) = wadm_process {
                stop_wadm(child, &install_dir).await?;
            }
            if nats_bin.is_some() {
                let nats_bin = install_dir.join(NATS_SERVER_BINARY);
                stop_nats(install_dir, nats_bin).await?;
            }
            return Err(e);
        }
    };

    spinner.finish_and_clear();

    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash up completed successfully");

    if let Some(ref manifest_path) = cmd.wadm_opts.wadm_manifest {
        out_json.insert("deployed_wadm_manifest_path".into(), json!(manifest_path));
        match process_wadm_manifest(
            client.clone(),
            lattice.clone(),
            host_started.clone(),
            WasmCloudHostState::NotRunning,
            ctl_client,
            manifest_path.clone(),
            cmd.detached,
        )
        .await
        {
            Ok(_) => out_text.push_str("Deployed wadm manifest"),
            Err(e) => {
                let _ = write!(out_text, "Deployment failed {e}");
            }
        };
    }

    // Write the pid file with the selected version and process ID.
    let pid_file_contents = json!({
        "version": wasmcloud_version,
        "pid": wasmcloud_child.id().unwrap()
    });

    tokio::fs::write(host_pid_file()?, pid_file_contents.to_string()).await?;

    // If we're running in detached mode, then we can print out some logs, build output and return early.
    if cmd.detached {
        out_json.insert("wasmcloud_log".to_string(), json!(host_log_path));
        out_json.insert("kill_cmd".to_string(), json!("wash down"));
        out_json.insert("nats_url".to_string(), json!(nats_listen_address));

        let _ = write!(
            out_text,
            "\n🕸  NATS is running in the background at {nats_listen_address}"
        );

        let _ = write!(
            out_text,
            "\n📜 Logs for the host are being written to {}",
            host_log_path.to_string_lossy()
        );
        let _ = write!(out_text, "\n\n⬇️  To stop wasmCloud, run \"wash down\"");
        return Ok(CommandOutput::new(out_text, out_json));
    }

    // If we're running in interactive mode, let's start the host
    run_wasmcloud_interactive(
        &mut wasmcloud_child,
        cmd.wadm_opts.wadm_manifest,
        host_started.clone(),
        output_kind,
    )
    .await?;

    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(
        // wadm and NATS both exit immediately when sent SIGINT
        "CTRL+c received, stopping wasmCloud, wadm, and NATS...".to_string(),
    );
    stop_wasmcloud(wasmcloud_child).await?;
    if let Err(e) = tokio::fs::remove_file(host_pid_file()?).await {
        warn!("failed to remove host pid file: {e}");
    };

    if let Some(mut wadm_process) = wadm_process {
        if let Err(e) = wadm_process.kill().await {
            warn!("failed to kill wadm: {e}");
        };
        // remove wadm pidfile, the process is stopped automatically by CTRL+c
        remove_wadm_pidfile(&install_dir).await?;
    }

    spinner.finish_and_clear();
    Ok(CommandOutput::new(out_text, out_json))
}

async fn get_wasmcloud_patch_version_or_default(version: Option<String>) -> Version {
    if let Some(version) = parse_version_string(version) {
        return version;
    }

    match new_patch_or_pre_1_0_0_minor_version_after_version_string(
        GITHUB_WASMCLOUD_ORG,
        GITHUB_WASMCLOUD_WASMCLOUD_REPO,
        WASMCLOUD_HOST_VERSION,
        None,
    )
    .await
    {
        Ok(version) => version,
        _ => {
            debug!("No new patch version found for wasmCloud, using {WASMCLOUD_HOST_VERSION}");
            WASMCLOUD_HOST_VERSION_T
        }
    }
}

/// Check if a wasmcloud host is running
async fn running_host_count(ctl_client: &CtlClient) -> Result<WasmCloudHostState> {
    match ctl_client
        .get_hosts()
        .await
        .map_err(|e| anyhow!(e))?
        .into_iter()
        .filter_map(wasmcloud_control_interface::CtlResponse::into_data)
        .count()
    {
        1 => return Ok(WasmCloudHostState::Running),
        2.. => return Ok(WasmCloudHostState::MultipleRunning),
        _ => (),
    }

    // Wasmcloud host might be starting but not up yet. Check if the process in the pid file is running.
    let pid_file_string = tokio::fs::read_to_string(host_pid_file()?).await?;
    let pid_file_value: Value = serde_json::from_str(&pid_file_string)?;
    if let Some(pid) = pid_file_value.get("pid") {
        if is_process_running(&pid.to_string()) {
            return Ok(WasmCloudHostState::Starting);
        }
    }
    Ok(WasmCloudHostState::NotRunning)
}

/// Check if a process with the given PID is running
///
/// # Arguments
///
/// * `pid` - A string representation of the process ID to check
///
/// # Returns
///
/// * `true` if the process is running, `false` otherwise
///
/// # Panics
///
/// This function uses `panic::catch_unwind` internally to handle potential panics
/// from the `sysinfo` library, but should not panic itself.
fn is_process_running(pid: &str) -> bool {
    use std::panic::{self, AssertUnwindSafe};

    // Try to parse the PID as a u32
    let Ok(pid_u32) = pid.parse::<u32>() else {
        return false;
    };

    // Convert to usize for sysinfo::Pid
    let pid_usize = pid_u32 as usize;

    // Check if PID is unreasonably large (likely invalid)
    if pid_usize > 100_000 {
        return false;
    }

    // Special case for PID 1 which might cause issues on some systems
    if pid_usize == 1 {
        return true;
    }

    // Wrap the potentially unsafe operations in catch_unwind to prevent panics
    panic::catch_unwind(AssertUnwindSafe(|| {
        // Create sysinfo::Pid safely
        let sys_pid = sysinfo::Pid::from(pid_usize);

        // Create a new system instance
        let Ok(sys) = panic::catch_unwind(AssertUnwindSafe(System::new_all)) else {
            return false;
        };

        // Refresh processes
        let mut sys = sys;
        if panic::catch_unwind(AssertUnwindSafe(|| {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true)
        }))
        .is_err()
        {
            return false;
        }

        // Check if the process exists
        panic::catch_unwind(AssertUnwindSafe(|| sys.processes().get(&sys_pid).is_some()))
            .unwrap_or(false)
    }))
    .unwrap_or(false)
}

/// Spawn off a task that waits until the host has started,
/// then loads and deploys the WADM manifest.
#[allow(clippy::too_many_arguments)]
fn process_wadm_manifest(
    client: Client,
    lattice: String,
    host_started: Arc<AtomicBool>,
    host_state: WasmCloudHostState,
    ctl_client: CtlClient,
    manifest_path: PathBuf,
    detached: bool,
) -> tokio::task::JoinHandle<std::result::Result<(), anyhow::Error>> {
    // Spawn a task that waits for the host to start
    tokio::spawn(async move {
        if detached && host_state < WasmCloudHostState::Running {
            tokio::time::timeout(tokio::time::Duration::from_secs(3), async {
                loop {
                    if matches!(
                        running_host_count(&ctl_client).await,
                        Ok(WasmCloudHostState::Running)
                    ) {
                        break;
                    }
                }
            })
            .await
            .context("failed to wait for host start while deploying WADM application")?;
        } else if !detached {
            // If the host was *not* detached, wait until host_started is updated from run_wasmcloud_interactive()
            while !host_started.load(Ordering::SeqCst) {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        };

        // Load the manifest, now that we're done waiting
        let manifest = load_app_manifest(AppManifestSource::File(manifest_path.clone()))
            .await
            .with_context(|| {
                format!(
                    "failed to load manifest from path [{}]",
                    manifest_path.display()
                )
            })?;

        // Deploy the WADM application
        deploy_wadm_application(&client, manifest, lattice.as_ref())
            .await
            .with_context(|| {
                format!(
                    "failed to deploy wadm application [{}]",
                    manifest_path.display()
                )
            })?;

        Ok(()) as Result<()>
    })
}

/// Helper function to deploy a WADM application (including removing a previous version)
/// for use when calling `wash up --manifest`
async fn deploy_wadm_application(
    client: &Client,
    manifest: AppManifest,
    lattice: &str,
) -> Result<()> {
    let model_name = manifest.name().context("failed to find model name")?;
    let _ = crate::lib::app::undeploy_model(client, Some(lattice.into()), model_name).await;
    match deploy_model_from_manifest(client, Some(lattice.into()), manifest, None).await {
        // Ignore if the model is already deployed
        Err(e) if e.to_string().contains("already exists") => {}
        // All other failures are unexpected
        Err(e) => bail!(e),
        _ => {}
    }
    Ok(())
}

/// Helper function to start the NATS binary, redirecting output to nats.log
pub(crate) async fn start_nats(
    install_dir: &Path,
    nats_binary: &Path,
    nats_config: NatsConfig,
    nats_log_path: &Path,
    command_group: CommandGroupUsage,
) -> Result<Child> {
    // Ensure that leaf node remote connection can be established before launching NATS
    if let (Some(url), Some(creds)) = (
        nats_config.remote_url.as_ref(),
        nats_config.credentials.as_ref(),
    ) {
        if let Err(e) = create_nats_client_from_opts(
            url,
            &nats_config.port.to_string(),
            None,
            None,
            Some(creds.to_owned()),
            None,
            false,
        )
        .await
        {
            bail!("Could not connect to leafnode remote: {}", e);
        }
    }

    // Start NATS server, redirecting output to a log file
    let nats_log_file = tokio::fs::File::create(&nats_log_path)
        .await?
        .into_std()
        .await;
    let nats_process =
        start_nats_server(nats_binary, nats_log_file, nats_config, command_group).await?;
    eprintln!(
        "{} NATS server successfully started, using config @ [{}]",
        emoji::INFO_SQUARE,
        nats_binary
            .parent()
            .context("unexpectedly missing parent dir")?
            .join(NATS_SERVER_CONF)
            .display()
    );
    eprintln!(
        "{} NATS server logs written to [{}]",
        emoji::INFO_SQUARE,
        nats_log_path.display()
    );

    // save the PID so we can kill it later
    if let Some(pid) = nats_process.id() {
        let pid_file = nats_pid_path(install_dir);
        tokio::fs::write(&pid_file, pid.to_string()).await?;
    }

    Ok(nats_process)
}

/// Helper function to run an optimistic patch update, but fall back to the previous version if the new version fails
/// to download.
async fn install_patch_or_default_wadm_version(
    version: &Option<String>,
    install_dir: &Path,
) -> Result<PathBuf> {
    if let Some(version) = version {
        return ensure_wadm(version, install_dir).await;
    }

    let version = version.clone().unwrap_or_else(|| WADM_VERSION.to_owned());
    if let Ok(new_patch_version) = new_patch_or_pre_1_0_0_minor_version_after_version_string(
        GITHUB_WASMCLOUD_ORG,
        GITHUB_WASMCLOUD_WADM_REPO,
        &version,
        None,
    )
    .await
    {
        eprintln!(
            "{} Found a new patch version of wadm: {}",
            emoji::INFO_SQUARE,
            new_patch_version
        );
        // Re-add stripped 'v' prefix due to semver parsing
        let new_version = format!("v{new_patch_version}");
        match ensure_wadm(&new_version, install_dir).await {
            Ok(path) => Ok(path),
            Err(e) => {
                debug!(
                    "🟨 Couldn't download the patched wadm {new_version}, falling back to {version}: {e}"
                );
                ensure_wadm(&version, install_dir).await
            }
        }
    } else {
        debug!("No new version found, using the provided: {}", version);
        ensure_wadm(&version, install_dir).await
    }
}

/// Helper function to run wasmCloud in interactive mode
async fn run_wasmcloud_interactive(
    wasmcloud_child: &mut Child,
    wadm_manifest: Option<PathBuf>,
    host_started: Arc<AtomicBool>,
    output_kind: OutputKind,
) -> Result<()> {
    use std::sync::mpsc::channel;
    let (running_sender, running_receiver) = channel();
    let running = Arc::new(AtomicBool::new(true));

    // Handle Ctrl + c with Tokio
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .context("failed to wait for ctrl_c signal")?;
        // Set the host as not running
        if running.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            let _ = running_sender.send(true);
        } else {
            warn!("\nRepeated CTRL+C received, killing wasmCloud and NATS. This may result in zombie processes");
        }
        Result::<_, anyhow::Error>::Ok(())
    });

    if output_kind != OutputKind::Json {
        println!("🏃 Running in interactive mode.");
        if let Some(ref manifest_path) = wadm_manifest {
            println!(
                "🚀 Deploying WADM manifest at [{}]",
                manifest_path.display()
            );
        }
        println!("🎛️ To start the dashboard, run `wash ui`");
        println!("🚪 Press `CTRL+c` at any time to exit");
    }

    // Create a separate thread to log host output
    let handle = wasmcloud_child.stderr.take().map(|stderr| {
        tokio::spawn(async {
            let mut lines = BufReader::new(stderr).lines();
            loop {
                if let Ok(Some(line)) = lines.next_line().await {
                    // TODO(brooksmtownsend): in the future, would be great to print these in a prettier format
                    println!("{line}");
                }
            }
        })
    });

    // Mark the host as started
    host_started.store(true, Ordering::SeqCst);

    // Wait for the user to send Ctrl+C in a thread where blocking is acceptable
    let _ = running_receiver.recv();

    // Prevent extraneous messages from the host getting printed as the host shuts down
    if let Some(handle) = handle {
        handle.abort();
    };
    Ok(())
}

#[cfg(unix)]
async fn stop_wasmcloud(mut wasmcloud_child: Child) -> Result<()> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    if let Some(pid) = wasmcloud_child.id() {
        // Send the SIGTERM signal to ensure that wasmcloud is graceful shutdown.
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)?;

        // TODO(iceber): the timeout for the SIGTERM could be added in the future,
        // but it doesn't look like it's needed yet.
        wasmcloud_child.wait().await?;
    }
    Ok(())
}

#[cfg(target_family = "windows")]
async fn stop_wasmcloud(mut wasmcloud_child: Child) -> Result<()> {
    wasmcloud_child.kill().await?;
    Ok(())
}

async fn is_wadm_running(
    nats_host: &str,
    nats_port: u16,
    credsfile: Option<PathBuf>,
    lattice: &str,
) -> Result<bool> {
    let client = create_nats_client_from_opts(
        nats_host,
        &nats_port.to_string(),
        None,
        None,
        credsfile,
        None,
        false,
    )
    .await?;

    Ok(
        crate::lib::app::get_models(&client, Some(lattice.to_string()))
            .await
            .is_ok(),
    )
}

async fn stop_wadm<P>(mut wadm: Child, install_dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    wadm.kill().await?;
    remove_wadm_pidfile(install_dir).await
}

pub(crate) async fn remove_wadm_pidfile<P>(install_dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    if let Err(err) = tokio::fs::remove_file(install_dir.as_ref().join(WADM_PID_FILE)).await {
        if err.kind() != ErrorKind::NotFound {
            bail!(err);
        }
    }
    Ok(())
}

/// Helper function to create a NATS client from the same arguments wasmCloud will use
pub(crate) async fn nats_client_from_wasmcloud_opts(
    wasmcloud_opts: &WasmcloudOpts,
) -> Result<Client> {
    create_nats_client_from_opts(
        &wasmcloud_opts
            .ctl_host
            .clone()
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string()),
        &wasmcloud_opts
            .ctl_port
            .map_or_else(|| DEFAULT_NATS_PORT.to_string(), |port| port.to_string()),
        wasmcloud_opts.ctl_jwt.clone(),
        wasmcloud_opts.ctl_seed.clone(),
        wasmcloud_opts.ctl_credsfile.clone(),
        wasmcloud_opts.ctl_tls_ca_file.clone(),
        wasmcloud_opts.ctl_tls_first,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::UpCommand;
    use anyhow::Result;
    use clap::Parser;

    const LOCAL_REGISTRY: &str = "localhost:5001";

    // Assert that our API doesn't unknowingly drift
    #[test]
    fn test_up_comprehensive() -> Result<()> {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        let up_all_flags: UpCommand = Parser::try_parse_from([
            "up",
            "--allow-latest",
            "--allowed-insecure",
            LOCAL_REGISTRY,
            "--cluster-issuers",
            "CBZZ6BLE7PIJNCEJMXOHAJ65KIXRVXDA74W6LUKXC4EPFHTJREXQCOYI",
            "--cluster-seed",
            "SCAKLQ2FFT4LZUUVQMH6N37US3IZUEVJBUR3V532VV3DAAHSZXPQY6DYIM",
            "--config-service-enabled",
            "--ctl-credsfile",
            TESTDIR,
            "--ctl-host",
            "127.0.0.2",
            "--ctl-jwt",
            "eyyjWT",
            "--ctl-port",
            "4232",
            "--ctl-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--ctl-tls",
            "--enable-ipv6",
            "--enable-structured-logging",
            "--host-seed",
            "SNAP4UVNHVWSBJ5MHAQ6M3RB23S3ALA3O3A4RF25G2FQB5CCZJBBBWCKBY",
            "--detached",
            "--nats-credsfile",
            TESTDIR,
            "--nats-host",
            "127.0.0.2",
            "--nats-js-domain",
            "domain",
            "--nats-port",
            "4232",
            "--nats-remote-url",
            "tls://remote.global",
            "--nats-version",
            "v2.11.3",
            "--provider-delay",
            "500",
            "--rpc-credsfile",
            TESTDIR,
            "--rpc-host",
            "127.0.0.2",
            "--rpc-jwt",
            "eyyjWT",
            "--rpc-port",
            "4232",
            "--rpc-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--rpc-timeout-ms",
            "500",
            "--rpc-tls",
            "--structured-log-level",
            "warn",
            "--wasmcloud-js-domain",
            "domain",
            "--wasmcloud-version",
            "v0.57.1",
            "--lattice",
            "anotherprefix",
        ])?;
        assert!(up_all_flags.wasmcloud_opts.allow_latest);
        assert_eq!(
            up_all_flags.wasmcloud_opts.allowed_insecure,
            Some(vec![LOCAL_REGISTRY.to_string()])
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.cluster_issuers,
            Some(vec![
                "CBZZ6BLE7PIJNCEJMXOHAJ65KIXRVXDA74W6LUKXC4EPFHTJREXQCOYI".to_string()
            ])
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.cluster_seed,
            Some("SCAKLQ2FFT4LZUUVQMH6N37US3IZUEVJBUR3V532VV3DAAHSZXPQY6DYIM".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.config_service_enabled);
        assert!(!up_all_flags.nats_opts.connect_only);
        assert!(up_all_flags.wasmcloud_opts.ctl_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.ctl_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.ctl_tls);
        assert!(up_all_flags.wasmcloud_opts.rpc_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.rpc_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.rpc_tls);
        assert!(up_all_flags.wasmcloud_opts.enable_ipv6);
        assert!(up_all_flags.wasmcloud_opts.enable_structured_logging);
        assert_eq!(
            up_all_flags.wasmcloud_opts.host_seed,
            Some("SNAP4UVNHVWSBJ5MHAQ6M3RB23S3ALA3O3A4RF25G2FQB5CCZJBBBWCKBY".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.structured_log_level,
            "warn".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.wasmcloud_version,
            Some("v0.57.1".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.lattice.unwrap(),
            "anotherprefix".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.wasmcloud_js_domain,
            Some("domain".to_string())
        );
        assert_eq!(up_all_flags.nats_opts.nats_version, "v2.11.3".to_string());
        assert_eq!(
            up_all_flags.nats_opts.nats_remote_url,
            Some("tls://remote.global".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.provider_delay, 500);
        assert!(up_all_flags.detached);

        Ok(())
    }

    #[test]
    fn test_is_process_running() {
        // Test with an invalid PID string
        assert!(
            !super::is_process_running("not-a-pid"),
            "Invalid PID string should return false"
        );

        // Test with a non-existent PID (a very large number)
        assert!(
            !super::is_process_running("999999999"),
            "Non-existent PID should return false"
        );

        // Test with a simple valid case - we'll use a small number that's unlikely to be a real PID
        // This just tests the parsing logic, not actual process existence
        let result = super::is_process_running("1");
        // We don't assert the result since PID 1 might or might not exist depending on the system
        // Just make sure it doesn't crash
        println!("PID 1 exists: {result}");
    }
}
