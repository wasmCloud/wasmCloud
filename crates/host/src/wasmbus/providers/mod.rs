//! Provider module
//!
//! The root of this module includes functionality for running and managing provider binaries. The
//! submodules contain builtin implementations of wasmCloud capabilities providers.
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context as _};
use async_nats::Client;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::{stream, Future, StreamExt};
use nkeys::XKey;
use tokio::io::AsyncWriteExt;
use tokio::process;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{error, instrument, trace, warn};
use uuid::Uuid;
use wascap::jwt::{CapabilityProvider, Token};
use wasmcloud_core::{provider_config_update_subject, HealthCheckResponse, HostData, OtelConfig};
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_tracing::context::TraceContextInjector;

use crate::event::EventPublisher;
use crate::jwt;
use crate::wasmbus::injector_to_headers;
use crate::wasmbus::{config::ConfigBundle, Annotations};

use super::Host;

mod http_server;
mod messaging_nats;

/// A trait for sending and receiving messages to/from a provider
#[async_trait::async_trait]
pub trait ProviderManager: Send + Sync {
    /// Put a link to the provider
    async fn put_link(
        &self,
        link: &wasmcloud_core::InterfaceLinkDefinition,
        target: &str,
    ) -> anyhow::Result<()>;

    /// Delete a link from the provider
    async fn delete_link(
        &self,
        link: &wasmcloud_core::InterfaceLinkDefinition,
        target: &str,
    ) -> anyhow::Result<()>;
}

/// An Provider instance
#[derive(Debug)]
pub(crate) struct Provider {
    pub(crate) image_ref: String,
    pub(crate) claims_token: Option<jwt::Token<jwt::CapabilityProvider>>,
    pub(crate) xkey: XKey,
    pub(crate) annotations: Annotations,
    /// Shutdown signal for the provider, set to `false` initially. When set to `true`, the
    /// tasks running the provider, health check, and config watcher will stop.
    pub(crate) shutdown: Arc<AtomicBool>,
    /// Tasks running the provider, health check, and config watcher
    pub(crate) tasks: JoinSet<()>,
}

impl Host {
    /// Fetch configuration and secrets for a capability provider, forming the host configuration
    /// with links, config and secrets to pass to that provider. Also returns the config bundle
    /// which is used to watch for changes to the configuration, or can be discarded if
    /// configuration updates aren't necessary.
    pub(crate) async fn prepare_provider_config(
        &self,
        config: &[String],
        claims_token: Option<&Token<CapabilityProvider>>,
        provider_id: &str,
        provider_xkey: &XKey,
        annotations: &BTreeMap<String, String>,
    ) -> anyhow::Result<(HostData, ConfigBundle)> {
        let (config, secrets) = self
            .fetch_config_and_secrets(
                config,
                claims_token.as_ref().map(|t| &t.jwt),
                annotations.get("wasmcloud.dev/appspec"),
            )
            .await?;
        // We only need to store the public key of the provider xkey, as the private key is only needed by the provider
        let xkey = XKey::from_public_key(&provider_xkey.public_key())
            .context("failed to create XKey from provider public key xkey")?;

        // Prepare startup links by generating the source and target configs. Note that because the provider may be the source
        // or target of a link, we need to iterate over all links to find the ones that involve the provider.
        let all_links = self.links.read().await;
        let provider_links = all_links
            .values()
            .flatten()
            .filter(|link| link.source_id() == provider_id || link.target() == provider_id);
        let link_definitions = stream::iter(provider_links)
            .filter_map(|link| async {
                if link.source_id() == provider_id || link.target() == provider_id {
                    match self
                        .resolve_link_config(
                            link.clone(),
                            claims_token.as_ref().map(|t| &t.jwt),
                            annotations.get("wasmcloud.dev/appspec"),
                            &xkey,
                        )
                        .await
                    {
                        Ok(provider_link) => Some(provider_link),
                        Err(e) => {
                            error!(
                                error = ?e,
                                provider_id,
                                source_id = link.source_id(),
                                target = link.target(),
                                "failed to resolve link config, skipping link"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<wasmcloud_core::InterfaceLinkDefinition>>()
            .await;

        let secrets = {
            // NOTE(brooksmtownsend): This trait import is used here to ensure we're only exposing secret
            // values when we need them.
            use secrecy::ExposeSecret;
            secrets
                .iter()
                .map(|(k, v)| match v.expose_secret() {
                    SecretValue::String(s) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::String(s.to_owned()),
                    ),
                    SecretValue::Bytes(b) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::Bytes(b.to_owned()),
                    ),
                })
                .collect()
        };
        let host_config = config.get_config().await.clone();
        let lattice_rpc_user_seed = self
            .host_config
            .rpc_key
            .as_ref()
            .map(|key| key.seed())
            .transpose()
            .context("private key missing for provider RPC key")?;
        let default_rpc_timeout_ms = Some(
            self.host_config
                .rpc_timeout
                .as_millis()
                .try_into()
                .context("failed to convert rpc_timeout to u64")?,
        );
        let otel_config = OtelConfig {
            enable_observability: self.host_config.otel_config.enable_observability,
            enable_traces: self.host_config.otel_config.enable_traces,
            enable_metrics: self.host_config.otel_config.enable_metrics,
            enable_logs: self.host_config.otel_config.enable_logs,
            observability_endpoint: self.host_config.otel_config.observability_endpoint.clone(),
            traces_endpoint: self.host_config.otel_config.traces_endpoint.clone(),
            metrics_endpoint: self.host_config.otel_config.metrics_endpoint.clone(),
            logs_endpoint: self.host_config.otel_config.logs_endpoint.clone(),
            protocol: self.host_config.otel_config.protocol,
            additional_ca_paths: self.host_config.otel_config.additional_ca_paths.clone(),
            trace_level: self.host_config.otel_config.trace_level.clone(),
            ..Default::default()
        };

        // The provider itself needs to know its private key
        let provider_xkey_private_key = if let Ok(seed) = provider_xkey.seed() {
            seed
        } else {
            // This should never happen since this returns an error when an Xkey is
            // created from a public key, but if we can't generate one for whatever
            // reason, we should bail.
            bail!("failed to generate seed for provider xkey")
        };
        let host_data = HostData {
            host_id: self.host_key.public_key(),
            lattice_rpc_prefix: self.host_config.lattice.to_string(),
            link_name: "default".to_string(),
            lattice_rpc_user_jwt: self.host_config.rpc_jwt.clone().unwrap_or_default(),
            lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
            lattice_rpc_url: self.host_config.rpc_nats_url.to_string(),
            env_values: vec![],
            instance_id: Uuid::new_v4().to_string(),
            provider_key: provider_id.to_string(),
            link_definitions,
            config: host_config,
            secrets,
            provider_xkey_private_key,
            host_xkey_public_key: self.secrets_xkey.public_key(),
            cluster_issuers: vec![],
            default_rpc_timeout_ms,
            log_level: Some(self.host_config.log_level.clone()),
            structured_logging: self.host_config.enable_structured_logging,
            otel_config,
        };
        Ok((host_data, config))
    }

    /// Start a binary provider
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_binary_provider(
        self: Arc<Self>,
        path: PathBuf,
        host_data: HostData,
        config: Arc<RwLock<ConfigBundle>>,
        provider_xkey: XKey,
        provider_id: &str,
        config_names: Vec<String>,
        claims_token: Option<Token<CapabilityProvider>>,
        annotations: BTreeMap<String, String>,
        shutdown: Arc<AtomicBool>,
    ) -> anyhow::Result<JoinSet<()>> {
        trace!("spawn provider process");

        let mut tasks = JoinSet::new();

        // Spawn a task to ensure the provider is restarted if it exits prematurely,
        // updating the configuration as needed
        tasks.spawn(
            Arc::clone(&self)
                .run_provider(
                    path,
                    host_data,
                    Arc::clone(&config),
                    provider_xkey,
                    provider_id.to_string(),
                    config_names,
                    claims_token,
                    annotations,
                    shutdown.clone(),
                )
                .await?,
        );

        // Spawn a task to check the health of the provider every 30 seconds
        tasks.spawn(check_health(
            Arc::clone(&self.rpc_nats),
            self.event_publisher.clone(),
            Arc::clone(&self.host_config.lattice),
            self.host_key.public_key(),
            provider_id.to_string(),
        ));

        Ok(tasks)
    }

    /// Run and supervise a binary provider, restarting it if it exits prematurely.
    #[allow(clippy::too_many_arguments)]
    async fn run_provider(
        self: Arc<Self>,
        path: PathBuf,
        host_data: HostData,
        config_bundle: Arc<RwLock<ConfigBundle>>,
        provider_xkey: XKey,
        provider_id: String,
        config_names: Vec<String>,
        claims_token: Option<Token<CapabilityProvider>>,
        annotations: BTreeMap<String, String>,
        shutdown: Arc<AtomicBool>,
    ) -> anyhow::Result<impl Future<Output = ()>> {
        let host_data =
            serde_json::to_vec(&host_data).context("failed to serialize provider data")?;

        // If there's any issues starting the provider, we want to exit immediately
        let child = Arc::new(RwLock::new(
            provider_command(&path, host_data)
                .await
                .context("failed to configure binary provider command")?,
        ));
        let lattice = Arc::clone(&self.host_config.lattice);
        Ok(async move {
            // Use a JoinSet to manage the config watcher task so that
            // it can be cancelled on drop and replaced with new config
            // when a provider restarts
            let mut config_task = JoinSet::new();
            config_task.spawn(watch_config(
                Arc::clone(&self.rpc_nats),
                Arc::clone(&config_bundle),
                Arc::clone(&lattice),
                provider_id.clone(),
            ));
            loop {
                let mut child = child.write().await;
                match child.wait().await {
                    Ok(status) => {
                        // When the provider is shutting down, don't restart it
                        if shutdown.load(Ordering::Relaxed) {
                            trace!(
                                path = ?path.display(),
                                status = ?status,
                                "provider exited but will not be restarted since it's shutting down",
                            );
                            // Avoid a hot loop by waiting 1s before checking the status again
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }

                        warn!(
                            path = ?path.display(),
                            status = ?status,
                            "restarting provider that exited while being supervised",
                        );

                        let (host_data, new_config_bundle) = match self
                            .prepare_provider_config(
                                &config_names,
                                claims_token.as_ref(),
                                &provider_id,
                                &provider_xkey,
                                &annotations,
                            )
                            .await
                            .map(|(host_data, config)| {
                                (
                                    serde_json::to_vec(&host_data)
                                        .context("failed to serialize provider data"),
                                    Arc::new(RwLock::new(config)),
                                )
                            }) {
                            Ok((Ok(host_data), new_config_bundle)) => {
                                (host_data, new_config_bundle)
                            }
                            Err(e) => {
                                error!(err = ?e, "failed to prepare provider host data while restarting");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                            Ok((Err(e), _)) => {
                                error!(err = ?e, "failed to serialize provider host data while restarting");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                        };

                        // Stop the config watcher and start a new one with the new config bundle
                        config_task.abort_all();
                        config_task.spawn(watch_config(
                            Arc::clone(&self.rpc_nats),
                            new_config_bundle,
                            Arc::clone(&lattice),
                            provider_id.clone(),
                        ));

                        // Restart the provider by attempting to re-execute the binary with the same
                        // host data
                        let Ok(child_cmd) = provider_command(&path, host_data).await else {
                            error!(path = ?path.display(), "failed to restart provider");
                            shutdown.store(true, Ordering::Relaxed);
                            return;
                        };
                        *child = child_cmd;

                        // To avoid a tight loop, we wait 5 seconds after restarting. In the worst case,
                        // the provider will continually execute and exit every 5 seconds.
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(e) => {
                        error!(
                            path = ?path.display(),
                            err = ?e,
                            "failed to wait for provider to execute",
                        );

                        shutdown.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
        })
    }
}

/// Using the provided path as the provider binary, start the provider process and
/// pass the host data to it over stdin. Returns the child process handle which
/// has already been spawned.
async fn provider_command(path: &Path, host_data: Vec<u8>) -> anyhow::Result<process::Child> {
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

    // Pass through any OTEL configuration options to the provider as well
    for (k, v) in env::vars() {
        if k.starts_with("OTEL_") {
            let _ = child_cmd.env(k, v);
        }
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
/// Returns a future that should be polled to continually check provider
/// health every 30 seconds until the health receiver gets a message to stop
fn check_health(
    rpc_nats: Arc<Client>,
    event_publisher: Arc<dyn EventPublisher + Send + Sync>,
    lattice: Arc<str>,
    host_id: String,
    provider_id: String,
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
            let _ = health_check.tick().await;
            trace!(?provider_id, "performing provider health check");
            let request =
                async_nats::Request::new()
                    .payload(Bytes::new())
                    .headers(injector_to_headers(
                        &TraceContextInjector::default_with_span(),
                    ));
            if let Ok(async_nats::Message { payload, .. }) =
                rpc_nats.send_request(health_subject.clone(), request).await
            {
                match (
                    serde_json::from_slice::<HealthCheckResponse>(&payload),
                    previous_healthy,
                ) {
                    (Ok(HealthCheckResponse { healthy: true, .. }), false) => {
                        trace!(?provider_id, "provider health check succeeded");
                        previous_healthy = true;
                        if let Err(e) = event_publisher
                            .publish_event(
                                "health_check_passed",
                                crate::event::provider_health_check(&host_id, &provider_id),
                            )
                            .await
                        {
                            warn!(
                                ?e,
                                ?provider_id,
                                "failed to publish provider health check succeeded event",
                            );
                        }
                    }
                    (Ok(HealthCheckResponse { healthy: false, .. }), true) => {
                        trace!(?provider_id, "provider health check failed");
                        previous_healthy = false;
                        if let Err(e) = event_publisher
                            .publish_event(
                                "health_check_failed",
                                crate::event::provider_health_check(&host_id, &provider_id),
                            )
                            .await
                        {
                            warn!(
                                ?e,
                                ?provider_id,
                                "failed to publish provider health check failed event",
                            );
                        }
                    }
                    // If the provider health status didn't change, we simply publish a health check status event
                    (Ok(_), _) => {
                        if let Err(e) = event_publisher
                            .publish_event(
                                "health_check_status",
                                crate::event::provider_health_check(&host_id, &provider_id),
                            )
                            .await
                        {
                            warn!(
                                ?e,
                                ?provider_id,
                                "failed to publish provider health check status event",
                            );
                        }
                    }
                    _ => warn!(
                        ?provider_id,
                        "failed to deserialize provider health check response"
                    ),
                }
            } else {
                warn!(
                    ?provider_id,
                    "failed to request provider health, retrying in 30 seconds"
                );
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
) -> impl Future<Output = ()> {
    let subject = provider_config_update_subject(&lattice, &provider_id);
    trace!(?provider_id, "starting config update listener");
    async move {
        loop {
            let mut config = config.write().await;
            if let Ok(update) = config.changed().await {
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
            } else {
                break;
            };
        }
    }
}
