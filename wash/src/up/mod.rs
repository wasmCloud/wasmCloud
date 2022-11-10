use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::fs::create_dir_all;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
};

use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::down::stop_nats;
use crate::down::stop_wasmcloud;
use crate::util::{CommandOutput, OutputKind};
use wash_lib::start::*;
mod config;
mod credsfile;
pub use config::DOWNLOADS_DIR;
use config::*;

#[derive(Parser, Debug, Clone)]
pub(crate) struct UpCommand {
    /// Launch NATS and wasmCloud detached from the current terminal as background processes
    #[clap(short = 'd', long = "detached", alias = "detach")]
    pub(crate) detached: bool,

    #[clap(flatten)]
    pub(crate) nats_opts: NatsOpts,

    #[clap(flatten)]
    pub(crate) wasmcloud_opts: WasmcloudOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct NatsOpts {
    /// Optional path to a NATS credentials file to authenticate and extend existing NATS infrastructure.
    #[clap(
        long = "nats-credsfile",
        env = "NATS_CREDSFILE",
        requires = "nats_remote_url"
    )]
    pub(crate) nats_credsfile: Option<PathBuf>,

    /// Optional remote URL of existing NATS infrastructure to extend.
    #[clap(
        long = "nats-remote-url",
        env = "NATS_REMOTE_URL",
        requires = "nats_credsfile"
    )]
    pub(crate) nats_remote_url: Option<String>,

    /// If a connection can't be established, exit and don't start a NATS server. Will be ignored if a remote_url and credsfile are specified
    #[clap(
        long = "nats-connect-only",
        env = "NATS_CONNECT_ONLY",
        conflicts_with = "nats_remote_url"
    )]
    pub(crate) connect_only: bool,

    /// NATS server version to download, e.g. `v2.7.2`. See https://github.com/nats-io/nats-server/releases/ for releases
    #[clap(long = "nats-version", default_value = NATS_SERVER_VERSION, env = "NATS_VERSION")]
    pub(crate) nats_version: String,

    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = DEFAULT_NATS_HOST, env = "NATS_HOST")]
    pub(crate) nats_host: String,

    /// NATS server port to connect to. This will be used as the NATS listen port if `--nats-connect-only` isn't set
    #[clap(long = "nats-port", default_value = DEFAULT_NATS_PORT, env = "NATS_PORT")]
    pub(crate) nats_port: u16,

    /// NATS Server Jetstream domain, defaults to `core`
    #[clap(long = "nats-js-domain", env = "NATS_JS_DOMAIN")]
    pub(crate) nats_js_domain: Option<String>,
}

impl From<NatsOpts> for NatsConfig {
    fn from(other: NatsOpts) -> NatsConfig {
        NatsConfig {
            host: other.nats_host,
            port: other.nats_port,
            js_domain: other.nats_js_domain,
            remote_url: other.nats_remote_url,
            credentials: other.nats_credsfile,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct WasmcloudOpts {
    /// wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud-otp/releases for releases
    #[clap(long = "wasmcloud-version", default_value = WASMCLOUD_HOST_VERSION, env = "WASMCLOUD_VERSION")]
    pub(crate) wasmcloud_version: String,

    /// A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices
    #[clap(
        short = 'x',
        long = "lattice-prefix",
        default_value = DEFAULT_LATTICE_PREFIX,
        env = WASMCLOUD_LATTICE_PREFIX,
    )]
    pub(crate) lattice_prefix: String,

    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate it's public key  
    #[clap(long = "host-seed", env = WASMCLOUD_HOST_SEED)]
    pub(crate) host_seed: Option<String>,

    /// An IP address or DNS name to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "rpc-host", env = WASMCLOUD_RPC_HOST)]
    pub(crate) rpc_host: Option<String>,

    /// A port to use to connect to NATS for RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "rpc-port", env = WASMCLOUD_RPC_PORT)]
    pub(crate) rpc_port: Option<u16>,

    /// A seed nkey to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-seed", env = WASMCLOUD_RPC_SEED, requires = "rpc_jwt")]
    pub(crate) rpc_seed: Option<String>,

    /// Timeout in milliseconds for all RPC calls
    #[clap(long = "rpc-timeout-ms", default_value = DEFAULT_RPC_TIMEOUT_MS, env = WASMCLOUD_RPC_TIMEOUT_MS)]
    pub(crate) rpc_timeout_ms: u32,

    /// A user JWT to use to authenticate to NATS for RPC messages
    #[clap(long = "rpc-jwt", env = WASMCLOUD_RPC_JWT, requires = "rpc_seed")]
    pub(crate) rpc_jwt: Option<String>,

    /// Optional flag to enable host communication with a NATS server over TLS for RPC messages
    #[clap(long = "rpc-tls", env = WASMCLOUD_RPC_TLS)]
    pub(crate) rpc_tls: bool,

    /// Convenience flag for RPC authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "rpc-credsfile", env = WASMCLOUD_RPC_CREDSFILE)]
    pub(crate) rpc_credsfile: Option<PathBuf>,

    /// An IP address or DNS name to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "prov-rpc-host", env = WASMCLOUD_PROV_RPC_HOST)]
    pub(crate) prov_rpc_host: Option<String>,

    /// A port to use to connect to NATS for Provider RPC messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "prov-rpc-port", env = WASMCLOUD_PROV_RPC_PORT)]
    pub(crate) prov_rpc_port: Option<u16>,

    /// A seed nkey to use to authenticate to NATS for Provider RPC messages
    #[clap(long = "prov-rpc-seed", env = WASMCLOUD_PROV_RPC_SEED, requires = "prov_rpc_jwt")]
    pub(crate) prov_rpc_seed: Option<String>,

    /// Optional flag to enable host communication with a NATS server over TLS for Provider RPC messages
    #[clap(long = "prov-rpc-tls", env = WASMCLOUD_PROV_RPC_TLS)]
    pub(crate) prov_rpc_tls: bool,

    /// A user JWT to use to authenticate to NATS for Provider RPC messages
    #[clap(long = "prov-rpc-jwt", env = WASMCLOUD_PROV_RPC_JWT, requires = "prov_rpc_seed")]
    pub(crate) prov_rpc_jwt: Option<String>,

    /// Convenience flag for Provider RPC authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "prov-rpc-credsfile", env = WASMCLOUD_PROV_RPC_CREDSFILE)]
    pub(crate) prov_rpc_credsfile: Option<PathBuf>,

    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = WASMCLOUD_CTL_HOST)]
    pub(crate) ctl_host: Option<String>,

    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = WASMCLOUD_CTL_PORT)]
    pub(crate) ctl_port: Option<u16>,

    /// A seed nkey to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED, requires = "ctl_jwt")]
    pub(crate) ctl_seed: Option<String>,

    /// A user JWT to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT, requires = "ctl_seed")]
    pub(crate) ctl_jwt: Option<String>,

    /// Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "ctl-credsfile", env = WASMCLOUD_CTL_CREDSFILE)]
    pub(crate) ctl_credsfile: Option<PathBuf>,

    /// Optional flag to enable host communication with a NATS server over TLS for CTL messages
    #[clap(long = "ctl-tls", env = WASMCLOUD_CTL_TLS)]
    pub(crate) ctl_tls: bool,

    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    #[clap(long = "cluster-seed", env = WASMCLOUD_CLUSTER_SEED)]
    pub(crate) cluster_seed: Option<String>,

    /// A comma-delimited list of public keys that can be used as issuers on signed invocations
    #[clap(long = "cluster-issuers", env = WASMCLOUD_CLUSTER_ISSUERS)]
    pub(crate) cluster_issuers: Option<Vec<String>>,

    /// Delay, in milliseconds, between requesting a provider shut down and forcibly terminating its process
    #[clap(long = "provider-delay", default_value = DEFAULT_PROV_SHUTDOWN_DELAY_MS, env = WASMCLOUD_PROV_SHUTDOWN_DELAY_MS)]
    pub(crate) provider_delay: u32,

    /// Determines whether OCI images tagged latest are allowed to be pulled from OCI registries and started
    #[clap(long = "allow-latest", env = WASMCLOUD_OCI_ALLOW_LATEST)]
    pub(crate) allow_latest: bool,

    /// A comma-separated list of OCI hosts to which insecure (non-TLS) connections are allowed
    #[clap(long = "allowed-insecure", env = WASMCLOUD_OCI_ALLOWED_INSECURE)]
    pub(crate) allowed_insecure: Option<Vec<String>>,

    /// Jetstream domain name, configures a host to properly connect to a NATS supercluster, defaults to `core`
    #[clap(long = "wasmcloud-js-domain", env = WASMCLOUD_JS_DOMAIN)]
    pub(crate) wasmcloud_js_domain: Option<String>,

    /// Denotes if a wasmCloud host should issue requests to a config service on startup
    #[clap(long = "config-service-enabled", env = WASMCLOUD_CONFIG_SERVICE)]
    pub(crate) config_service_enabled: bool,

    /// Enable JSON structured logging from the wasmCloud host
    #[clap(
        long = "enable-structured-logging",
        env = WASMCLOUD_STRUCTURED_LOGGING_ENABLED
    )]
    pub(crate) enable_structured_logging: bool,

    /// Controls the verbosity of JSON structured logs from the wasmCloud host
    #[clap(long = "structured-log-level", default_value = DEFAULT_STRUCTURED_LOG_LEVEL, env = WASMCLOUD_STRUCTURED_LOG_LEVEL)]
    pub(crate) structured_log_level: String,

    /// Enables IPV6 addressing for wasmCloud hosts
    #[clap(long = "enable-ipv6", env = WASMCLOUD_ENABLE_IPV6)]
    pub(crate) enable_ipv6: bool,

    /// If enabled, wasmCloud will not be downloaded if it's not installed
    #[clap(long = "wasmcloud-start-only")]
    pub(crate) start_only: bool,
}

pub(crate) async fn handle_command(
    command: UpCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_up(command, output_kind).await
}

pub(crate) async fn handle_up(cmd: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    create_dir_all(&install_dir).await?;
    let spinner = Spinner::new(&output_kind)?;
    // Capture listen address to keep the value after the nats_opts are moved
    let nats_listen_address = format!("{}:{}", cmd.nats_opts.nats_host, cmd.nats_opts.nats_port);

    // Avoid downloading + starting NATS if the user already runs their own server. Ignore connect_only
    // if this server has a remote and credsfile as we have to start a leafnode in that scenario
    let nats_opts = cmd.nats_opts.clone();
    let nats_bin = if !cmd.nats_opts.connect_only
        || cmd.nats_opts.nats_remote_url.is_some() && cmd.nats_opts.nats_credsfile.is_some()
    {
        // Download NATS if not already installed
        spinner.update_spinner_message(" Downloading NATS ...".to_string());
        let nats_binary = ensure_nats_server(&cmd.nats_opts.nats_version, &install_dir).await?;

        spinner.update_spinner_message(" Starting NATS ...".to_string());
        start_nats(&install_dir, &nats_binary, cmd.nats_opts.clone()).await?;
        Some(nats_binary)
    } else {
        // If we can connect to NATS, return None as we aren't managing the child process.
        // Otherwise, exit with error since --nats-connect-only was specified
        tokio::net::TcpStream::connect(&nats_listen_address)
            .await
            .map(|_| None)
            .map_err(|_| {
                anyhow!(
                    "Could not connect to NATS at {}, exiting since --nats-connect-only was set",
                    nats_listen_address
                )
            })?
    };

    // Download wasmCloud if not already installed
    let wasmcloud_executable = if !cmd.wasmcloud_opts.start_only {
        spinner.update_spinner_message(" Downloading wasmCloud ...".to_string());
        ensure_wasmcloud(&cmd.wasmcloud_opts.wasmcloud_version, &install_dir).await?
    } else {
        // Ensure we clean up the NATS server if we can't start wasmCloud
        if nats_bin.is_some() {
            stop_nats(install_dir).await?;
        }
        return Err(anyhow!("wasmCloud was not installed, exiting without downloading as --wasmcloud-start-only was set"));
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

    let host_env = configure_host_env(nats_opts, cmd.wasmcloud_opts).await;
    let wasmcloud_child = match start_wasmcloud_host(
        &wasmcloud_executable,
        std::process::Stdio::null(),
        stderr,
        host_env,
    )
    .await
    {
        Ok(child) => child,
        Err(e) => {
            // Ensure we clean up the NATS server if we can't start wasmCloud
            stop_nats(install_dir).await?;
            return Err(e);
        }
    };

    spinner.finish_and_clear();
    if !cmd.detached {
        run_wasmcloud_interactive(wasmcloud_child, output_kind).await?;

        let spinner = Spinner::new(&output_kind)?;
        spinner.update_spinner_message(
            "CTRL+c received, gracefully stopping wasmCloud and NATS...".to_string(),
        );

        // Terminate wasmCloud and NATS processes
        let output = stop_wasmcloud(wasmcloud_executable.clone()).await?;
        if !output.status.success() {
            log::warn!("wasmCloud exited with a non-zero exit status, processes may need to be cleaned up manually")
        }

        stop_nats(install_dir).await?;

        spinner.finish_and_clear();
    }

    // Build the CommandOutput providing some useful information like pids, ports, and logfiles
    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash up completed successfully");

    if cmd.detached {
        let url = "http://localhost:4000";
        out_json.insert("wasmcloud_url".to_string(), json!(url));
        out_json.insert("wasmcloud_log".to_string(), json!(wasmcloud_log_path));
        out_json.insert("kill_cmd".to_string(), json!("wash down"));
        out_json.insert("nats_url".to_string(), json!(nats_listen_address));

        let _ = write!(
            out_text,
            "\n🕸  NATS is running in the background at http://{}",
            nats_listen_address
        );
        let _ = write!(
            out_text,
            "\n🌐 The wasmCloud dashboard is running at {}\n📜 Logs for the host are being written to {}",
            url, wasmcloud_log_path.to_string_lossy()
        );
        let _ = write!(out_text, "\n\n🛑 To stop wasmCloud, run \"wash down\"");
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
                return Err(anyhow!("Could not connect to leafnode remote: {}", e));
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
    start_nats_server(nats_binary, nats_log_file, nats_opts.into()).await
}

/// Helper function to run wasmCloud in interactive mode
async fn run_wasmcloud_interactive(
    mut wasmcloud_child: Child,
    output_kind: OutputKind,
) -> Result<()> {
    use std::sync::mpsc::channel;
    let (running_sender, running_receiver) = channel();
    let running = Arc::new(AtomicBool::new(true));

    ctrlc::set_handler(move || {
        if running.load(Ordering::SeqCst) {
            running.store(false, Ordering::SeqCst);
            let _ = running_sender.send(true);
        } else {
            log::warn!("\nRepeated CTRL+C received, killing wasmCloud and NATS. This may result in zombie processes")
        }
    })
    .expect("Error setting Ctrl-C handler, please file a bug issue https://github.com/wasmCloud/wash/issues/new/choose");

    if output_kind != OutputKind::Json {
        println!("🏃 Running in interactive mode, your host is running at http://localhost:4000",);
        println!("🚪 Press `CTRL+c` at any time to exit");
    }

    // Create a separate thread to log host output
    let handle = wasmcloud_child.stderr.take().map(|stderr| {
        tokio::spawn(async {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                //TODO(brooksmtownsend): in the future, would be great to print these in a prettier format
                println!("{}", line)
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

#[cfg(test)]
mod tests {
    use super::UpCommand;
    use anyhow::Result;
    use clap::Parser;

    // Assert that our API doesn't unknowingly drift
    #[test]
    fn test_up_comprehensive() -> Result<()> {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        let up_all_flags: UpCommand = Parser::try_parse_from(&[
            "up",
            "--allow-latest",
            "--allowed-insecure",
            "localhost:5000",
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
            "--prov-rpc-credsfile",
            TESTDIR,
            "--prov-rpc-host",
            "127.0.0.2",
            "--prov-rpc-jwt",
            "eyyjWT",
            "--prov-rpc-port",
            "4232",
            "--prov-rpc-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--prov-rpc-tls",
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
            Some(vec!["localhost:5000".to_string()])
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
        assert!(up_all_flags.wasmcloud_opts.prov_rpc_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.prov_rpc_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.prov_rpc_tls);
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
