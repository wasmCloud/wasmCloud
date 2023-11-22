use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::pin::pin;
use std::process::ExitStatus;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use nkeys::KeyPair;
use serde::Deserialize;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tokio::{select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tracing::warn;
use wascap::jwt;
use wasmcloud_control_interface::CtlOperationAck;

pub mod nats;
pub mod redis;
pub mod vault;

pub fn tempdir() -> Result<TempDir> {
    tempfile::tempdir().context("failed to create temporary directory")
}

pub async fn free_port() -> Result<u16> {
    TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

pub async fn assert_start_actor(
    ctl_client: &wasmcloud_control_interface::Client,
    nats_client: &async_nats::Client, // TODO: This should be exposed by `wasmcloud_control_interface::Client`
    lattice_prefix: &str,
    host_key: &KeyPair,
    url: impl AsRef<str>,
    count: u16,
) -> anyhow::Result<()> {
    let mut sub_started = nats_client
        .subscribe(format!("wasmbus.evt.{lattice_prefix}.actors_started"))
        .await?;

    // TODO(#740): Remove deprecated once control clients no longer use this command
    #[allow(deprecated)]
    let CtlOperationAck { accepted, error } = ctl_client
        .start_actor(&host_key.public_key(), url.as_ref(), count, None)
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(error == "");
    ensure!(accepted);

    // Naive wait for at least a stopped / started event before exiting this function. This prevents
    // assuming we're done with scaling too early since scale is an early-ack ctl request.
    tokio::select! {
        _ = sub_started.next() => {
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for actor started event");
        },
    }

    Ok(())
}

pub async fn assert_scale_actor(
    ctl_client: &wasmcloud_control_interface::Client,
    nats_client: &async_nats::Client, // TODO: This should be exposed by `wasmcloud_control_interface::Client`
    lattice_prefix: &str,
    host_key: &KeyPair,
    url: impl AsRef<str>,
    annotations: Option<HashMap<String, String>>,
    count: Option<u16>,
) -> anyhow::Result<()> {
    let mut sub_started = nats_client
        .subscribe(format!("wasmbus.evt.{lattice_prefix}.actors_started"))
        .await?;
    let mut sub_stopped = nats_client
        .subscribe(format!("wasmbus.evt.{lattice_prefix}.actors_stopped"))
        .await?;
    let CtlOperationAck { accepted, error } = ctl_client
        .scale_actor(&host_key.public_key(), url.as_ref(), count, annotations)
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(error == "");
    ensure!(accepted);

    // Naive wait for at least a stopped / started event before exiting this function. This prevents
    // assuming we're done with scaling too early since scale is an early-ack ctl request.
    tokio::select! {
        _ = sub_started.next() => {
        }
        _ = sub_stopped.next() => {
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            bail!("timed out waiting for actor scale event");
        },
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)] // Shush clippy, it's a test function
pub async fn assert_start_provider(
    client: &wasmcloud_control_interface::Client,
    rpc_client: &async_nats::Client,
    lattice_prefix: &str,
    host_key: &KeyPair,
    provider_key: &KeyPair,
    link_name: &str,
    url: impl AsRef<str>,
    configuration: Option<String>,
) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct HealthCheckResponse {
        #[serde(default)]
        healthy: bool,
        #[serde(default)]
        message: Option<String>,
    }

    let CtlOperationAck { accepted, error } = client
        .start_provider(
            &host_key.public_key(),
            url.as_ref(),
            Some(link_name.to_string()),
            None,
            configuration,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(error == "");
    ensure!(accepted);

    let res = pin!(IntervalStream::new(interval(Duration::from_secs(1)))
        .take(30)
        .then(|_| rpc_client.request(
            format!(
                "wasmbus.rpc.{}.{}.{}.health",
                lattice_prefix,
                provider_key.public_key(),
                link_name,
            ),
            "".into(),
        ))
        .filter_map(|res| {
            match res {
                Err(error) => {
                    warn!(?error, "failed to connect to provider");
                    None
                }
                Ok(res) => Some(res),
            }
        }))
    .next()
    .await
    .context("failed to perform health check request")?;

    let HealthCheckResponse { healthy, message } =
        rmp_serde::from_slice(&res.payload).context("failed to decode health check response")?;
    ensure!(message == None);
    ensure!(healthy);
    Ok(())
}

pub async fn assert_advertise_link(
    client: &wasmcloud_control_interface::Client,
    actor_claims: &jwt::Claims<jwt::Actor>,
    provider_key: &KeyPair,
    contract_id: impl AsRef<str>,
    link_name: &str,
    values: HashMap<String, String>,
) -> Result<()> {
    client
        .advertise_link(
            &actor_claims.subject,
            &provider_key.public_key(),
            contract_id.as_ref(),
            link_name,
            values,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    Ok(())
}

pub async fn assert_remove_link(
    client: &wasmcloud_control_interface::Client,
    actor_claims: &jwt::Claims<jwt::Actor>,
    contract_id: impl AsRef<str>,
    link_name: &str,
) -> Result<()> {
    client
        .remove_link(&actor_claims.subject, contract_id.as_ref(), link_name)
        .await
        .map_err(|e| anyhow!(e).context("failed to remove link"))?;
    Ok(())
}

pub async fn assert_config_put(
    client: &wasmcloud_control_interface::Client,
    actor_claims: &jwt::Claims<jwt::Actor>,
    key: &str,
    value: impl Into<Vec<u8>>,
) -> Result<()> {
    client
        .put_config(&actor_claims.subject, key, value)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put config"))
}

pub async fn assert_put_label(
    client: &wasmcloud_control_interface::Client,
    host_id: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    client
        .put_label(host_id, key, value)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put label"))
}

pub async fn assert_delete_label(
    client: &wasmcloud_control_interface::Client,
    host_id: &str,
    key: &str,
) -> Result<()> {
    client
        .delete_label(host_id, key)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put label"))
}

pub async fn spawn_server(
    cmd: &mut Command,
) -> Result<(JoinHandle<Result<ExitStatus>>, oneshot::Sender<()>)> {
    let mut child = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn child")?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let child = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                child.kill().await.context("failed to kill child")?;
                child.wait().await
            }
            status = child.wait() => {
                status
            }
        )
        .context("failed to wait for child")
    });
    Ok((child, stop_tx))
}

pub async fn stop_server(
    server: JoinHandle<Result<ExitStatus>>,
    stop_tx: oneshot::Sender<()>,
) -> Result<()> {
    stop_tx.send(()).expect("failed to stop");
    let status = server
        .await
        .context("failed to wait for server to exit")??;
    ensure!(status.code().is_none());
    Ok(())
}
