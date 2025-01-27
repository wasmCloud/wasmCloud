use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_nats::Client;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use cloudevents::EventBuilderV10;
use futures::Future;
use nkeys::XKey;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinSet;
use tokio::{process, select};
use tracing::{error, instrument, trace, warn};
use wascap::jwt;
use wasmcloud_core::{provider_config_update_subject, HealthCheckResponse};
use wasmcloud_tracing::context::TraceContextInjector;

use crate::wasmbus::{config::ConfigBundle, Annotations};
use crate::wasmbus::{event, injector_to_headers};

mod http_server;
mod messaging_nats;

/// An Provider instance
#[derive(Debug)]
pub(crate) struct Provider {
    pub(crate) image_ref: String,
    pub(crate) claims_token: Option<jwt::Token<jwt::CapabilityProvider>>,
    pub(crate) xkey: XKey,
    pub(crate) annotations: Annotations,
    #[allow(unused)]
    /// Config bundle for the aggregated configuration being watched by the provider
    pub(crate) config: Arc<RwLock<ConfigBundle>>,
    /// Whether the provider should be restarted on exit
    pub(crate) restart_on_exit: Arc<AtomicBool>,
    pub(crate) tasks: JoinSet<()>,
}

#[allow(clippy::too_many_arguments)]
#[instrument(
    level = "info",
    skip(rpc_nats, ctl_nats, event_builder, host_data, config)
)]
pub(crate) async fn start_binary_provider(
    // Clients
    rpc_nats: Arc<Client>,
    ctl_nats: Client,
    event_builder: EventBuilderV10,
    // Necessary host information
    lattice: Arc<str>,
    host_id: &str,
    // Provider information
    provider_id: &str,
    path: PathBuf,
    host_data: Vec<u8>,
    config: Arc<RwLock<ConfigBundle>>,
) -> anyhow::Result<(JoinSet<()>, Arc<AtomicBool>)> {
    trace!("spawn provider process");

    let mut tasks = JoinSet::new();

    // Create a channel for watching for child process exit
    let (exit_tx, exit_rx) = broadcast::channel::<()>(1);

    let boo = Arc::new(AtomicBool::new(true));
    tasks.spawn(run_provider(path, host_data, boo.clone(), exit_tx).await?);

    // Spawn off a task to check the health of the provider every 30 seconds
    tasks.spawn(check_health(
        Arc::clone(&rpc_nats),
        ctl_nats,
        event_builder,
        Arc::clone(&lattice),
        host_id.to_string(),
        provider_id.to_string(),
        exit_rx.resubscribe(),
    ));

    // Spawn off a task to watch for config bundle updates and forward them to
    // the provider that we're spawning and managing
    tasks.spawn(watch_config(
        Arc::clone(&rpc_nats),
        Arc::clone(&config),
        Arc::clone(&lattice),
        provider_id.to_string(),
        exit_rx,
    ));

    Ok((tasks, boo))
}

/// Run and supervise a binary provider, restarting it if it exits prematurely.
async fn run_provider(
    path: PathBuf,
    host_data: Vec<u8>,
    restart_on_exit: Arc<AtomicBool>,
    exit_tx: broadcast::Sender<()>,
) -> anyhow::Result<impl Future<Output = ()>> {
    // If there's any issues starting the provider, we want to exit immediately
    let child = Arc::new(RwLock::new(
        provider_command(&path, &host_data)
            .await
            .context("failed to configure binary provider command")?,
    ));
    Ok(async move {
        loop {
            let mut child = child.write().await;
            match child.wait().await {
                Ok(status) => {
                    if restart_on_exit.load(Ordering::Relaxed) {
                        trace!(
                            "provider @ [{}] exited with `{status:?}` but will not be restarted",
                            path.display()
                        );
                        // Avoid a hot loop by waiting 100ms before checking the status again
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }

                    warn!(
                        "restarting provider @ [{}] that exited with `{status:?}`",
                        path.display()
                    );

                    // Restart the provider by attempting to re-execute the binary with the same
                    // host data
                    let Ok(child_cmd) = provider_command(&path, &host_data).await else {
                        exit_tx
                            .send(())
                            .expect("failed to send provider stop while restarting");
                        return;
                    };
                    *child = child_cmd;

                    // To avoid a tight loop, we wait 5 seconds after restarting. In the worst case,
                    // the provider will continually execute and exit every 5 seconds.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    error!(
                        "failed to wait for provider @ [{}] to execute: {e}",
                        path.display()
                    );

                    if let Err(err) = exit_tx.send(()) {
                        warn!(%err, "failed to send exit tx");
                    }
                    return;
                }
            }
        }
    })
}

async fn provider_command(path: &Path, host_data: &[u8]) -> anyhow::Result<process::Child> {
    let mut child_cmd = process::Command::new(path);
    // Prevent the provider from inheriting the host's environment, with the exception of
    // the following variables we manually add back
    child_cmd.env_clear();

    if cfg!(windows) {
        // Proxy SYSTEMROOT to providers. Without this, providers on Windows won't be able to start
        child_cmd.env(
            "SYSTEMROOT",
            env::var("SYSTEMROOT").context("SYSTEMROOT is not set. Providers cannot be started")?,
        );
    }

    // Proxy RUST_LOG to (Rust) providers, so they can use the same module-level directives
    if let Ok(rust_log) = env::var("RUST_LOG") {
        let _ = child_cmd.env("RUST_LOG", rust_log);
    }

    let mut child = child_cmd
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn provider process")?;
    let mut stdin = child.stdin.take().context("failed to take stdin")?;
    stdin
        .write_all(STANDARD.encode(host_data).as_bytes())
        .await
        .context("failed to write provider data")?;
    stdin
        .write_all(b"\r\n")
        .await
        .context("failed to write newline")?;
    stdin.shutdown().await.context("failed to close stdin")?;

    Ok(child)
}

/// Watch for health check responses from the provider
///
/// Returns a future that continually checks provider health every 30 seconds
/// until the health receiver gets a message
fn check_health(
    rpc_nats: Arc<Client>,
    ctl_nats: Client,
    event_builder: EventBuilderV10,
    lattice: Arc<str>,
    host_id: String,
    provider_id: String,
    mut exit_health_rx: broadcast::Receiver<()>,
) -> impl Future<Output = ()> {
    let health_subject =
        async_nats::Subject::from(format!("wasmbus.rpc.{lattice}.{provider_id}.health"));

    // Check the health of the provider every 30 seconds
    let mut health_check = tokio::time::interval(Duration::from_secs(30));
    let mut previous_healthy = false;
    // Allow the provider 5 seconds to initialize
    health_check.reset_after(Duration::from_secs(5));
    async move {
        loop {
            select! {
                _ = health_check.tick() => {
                    trace!(?provider_id, "performing provider health check");
                    let request = async_nats::Request::new()
                        .payload(Bytes::new())
                        .headers(injector_to_headers(&TraceContextInjector::default_with_span()));
                    if let Ok(async_nats::Message { payload, ..}) = rpc_nats.send_request(
                        health_subject.clone(),
                        request,
                        ).await {
                            match (serde_json::from_slice::<HealthCheckResponse>(&payload), previous_healthy) {
                                (Ok(HealthCheckResponse { healthy: true, ..}), false) => {
                                    trace!(?provider_id, "provider health check succeeded");
                                    previous_healthy = true;
                                    if let Err(e) = event::publish(
                                        &event_builder,
                                        &ctl_nats,
                                        &lattice,
                                        "health_check_passed",
                                        event::provider_health_check(
                                            &host_id,
                                            &provider_id,
                                        )
                                    ).await {
                                        warn!(
                                            ?e,
                                            ?provider_id,
                                            "failed to publish provider health check succeeded event",
                                        );
                                    }
                                },
                                (Ok(HealthCheckResponse { healthy: false, ..}), true) => {
                                    trace!(?provider_id, "provider health check failed");
                                    previous_healthy = false;
                                    if let Err(e) = event::publish(
                                        &event_builder,
                                        &ctl_nats,
                                        &lattice,
                                        "health_check_failed",
                                        event::provider_health_check(
                                            &host_id,
                                            &provider_id,
                                        )
                                    ).await {
                                        warn!(
                                            ?e,
                                            ?provider_id,
                                            "failed to publish provider health check failed event",
                                        );
                                    }
                                }
                                // If the provider health status didn't change, we simply publish a health check status event
                                (Ok(_), _) => {
                                    if let Err(e) = event::publish(
                                        &event_builder,
                                        &ctl_nats,
                                        &lattice,
                                        "health_check_status",
                                        event::provider_health_check(
                                            &host_id,
                                            &provider_id,
                                        )
                                    ).await {
                                        warn!(
                                            ?e,
                                            ?provider_id,
                                            "failed to publish provider health check status event",
                                        );
                                    }
                                },
                                _ => warn!(
                                    ?provider_id,
                                    "failed to deserialize provider health check response"
                                ),
                            }
                        }
                        else {
                            warn!(?provider_id, "failed to request provider health, retrying in 30 seconds");
                        }
                }
                exit = exit_health_rx.recv() => {
                    if let Err(err) = exit {
                        warn!(%err, ?provider_id, "failed to receive exit in health check task");
                    }
                    break;
                }
            }
        }
    }
}

/// Watch for config updates and send them to the provider
///
/// Returns a future that continually checks provider config changes
/// until the config receiver gets a message
fn watch_config(
    rpc_nats: Arc<Client>,
    config: Arc<RwLock<ConfigBundle>>,
    lattice: Arc<str>,
    provider_id: String,
    mut exit_config_rx: broadcast::Receiver<()>,
) -> impl Future<Output = ()> {
    let subject = provider_config_update_subject(&lattice, &provider_id);
    trace!(?provider_id, "starting config update listener");
    async move {
        loop {
            let mut config = config.write().await;
            select! {
                maybe_update = config.changed() => {
                    let Ok(update) = maybe_update else {
                        // TODO: shouldn't this be continue?
                        break;
                    };
                    trace!(?provider_id, "provider config bundle changed");
                    let bytes = match serde_json::to_vec(&*update) {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            error!(%err, ?provider_id, ?lattice, "failed to serialize configuration update ");
                            continue;
                        }
                    };
                    trace!(?provider_id, subject, "publishing config bundle bytes");
                    if let Err(err) = rpc_nats.publish(subject.clone(), Bytes::from(bytes)).await {
                        error!(%err, ?provider_id, ?lattice, "failed to publish configuration update bytes to component");
                    }
                }
                exit = exit_config_rx.recv() => {
                    if let Err(err) = exit {
                        warn!(%err, ?provider_id, "failed to receive exit in config update task");
                    }
                    // TODO: shouldn't this be return?
                    break;
                }
            }
        }
    }
}
