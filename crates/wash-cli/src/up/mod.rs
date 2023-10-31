use std::collections::HashMap;
use std::fmt::Write;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{bail, Context, Result};
use async_nats::Client;
use clap::Parser;
use serde_json::json;

use tokio::fs::create_dir_all;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
};
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::downloads_dir;
use wash_lib::config::DEFAULT_NATS_TIMEOUT_MS;
use wash_lib::start::ensure_wadm;
use wash_lib::start::find_wasmcloud_binary;
use wash_lib::start::nats_pid_path;
use wash_lib::start::start_wadm;
use wash_lib::start::WadmConfig;
use wash_lib::start::{
    ensure_nats_server, ensure_wasmcloud, start_nats_server, start_wasmcloud_host, NatsConfig,
    WADM_PID,
};
use wasmcloud_control_interface::{Client as CtlClient, ClientBuilder as CtlClientBuilder};

use crate::appearance::spinner::Spinner;
use crate::down::stop_nats;
use crate::util::nats_client_from_opts;

mod config;
mod credsfile;
pub use config::*;

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

    /// Optional remote URL of existing NATS infrastructure to extend.
    #[clap(
        long = "nats-remote-url",
        env = "NATS_REMOTE_URL",
        requires = "nats_credsfile"
    )]
    pub nats_remote_url: Option<String>,

    /// If a connection can't be established, exit and don't start a NATS server. Will be ignored if a remote_url and credsfile are specified
    #[clap(
        long = "nats-connect-only",
        env = "NATS_CONNECT_ONLY",
        conflicts_with = "nats_remote_url"
    )]
    pub connect_only: bool,

    /// NATS server version to download, e.g. `v2.7.2`. See https://github.com/nats-io/nats-server/releases/ for releases
    #[clap(long = "nats-version", default_value = NATS_SERVER_VERSION, env = "NATS_VERSION")]
    pub nats_version: String,

    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = DEFAULT_NATS_HOST, env = "NATS_HOST")]
    pub nats_host: String,

    /// NATS server port to connect to. This will be used as the NATS listen port if `--nats-connect-only` isn't set
    #[clap(long = "nats-port", default_value = DEFAULT_NATS_PORT, env = "NATS_PORT")]
    pub nats_port: u16,

    /// NATS websocket port to use. Websocket support will not be enabled if this option isn't set. TLS is not supported. This is required for the wash ui to connect from localhost
    #[clap(long = "nats-websocket-port", env = "NATS_WEBSOCKET_PORT")]
    pub nats_websocket_port: Option<u16>,

    /// NATS Server Jetstream domain, defaults to `core`
    #[clap(long = "nats-js-domain", env = "NATS_JS_DOMAIN")]
    pub nats_js_domain: Option<String>,
}

impl From<NatsOpts> for NatsConfig {
    fn from(other: NatsOpts) -> NatsConfig {
        NatsConfig {
            host: other.nats_host,
            port: other.nats_port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{}", other.nats_port)),
            js_domain: other.nats_js_domain,
            remote_url: other.nats_remote_url,
            credentials: other.nats_credsfile,
            websocket_port: other.nats_websocket_port,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct WasmcloudOpts {
    /// wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud-otp/releases for releases
    #[clap(long = "wasmcloud-version", default_value = WASMCLOUD_HOST_VERSION, env = "WASMCLOUD_VERSION")]
    pub wasmcloud_version: String,

    /// A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices
    #[clap(
        short = 'x',
        long = "lattice-prefix",
        default_value = DEFAULT_LATTICE_PREFIX,
        env = WASMCLOUD_LATTICE_PREFIX,
    )]
    pub lattice_prefix: String,

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
    pub rpc_timeout_ms: u32,

    /// A user JWT to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-jwt", env = WASMCLOUD_RPC_JWT, requires = "rpc_seed")]
    pub rpc_jwt: Option<String>,

    /// Optional flag to enable host communication with a NATS server over TLS for RPC messages
    #[clap(long = "rpc-tls", env = WASMCLOUD_RPC_TLS)]
    pub rpc_tls: bool,

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

    /// Denotes if a wasmCloud host should allow starting actors from the file system
    #[clap(long = "allow-file-load", default_value = DEFAULT_ALLOW_FILE_LOAD, env = WASMCLOUD_ALLOW_FILE_LOAD)]
    pub allow_file_load: Option<bool>,

    /// Enable JSON structured logging from the wasmCloud host
    #[clap(
        long = "enable-structured-logging",
        env = WASMCLOUD_STRUCTURED_LOGGING_ENABLED
    )]
    pub enable_structured_logging: bool,

    /// Controls the verbosity of JSON structured logs from the wasmCloud host
    #[clap(long = "log-level", alias = "structured-log-level", default_value = DEFAULT_STRUCTURED_LOG_LEVEL, env = WASMCLOUD_LOG_LEVEL)]
    pub structured_log_level: String,

    /// Enables IPV6 addressing for wasmCloud hosts
    #[clap(long = "enable-ipv6", env = WASMCLOUD_ENABLE_IPV6)]
    pub enable_ipv6: bool,

    /// If enabled, wasmCloud will not be downloaded if it's not installed
    #[clap(long = "wasmcloud-start-only")]
    pub start_only: bool,
}

impl WasmcloudOpts {
    pub async fn into_ctl_client(self, auction_timeout_ms: Option<u64>) -> Result<CtlClient> {
        let lattice_prefix = self.lattice_prefix;
        let ctl_host = self
            .ctl_host
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string());
        let ctl_port = self.ctl_port.unwrap_or(4222).to_string();
        let auction_timeout_ms = auction_timeout_ms.unwrap_or(DEFAULT_NATS_TIMEOUT_MS);

        let nc = nats_client_from_opts(
            &ctl_host,
            &ctl_port,
            self.ctl_jwt,
            self.ctl_seed,
            self.ctl_credsfile,
        )
        .await
        .context("Failed to create NATS client")?;

        let mut builder = CtlClientBuilder::new(nc)
            .lattice_prefix(lattice_prefix)
            .timeout(tokio::time::Duration::from_millis(
                self.rpc_timeout_ms.into(),
            ))
            .auction_timeout(tokio::time::Duration::from_millis(auction_timeout_ms));

        if let Ok(topic_prefix) = std::env::var("WASMCLOUD_CTL_TOPIC_PREFIX") {
            builder = builder.topic_prefix(topic_prefix);
        }

        let ctl_client = builder.build();

        Ok(ctl_client)
    }
}

#[derive(Parser, Debug, Clone)]
pub struct WadmOpts {
    /// wadm version to download, e.g. `v0.4.0`. See https://github.com/wasmCloud/wadm/releases for releases
    #[clap(long = "wadm-version", default_value = WADM_VERSION, env = "WADM_VERSION")]
    pub wadm_version: String,

    #[clap(long = "disable-wadm")]
    pub disable_wadm: bool,
}

pub async fn handle_command(command: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    handle_up(command, output_kind).await
}

pub async fn handle_up(cmd: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = downloads_dir()?;
    create_dir_all(&install_dir).await?;
    let spinner = Spinner::new(&output_kind)?;

    // Ensure we use the open dashboard port and the supplied NATS host/port if no overrides were supplied
    let wasmcloud_opts = WasmcloudOpts {
        ctl_host: Some(
            cmd.wasmcloud_opts
                .ctl_host
                .unwrap_or_else(|| cmd.nats_opts.nats_host.to_owned()),
        ),
        ctl_port: Some(
            cmd.wasmcloud_opts
                .ctl_port
                .unwrap_or(cmd.nats_opts.nats_port),
        ),
        rpc_host: Some(
            cmd.wasmcloud_opts
                .rpc_host
                .unwrap_or_else(|| cmd.nats_opts.nats_host.to_owned()),
        ),
        rpc_port: Some(
            cmd.wasmcloud_opts
                .rpc_port
                .unwrap_or(cmd.nats_opts.nats_port),
        ),
        ..cmd.wasmcloud_opts
    };
    // Capture listen address to keep the value after the nats_opts are moved
    let nats_listen_address = format!("{}:{}", cmd.nats_opts.nats_host, cmd.nats_opts.nats_port);

    let nats_client = nats_client_from_wasmcloud_opts(&wasmcloud_opts).await;
    let nats_opts = cmd.nats_opts.clone();

    // Avoid downloading + starting NATS if the user already runs their own server and we can connect.
    let should_run_nats = !cmd.nats_opts.connect_only && nats_client.is_err();
    // Ignore connect_only if this server has a remote and credsfile as we have to start a leafnode in that scenario
    let supplied_remote_credentials =
        cmd.nats_opts.nats_remote_url.is_some() && cmd.nats_opts.nats_credsfile.is_some();

    let nats_bin = if should_run_nats || supplied_remote_credentials {
        // Download NATS if not already installed
        spinner.update_spinner_message(" Downloading NATS ...".to_string());
        let nats_binary = ensure_nats_server(&cmd.nats_opts.nats_version, &install_dir).await?;

        spinner.update_spinner_message(" Starting NATS ...".to_string());
        start_nats(&install_dir, &nats_binary, cmd.nats_opts.clone()).await?;
        Some(nats_binary)
    } else {
        // The user is running their own NATS server, so we don't need to download or start one
        None
    };

    // Based on the options provided for wasmCloud, form a client connection to NATS.
    // If this fails, we should return early since wasmCloud wouldn't be able to connect either
    nats_client_from_wasmcloud_opts(&wasmcloud_opts).await?;

    let wadm_process = if !cmd.wadm_opts.disable_wadm
        && !is_wadm_running(&nats_opts, &wasmcloud_opts.lattice_prefix)
            .await
            .unwrap_or(false)
    {
        spinner.update_spinner_message(" Starting wadm ...".to_string());
        let config = WadmConfig {
            structured_logging: cmd.wasmcloud_opts.enable_structured_logging,
            js_domain: cmd.nats_opts.nats_js_domain.clone(),
            nats_server_url: format!("{}:{}", cmd.nats_opts.nats_host, cmd.nats_opts.nats_port),
            nats_credsfile: cmd.nats_opts.nats_credsfile,
        };
        // Start wadm, redirecting output to a log file
        let wadm_log_path = install_dir.join("wadm.log");
        let wadm_log_file = tokio::fs::File::create(&wadm_log_path)
            .await?
            .into_std()
            .await;

        let wadm_path = ensure_wadm(&cmd.wadm_opts.wadm_version, &install_dir).await;
        match wadm_path {
            Ok(path) => {
                let wadm_child = start_wadm(&path, wadm_log_file, Some(config)).await;
                if let Err(e) = &wadm_child {
                    println!("🟨 Couldn't start wadm: {e}");
                    None
                } else {
                    Some(wadm_child.unwrap())
                }
            }
            Err(e) => {
                println!("🟨 Couldn't download wadm {WADM_VERSION}: {e}");
                None
            }
        }
    } else {
        None
    };

    // Download wasmCloud if not already installed
    let wasmcloud_executable = if !cmd.wasmcloud_opts.start_only {
        spinner.update_spinner_message(" Downloading wasmCloud ...".to_string());
        ensure_wasmcloud(&wasmcloud_opts.wasmcloud_version, &install_dir).await?
    } else if let Some(wasmcloud_bin) =
        find_wasmcloud_binary(&install_dir, &wasmcloud_opts.wasmcloud_version).await
    {
        wasmcloud_bin
    } else {
        // Ensure we clean up the NATS server and wadm if we can't start wasmCloud
        if let Some(child) = wadm_process {
            stop_wadm(child, &install_dir).await?;
        }
        if nats_bin.is_some() {
            stop_nats(install_dir).await?;
        }
        bail!("wasmCloud was not installed, exiting without downloading as --wasmcloud-start-only was set");
    };

    // Redirect output (which is on stderr) to a log file in detached mode, or use the terminal
    spinner.update_spinner_message(" Starting wasmCloud ...".to_string());
    let wasmcloud_log_path = install_dir.join("wasmcloud.log");
    let stderr: Stdio = if cmd.detached {
        tokio::fs::File::create(&wasmcloud_log_path)
            .await?
            .into_std()
            .await
            .into()
    } else {
        Stdio::piped()
    };
    let version = wasmcloud_opts.wasmcloud_version.clone();

    let host_env = configure_host_env(nats_opts, wasmcloud_opts).await;
    let mut wasmcloud_child = match start_wasmcloud_host(
        &wasmcloud_executable,
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
                stop_nats(install_dir).await?;
            }
            return Err(e);
        }
    };

    spinner.finish_and_clear();
    if !cmd.detached {
        run_wasmcloud_interactive(&mut wasmcloud_child, output_kind).await?;

        let spinner = Spinner::new(&output_kind)?;
        spinner.update_spinner_message(
            // wadm and NATS both exit immediately when sent SIGINT
            "CTRL+c received, stopping wasmCloud, wadm, and NATS...".to_string(),
        );

        stop_wasmcloud(wasmcloud_child).await?;

        if wadm_process.is_some() {
            // remove wadm pidfile, the process is stopped automatically by CTRL+c
            remove_wadm_pidfile(&install_dir).await?;
        }

        spinner.finish_and_clear();
    }

    // Build the CommandOutput providing some useful information like pids, ports, and logfiles
    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash up completed successfully");

    if cmd.detached {
        // Write the pid file with the selected version
        tokio::fs::write(install_dir.join(config::WASMCLOUD_PID_FILE), version).await?;
        out_json.insert("wasmcloud_log".to_string(), json!(wasmcloud_log_path));
        out_json.insert("kill_cmd".to_string(), json!("wash down"));
        out_json.insert("nats_url".to_string(), json!(nats_listen_address));

        let _ = write!(
            out_text,
            "\n🕸  NATS is running in the background at http://{nats_listen_address}"
        );

        let _ = write!(
            out_text,
            "\n📜 Logs for the host are being written to {}",
            wasmcloud_log_path.to_string_lossy()
        );
        let _ = write!(out_text, "\n\n⬇️  To stop wasmCloud, run \"wash down\"");
    }

    Ok(CommandOutput::new(out_text, out_json))
}

/// Helper function to start the NATS binary, redirecting output to nats.log
async fn start_nats(install_dir: &Path, nats_binary: &Path, nats_opts: NatsOpts) -> Result<Child> {
    // Ensure that leaf node remote connection can be established before launching NATS
    let nats_opts = match (
        nats_opts.nats_remote_url.as_ref(),
        nats_opts.nats_credsfile.as_ref(),
    ) {
        (Some(url), Some(creds)) => {
            if let Err(e) = crate::util::nats_client_from_opts(
                url,
                &nats_opts.nats_port.to_string(),
                None,
                None,
                Some(creds.to_owned()),
            )
            .await
            {
                bail!("Could not connect to leafnode remote: {}", e);
            } else {
                nats_opts
            }
        }
        (_, _) => nats_opts,
    };
    // Start NATS server, redirecting output to a log file
    let nats_log_path = install_dir.join("nats.log");
    let nats_log_file = tokio::fs::File::create(&nats_log_path)
        .await?
        .into_std()
        .await;
    let nats_process = start_nats_server(nats_binary, nats_log_file, nats_opts.into()).await?;

    // save the PID so we can kill it later
    if let Some(pid) = nats_process.id() {
        let pid_file = nats_pid_path(install_dir);
        tokio::fs::write(&pid_file, pid.to_string()).await?;
    }

    Ok(nats_process)
}

/// Helper function to run wasmCloud in interactive mode
async fn run_wasmcloud_interactive(
    wasmcloud_child: &mut Child,
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
        if running.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            let _ = running_sender.send(true);
        } else {
            log::warn!("\nRepeated CTRL+C received, killing wasmCloud and NATS. This may result in zombie processes")
        }
        Result::<_, anyhow::Error>::Ok(())
    });

    if output_kind != OutputKind::Json {
        println!("🏃 Running in interactive mode.",);
        println!("🎛️  If you enabled --nats-websocket-port, start the dashboard by executing `wash ui --experimental`");
        println!("🚪 Press `CTRL+c` at any time to exit");
    }

    // Create a separate thread to log host output
    let handle = wasmcloud_child.stderr.take().map(|stderr| {
        tokio::spawn(async {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                //TODO(brooksmtownsend): in the future, would be great to print these in a prettier format
                println!("{line}")
            }
        })
    });

    // Wait for the user to send Ctrl+C in a thread where blocking is acceptable
    let _ = running_receiver.recv();

    // Prevent extraneous messages from the host getting printed as the host shuts down
    if let Some(handle) = handle {
        handle.abort()
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

async fn is_wadm_running(nats_opts: &NatsOpts, lattice_prefix: &str) -> Result<bool> {
    let client = nats_client_from_opts(
        &nats_opts.nats_host,
        &nats_opts.nats_port.to_string(),
        None,
        None,
        nats_opts.nats_credsfile.clone(),
    )
    .await?;

    Ok(
        wash_lib::app::get_models(&client, Some(lattice_prefix.to_string()))
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

async fn remove_wadm_pidfile<P>(install_dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    if let Err(err) = tokio::fs::remove_file(install_dir.as_ref().join(WADM_PID)).await {
        if err.kind() != ErrorKind::NotFound {
            bail!(err);
        }
    }
    Ok(())
}

/// Helper function to create a NATS client from the same arguments wasmCloud will use
async fn nats_client_from_wasmcloud_opts(wasmcloud_opts: &WasmcloudOpts) -> Result<Client> {
    nats_client_from_opts(
        &wasmcloud_opts
            .ctl_host
            .clone()
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string()),
        &wasmcloud_opts
            .ctl_port
            .map(|port| port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string()),
        wasmcloud_opts.ctl_jwt.clone(),
        wasmcloud_opts.ctl_seed.clone(),
        wasmcloud_opts.ctl_credsfile.clone(),
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
            "v2.8.4",
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
            "--lattice-prefix",
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
            "v0.57.1".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.lattice_prefix,
            "anotherprefix".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.wasmcloud_js_domain,
            Some("domain".to_string())
        );
        assert_eq!(up_all_flags.nats_opts.nats_version, "v2.8.4".to_string());
        assert_eq!(
            up_all_flags.nats_opts.nats_remote_url,
            Some("tls://remote.global".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.provider_delay, 500);
        assert!(up_all_flags.detached);

        Ok(())
    }
}
