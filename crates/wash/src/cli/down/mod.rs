use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use anyhow::{bail, Context as _, Result};
use clap::Parser;
use futures::future::join_all;
use serde_json::json;
use tokio::process::Command;
use tracing::warn;
use crate::lib::cli::{stop::stop_hosts, CommandOutput, OutputKind};
use crate::lib::config::{
    create_nats_client_from_opts, downloads_dir, host_pid_file, DEFAULT_NATS_HOST,
    DEFAULT_NATS_PORT,
};
use crate::lib::id::ServerId;
use crate::lib::start::{nats_pid_path, NATS_SERVER_BINARY, WADM_PID};

use crate::appearance::spinner::Spinner;
use crate::config::{
    DEFAULT_LATTICE, WASMCLOUD_CTL_CREDSFILE, WASMCLOUD_CTL_HOST, WASMCLOUD_CTL_JWT,
    WASMCLOUD_CTL_PORT, WASMCLOUD_CTL_SEED, WASMCLOUD_CTL_TLS_CA_FILE, WASMCLOUD_CTL_TLS_FIRST,
    WASMCLOUD_LATTICE,
};

#[derive(Parser, Debug, Clone, Default, clap::ValueEnum, Eq, PartialEq)]
pub enum PurgeJetstream {
    /// Don't purge any Jetstream data, the default
    #[default]
    None,
    /// Purge all streams and KV buckets for wasmCloud and wadm
    All,
    /// Purge all streams and KV buckets for wadm, removing all application manifests
    Wadm,
    /// Purge all KV buckets for wasmCloud, removing all links and configuration data
    Wasmcloud,
}

#[derive(Parser, Debug, Clone, Default)]
pub struct DownCommand {
    /// A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices
    #[clap(
            short = 'x',
            long = "lattice",
            default_value = DEFAULT_LATTICE,
            env = WASMCLOUD_LATTICE,
        )]
    pub lattice: String,

    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = WASMCLOUD_CTL_HOST)]
    pub ctl_host: Option<String>,

    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = WASMCLOUD_CTL_PORT)]
    pub ctl_port: Option<u16>,

    /// Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "ctl-credsfile", env = WASMCLOUD_CTL_CREDSFILE)]
    pub ctl_credsfile: Option<PathBuf>,

    /// A seed nkey to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED, requires = "ctl_jwt")]
    pub ctl_seed: Option<String>,

    /// A user JWT to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT, requires = "ctl_seed")]
    pub ctl_jwt: Option<String>,

    /// A TLS CA file to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-tls-ca-file", env = WASMCLOUD_CTL_TLS_CA_FILE)]
    pub ctl_tls_ca_file: Option<PathBuf>,

    /// Perform TLS handshake before expecting the server greeting
    #[clap(long = "ctl-tls-first", env = WASMCLOUD_CTL_TLS_FIRST)]
    pub ctl_tls_first: Option<bool>,

    #[clap(long = "host-id")]
    pub host_id: Option<ServerId>,

    /// Shutdown all hosts running locally if launched with --multi-local
    #[clap(long = "all")]
    pub all: bool,

    /// Purge NATS Jetstream storage and streams that persist when wasmCloud is stopped
    #[clap(
        long = "purge-jetstream",
        alias = "flush-jetstream",
        default_value = "none"
    )]
    pub purge: PurgeJetstream,
}

pub async fn handle_command(
    command: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_down(command, output_kind).await
}

pub async fn handle_down(cmd: DownCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = downloads_dir()?;
    let sp = Spinner::new(&output_kind)?;
    sp.update_spinner_message(" Stopping wasmCloud ...".to_string());

    let mut out_json = HashMap::new();
    let mut out_text = String::new();

    let nats_client = create_nats_client_from_opts(
        &cmd.ctl_host
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string()),
        &cmd.ctl_port.map_or_else(|| DEFAULT_NATS_PORT.to_string(), |port| port.to_string()),
        cmd.ctl_jwt,
        cmd.ctl_seed,
        cmd.ctl_credsfile,
        cmd.ctl_tls_ca_file,
        cmd.ctl_tls_first.unwrap_or_default(),
    )
    .await;

    if let Ok(client) = nats_client.as_ref() {
        let ctl_client = wasmcloud_control_interface::ClientBuilder::new(client.clone())
            .lattice(&cmd.lattice)
            .auction_timeout(std::time::Duration::from_secs(2))
            .build();
        let host_id_string = cmd.host_id.map(|id| id.to_string());
        let (hosts, hosts_remain) =
            stop_hosts(ctl_client, host_id_string.as_ref(), cmd.all).await?;
        out_json.insert("hosts_stopped".to_string(), json!(hosts));
        out_text.push_str("✅ wasmCloud hosts stopped successfully\n");
        if hosts_remain {
            out_json.insert("nats_stopped".to_string(), json!(false));
            out_json.insert("wadm_stopped".to_string(), json!(false));
            out_text.push_str(
                "🛁 Exiting without stopping NATS or wadm, there are still hosts running",
            );
            return Ok(CommandOutput::new(out_text, out_json));
        } else {
            let wasmcloud_pid_file_path = host_pid_file()?;
            // Failing to find the pid file is not an error that should prevent stopping other resources
            if let Err(e) = tokio::fs::remove_file(&wasmcloud_pid_file_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to find wasmcloud pid file [{}]",
                        wasmcloud_pid_file_path.display()
                    )
                })
            {
                warn!("{}", e.to_string());
            }
        }
    } else {
        warn!("Couldn't connect to NATS, unable to stop running hosts");
    }

    match stop_wadm(&install_dir).await {
        Ok(_) => {
            let pid_file_path = &install_dir.join(WADM_PID);
            // Failing to find the pid file is not an error that should prevent stopping other resources
            if let Err(e) = tokio::fs::remove_file(&pid_file_path)
                .await
                .with_context(|| {
                    format!("failed to find WADM pid file [{}]", pid_file_path.display())
                })
            {
                warn!("{}", e.to_string());
            }

            out_json.insert("wadm_stopped".to_string(), json!(true));
            out_text.push_str("✅ wadm stopped successfully\n");
        }
        Err(e) => {
            out_json.insert("wadm_stopped".to_string(), json!(false));
            out_text.push_str(&format!("❌ Could not stop wadm: {e:?}\n"));
        }
    }

    if nats_client
        .as_ref()
        .is_ok_and(|_| cmd.purge != PurgeJetstream::None)
    {
        sp.update_spinner_message(" Purging NATS Jetstream ...".to_string());
        // SAFETY: nats_client is checked to be Ok() above
        let client = nats_client.unwrap();
        let js_client = async_nats::jetstream::new(client);

        if cmd.purge == PurgeJetstream::All || cmd.purge == PurgeJetstream::Wasmcloud {
            join_all(vec![
                delete_kv_idempotent(&js_client, format!("CONFIGDATA_{}", &cmd.lattice)),
                delete_kv_idempotent(&js_client, format!("LATTICEDATA_{}", &cmd.lattice)),
            ])
            .await
            .iter()
            .for_each(|result| {
                if let Err(e) = result {
                    out_text.push_str(&format!("❌ Error removing stream: {e:?}\n"));
                }
            });
        }

        if cmd.purge == PurgeJetstream::All || cmd.purge == PurgeJetstream::Wadm {
            let kvs = join_all(vec![
                delete_kv_idempotent(&js_client, "wadm_manifests"),
                delete_kv_idempotent(&js_client, "wadm_state"),
            ])
            .await;

            let streams = join_all(vec![
                delete_stream_idempotent(&js_client, "wadm_commands"),
                delete_stream_idempotent(&js_client, "wadm_events"),
                delete_stream_idempotent(&js_client, "wadm_mirror"),
                delete_stream_idempotent(&js_client, "wadm_notify"),
                delete_stream_idempotent(&js_client, "wadm_status"),
            ])
            .await;

            kvs.iter().chain(streams.iter()).for_each(|result| {
                if let Err(e) = result {
                    out_text.push_str(&format!("❌ Error removing stream: {e:?}\n"));
                }
            });
        }

        out_text.push_str("✅ NATS Jetstream purged successfully\n");
    }

    let nats_bin = install_dir.join(NATS_SERVER_BINARY);
    if nats_bin.is_file() {
        sp.update_spinner_message(" Stopping NATS server ...".to_string());
        if let Err(e) = stop_nats(&install_dir, &nats_bin).await {
            out_json.insert("nats_stopped".to_string(), json!(false));
            out_text.push_str(&format!(
                "❌ NATS server did not stop successfully: {e:?}\n"
            ));
        } else {
            out_json.insert("nats_stopped".to_string(), json!(true));
            out_text.push_str("✅ NATS server stopped successfully\n");
        }
    }

    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash down completed successfully");

    sp.finish_and_clear();
    Ok(CommandOutput::new(out_text, out_json))
}

/// Helper function to send the nats-server the stop command
pub async fn stop_nats<P>(work_dir: P, bin_path: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    let bin_path = bin_path.as_ref();
    let pid_file = nats_pid_path(work_dir.as_ref());
    let signal = if pid_file.is_file() {
        format!("stop={}", &pid_file.display())
    } else {
        return Err(anyhow::anyhow!(
            "No pidfile found for nats-server, assuming it's managed externally"
        ));
    };
    let output = Command::new(bin_path)
        .arg("--signal")
        .arg(signal)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(anyhow::Error::from);

    // remove PID file
    if pid_file.is_file() {
        let _ = tokio::fs::remove_file(&pid_file).await;
    }
    output
}

/// Helper function to kill the wadm process
pub async fn stop_wadm<P>(install_dir: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    let wadm_pidfile_path = install_dir.as_ref().join(WADM_PID);
    if let Ok(pid) = tokio::fs::read_to_string(&wadm_pidfile_path).await {
        tokio::process::Command::new("kill")
            .arg(pid)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!(e))
    } else {
        bail!("No pidfile found at [{}]", wadm_pidfile_path.display())
    }
}

/// Delete a Jetstream stream, ignoring errors if the stream doesn't exist
async fn delete_stream_idempotent(
    js: &async_nats::jetstream::Context,
    stream_name: impl AsRef<str>,
) -> Result<()> {
    match js.delete_stream(stream_name).await {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("stream not found") => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}

/// Delete a Jetstream key-value bucket, ignoring errors if the bucket doesn't exist
async fn delete_kv_idempotent(
    js: &async_nats::jetstream::Context,
    key: impl AsRef<str>,
) -> Result<()> {
    match js.delete_key_value(key).await {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("stream not found") => Ok(()),
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}
