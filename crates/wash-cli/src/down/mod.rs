use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use anyhow::{anyhow, bail, Result};
use async_nats::Client;
use clap::Parser;
use log::{error, warn};
use serde_json::json;
use tokio::process::Command;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::{DEFAULT_NATS_HOST, DEFAULT_NATS_PORT};
use wash_lib::id::ServerId;
use wash_lib::start::{nats_pid_path, NATS_SERVER_BINARY, WADM_PID};

use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::up::{
    DEFAULT_LATTICE_PREFIX, DOWNLOADS_DIR, WASMCLOUD_CTL_CREDSFILE, WASMCLOUD_CTL_HOST,
    WASMCLOUD_CTL_JWT, WASMCLOUD_CTL_PORT, WASMCLOUD_CTL_SEED, WASMCLOUD_LATTICE_PREFIX,
};
use crate::util::nats_client_from_opts;

#[derive(Parser, Debug, Clone, Default)]
pub struct DownCommand {
    /// A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices
    #[clap(
            short = 'x',
            long = "lattice-prefix",
            default_value = DEFAULT_LATTICE_PREFIX,
            env = WASMCLOUD_LATTICE_PREFIX,
        )]
    pub lattice_prefix: String,

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

    #[clap(long = "host-id")]
    pub host_id: Option<ServerId>,

    #[clap(long = "all")]
    pub all: bool,
}

pub async fn handle_command(
    command: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_down(command, output_kind).await
}

pub async fn handle_down(cmd: DownCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let sp = Spinner::new(&output_kind)?;
    sp.update_spinner_message(" Stopping wasmCloud ...".to_string());

    let mut out_json = HashMap::new();
    let mut out_text = String::from("");

    if let Ok(client) = nats_client_from_opts(
        &cmd.ctl_host
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string()),
        &cmd.ctl_port
            .map(|port| port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string()),
        cmd.ctl_jwt,
        cmd.ctl_seed,
        cmd.ctl_credsfile,
    )
    .await
    {
        let (hosts, hosts_remain) =
            stop_hosts(client, &cmd.lattice_prefix, &cmd.host_id, cmd.all).await?;
        out_json.insert("hosts_stopped".to_string(), json!(hosts));
        out_text.push_str("✅ wasmCloud hosts stopped successfully\n");
        if hosts_remain {
            out_json.insert("nats_stopped".to_string(), json!(false));
            out_json.insert("wadm_stopped".to_string(), json!(false));
            out_text.push_str(
                "🛁 Exiting without stopping NATS or wadm, there are still hosts running",
            );
            return Ok(CommandOutput::new(out_text, out_json));
        }
    } else {
        warn!("Couldn't connect to NATS, unable to stop running hosts")
    }

    match stop_wadm(&install_dir).await {
        Ok(_) => {
            tokio::fs::remove_file(&install_dir.join(WADM_PID)).await?;
            out_json.insert("wadm_stopped".to_string(), json!(true));
            out_text.push_str("✅ wadm stopped successfully\n");
        }
        Err(e) => {
            out_json.insert("wadm_stopped".to_string(), json!(false));
            out_text.push_str(&format!("❌ Could not stop wadm: {e:?}\n"));
        }
    }

    let nats_bin = install_dir.join(NATS_SERVER_BINARY);
    if nats_bin.is_file() {
        sp.update_spinner_message(" Stopping NATS server ...".to_string());
        if let Err(e) = stop_nats(&install_dir).await {
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

/// Stop running wasmCloud hosts, returns a vector of host IDs that were stopped and
/// a boolean indicating whether any hosts remain running
async fn stop_hosts(
    nats_client: Client,
    lattice_prefix: &str,
    host_id: &Option<ServerId>,
    all: bool,
) -> Result<(Vec<String>, bool)> {
    let client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice_prefix(lattice_prefix)
        .auction_timeout(std::time::Duration::from_secs(2))
        .build()
        .await
        .map_err(|e| anyhow!(e))?;

    let hosts = client.get_hosts().await.map_err(|e| anyhow!(e))?;

    // If a host ID was supplied, stop only that host
    if let Some(host_id) = host_id {
        let host_id_string = host_id.to_string();
        client.stop_host(&host_id_string, None).await.map_err(|e| {
            anyhow!(
                "Could not stop host, ensure a host with that ID is running: {:?}",
                e
            )
        })?;

        Ok((vec![host_id_string], hosts.len() > 1))
    } else if hosts.is_empty() {
        Ok((vec![], false))
    } else if hosts.len() == 1 {
        let host_id = &hosts[0].id;
        client
            .stop_host(host_id, None)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok((vec![host_id.to_string()], false))
    } else if all {
        let host_stops = hosts
            .iter()
            .map(|host| async {
                let host_id = &host.id;
                match client.stop_host(host_id, None).await {
                    Ok(_) => Some(host_id.to_owned()),
                    Err(e) => {
                        error!("Could not stop host {}: {:?}", host_id, e);
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        let all_stops = futures::future::join_all(host_stops).await;
        let host_ids = all_stops
            .iter()
            // Remove any host IDs that ran into errors
            .filter_map(|host_id| host_id.to_owned())
            .collect::<Vec<_>>();
        let hosts_remaining = all_stops.len() > host_ids.len();

        Ok((host_ids, hosts_remaining))
    } else {
        bail!(
                "More than one host is running, please specify a host ID or use --all\nRunning hosts: {:?}", hosts.into_iter().map(|h| h.id).collect::<Vec<_>>()
            )
    }
}

/// Helper function to send the nats-server the stop command
pub async fn stop_nats<P>(install_dir: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    let bin_path = install_dir.as_ref().join(NATS_SERVER_BINARY);
    let pid_file = nats_pid_path(install_dir);
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
    if let Ok(pid) = tokio::fs::read_to_string(&install_dir.as_ref().join(WADM_PID)).await {
        tokio::process::Command::new("kill")
            .arg(pid)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!(e))
    } else {
        Err(anyhow::anyhow!("No pidfile found"))
    }
}
