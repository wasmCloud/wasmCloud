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

use crate::bindings::wrpc::extension::types::WitMetadata;
use crate::bindings::wrpc::extension::{
    configurable,
    manageable::{self},
    types::InterfaceConfig,
};
use anyhow::Context as _;
use async_nats::Client;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use futures::{stream, Future, StreamExt};

use nkeys::XKey;
use tokio::io::AsyncWriteExt;
use tokio::process;
use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::{debug, error, info, instrument, trace, warn};
use uuid::Uuid;
use wascap::jwt::{CapabilityProvider, Token};
use wasmcloud_control_interface::SatisfiedProviderInterfaces;
use wasmcloud_core::secrets::SecretValue;
use wasmcloud_core::{ExtensionData, InterfaceLinkDefinition, OtelConfig};
use wasmcloud_tracing::context::TraceContextInjector;

use crate::event::EventPublisher;
use crate::jwt;
use crate::wasmbus::injector_to_headers;
use crate::wasmbus::{config::ConfigBundle, Annotations};

use super::Host;

// Add internal provider modules to the host
mod http_client;
mod http_server;
mod messaging_nats;

/// Convert wasmcloud_core::secrets::SecretValue to bindings SecretValue type
fn convert_secret_value(
    value: SecretValue,
) -> crate::bindings::wrpc::extension::types::SecretValue {
    match value {
        SecretValue::String(s) => crate::bindings::wrpc::extension::types::SecretValue::String(s),
        SecretValue::Bytes(b) => {
            crate::bindings::wrpc::extension::types::SecretValue::Bytes(b.into())
        }
    }
}

/// A trait for sending and receiving messages to/from a provider
#[async_trait::async_trait]
pub trait ProviderManager: Send + Sync {
    /// Notify the provider of configuration updates for its imports
    async fn put_interface_import_config(
        &self,
        provider_id: &str,
        target_id: &str,
        link_name: &str,
        config: &InterfaceConfig,
    ) -> anyhow::Result<()>;

    /// Notify the provider of configuration updates for its exports
    async fn put_interface_export_config(
        &self,
        provider_id: &str,
        source_id: &str,
        link_name: &str,
        config: &InterfaceConfig,
    ) -> anyhow::Result<()>;

    /// Delete a link from the provider source or target
    async fn delete_interface_import_config(
        &self,
        provider_id: &str,
        target_id: &str,
        link_name: &str,
    ) -> anyhow::Result<()>;

    /// Delete a link from the provider source or target
    async fn delete_interface_export_config(
        &self,
        provider_id: &str,
        source_id: &str,
        link_name: &str,
    ) -> anyhow::Result<()>;

    /// Helper function which produces a client extension necessary for interacting with provider functionality over wrpc
    async fn produce_extension_wrpc_client(
        &self,
        target: &str,
    ) -> anyhow::Result<wrpc_transport_nats::Client>;

    /// Request a provider to gracefully shutdown via the wRPC manageable interface
    async fn shutdown_provider(&self, provider_id: &str) -> anyhow::Result<()>;
}

impl Extension {
    pub fn satisfied_interfaces(&self) -> &SatisfiedProviderInterfaces {
        &self.satisfied_interfaces
    }
}

#[derive(Debug)]
pub(crate) struct Extension {
    /// A list of interfaces that this extension satisfies
    pub(crate) satisfied_interfaces: SatisfiedProviderInterfaces,
    /// OCI image reference (only populated for host-managed extensions)
    pub(crate) image_ref: Option<String>,
    pub(crate) claims_token: Option<jwt::Token<jwt::CapabilityProvider>>,
    /// Annotations for this extension
    pub(crate) annotations: Annotations,
    /// Whether this extension's lifecycle is managed by the host
    /// (true = host spawned it, false = external process)
    pub(crate) is_managed: bool,
    /// XKey public key from bind response (for encrypting secrets to this extension)
    /// Empty if extension not fully binded yet.
    pub(crate) xkey_public: Option<XKey>,
    /// Tasks running health checks, etc.
    pub(crate) tasks: JoinSet<()>,
}

pub struct PreparedProviderConfig {
    /// The config bundle for the provider
    pub config_bundle: ConfigBundle,
    /// The encrypted secrets for the provider
    pub encrypted_secrets: Vec<(String, wasmcloud_core::secrets::SecretValue)>,
    /// The link definitions for the provider
    pub link_definitions: Vec<InterfaceLinkDefinition>,
}

impl Host {
    /// Fetch configuration and secrets for a capability provider, forming the host configuration
    /// with config and secrets to pass to that provider. Also returns the config bundle
    /// which is used to watch for changes to the configuration, or can be discarded if
    /// configuration updates aren't necessary.
    /// Returns (ExtensionData, ConfigBundle, encrypted_secrets)
    /// The encrypted_secrets are encrypted with the provider's XKey and can be sent over wRPC.
    pub(crate) async fn prepare_extension_data(
        &self,
        provider_id: &str,
    ) -> anyhow::Result<ExtensionData> {
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

        let ext_data = ExtensionData {
            lattice_rpc_prefix: self.host_config.lattice.to_string(),
            lattice_rpc_user_jwt: self.host_config.rpc_jwt.clone().unwrap_or_default(),
            lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
            lattice_rpc_url: self.host_config.rpc_nats_url.to_string(),
            instance_id: Uuid::new_v4().to_string(),
            provider_id: provider_id.to_string(),
            default_rpc_timeout_ms,
            log_level: Some(self.host_config.log_level.clone()),
            structured_logging: self.host_config.enable_structured_logging,
            otel_config,
            host_id: self.host_key.public_key(),
        };

        Ok(ext_data)
    }

    // This requires a extension xkey, therefore should be done after a extension has been bind to.
    pub(crate) async fn prepare_extension_config(
        &self,
        config: &[String],
        claims_token: Option<&Token<CapabilityProvider>>,
        provider_id: &str,
        provider_xkey: &XKey,
        annotations: &BTreeMap<String, String>,
    ) -> anyhow::Result<PreparedProviderConfig> {
        let (config_bundle, secrets) = self
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

        // - String secrets: encrypt bytes, then base64 encode to preserve as String variant
        // - Bytes secrets: encrypt bytes directly
        use base64::Engine as _;
        let encrypted_secrets: Vec<(String, wasmcloud_core::secrets::SecretValue)> = {
            use secrecy::ExposeSecret;
            let mut result = Vec::with_capacity(secrets.len());
            for (k, v) in secrets.iter() {
                let encrypted_value = match v.expose_secret() {
                    SecretValue::String(s) => {
                        let encrypted_bytes = self
                            .secrets_xkey
                            .seal(s.as_bytes(), &provider_xkey)
                            .context("failed to encrypt secret string")?;
                        // Base64 encode so encrypted bytes can be stored as valid UTF-8 string
                        let encoded =
                            base64::engine::general_purpose::STANDARD.encode(&encrypted_bytes);
                        wasmcloud_core::secrets::SecretValue::String(encoded)
                    }
                    SecretValue::Bytes(bytes) => {
                        let encrypted_bytes = self
                            .secrets_xkey
                            .seal(&bytes, &provider_xkey)
                            .context("failed to encrypt secret bytes")?;
                        wasmcloud_core::secrets::SecretValue::Bytes(encrypted_bytes)
                    }
                };
                result.push((k.clone(), encrypted_value));
            }
            result
        };

        Ok(PreparedProviderConfig {
            config_bundle,
            encrypted_secrets,
            link_definitions,
        })
    }

    /// Helper function to perform the second phase of provider initialization.
    /// This includes health checking, binding, preparing final configuration, and applying it.
    /// Returns optiuional if configurable interface is not satisfied
    async fn complete_provider_configuration(
        &self,
        provider_id: &str,
        config_names: &[String],
        claims_token: Option<&Token<CapabilityProvider>>,
        annotations: &BTreeMap<String, String>,
        wrpc: &wrpc_transport_nats::Client,
    ) -> anyhow::Result<Option<ConfigBundle>> {
        // 1. Health Check Loop
        info!(provider_id, "Waiting for provider to become healthy...");
        const MAX_HEALTH_RETRIES: u32 = 10;
        const HEALTH_RETRY_DELAY: Duration = Duration::from_millis(500);
        for attempt in 0..MAX_HEALTH_RETRIES {
            if let Ok(Ok(resp)) = manageable::health_request(wrpc, None).await {
                if resp.healthy {
                    info!(provider_id, "Provider is healthy.");
                    break;
                }
            }
            if attempt == MAX_HEALTH_RETRIES - 1 {
                anyhow::bail!("Provider did not become healthy in time");
            }
            tokio::time::sleep(HEALTH_RETRY_DELAY).await;
        }

        // 2. Bind to the Provider
        info!(provider_id, "Binding to provider...");
        self.bind_provider(wrpc, provider_id)
            .await
            .context("Failed to bind to provider")?;

        if let Some(extension) = self.extensions.read().await.get(provider_id) {
            // If the provider isn't configurable, no need to proceed with config/linking
            if !extension.satisfied_interfaces().is_configurable() {
                warn!(
                    provider_id,
                    "Provider does not satisfy configurable interface, skipping configuration."
                );
                // An empty config bundle is fine, it just means no config will be watched
                return Ok(None);
            }

            // Retrieve the XKey that bind_provider stored
            let provider_xkey = extension
                .xkey_public
                .clone()
                .context("Provider XKey not found after bind")?;

            // Prepare Final Configuration
            let prepared_config = self
                .prepare_extension_config(
                    config_names,
                    claims_token,
                    provider_id,
                    &provider_xkey,
                    annotations,
                )
                .await
                .context("Failed to prepare provider link and secret configuration")?;

            // Apply Base Configuration
            info!(
                provider_id,
                "Applying initial base configuration to provider..."
            );
            {
                let config_map = prepared_config.config_bundle.get_config().await;
                let base_config = crate::bindings::wrpc::extension::types::BaseConfig {
                    config: config_map
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    secrets: prepared_config
                        .encrypted_secrets
                        .into_iter()
                        .map(|(k, v)| (k, convert_secret_value(v)))
                        .collect(),
                };
                configurable::update_base_config(wrpc, None, &base_config)
                    .await?
                    .map_err(|e| anyhow::anyhow!("Provider rejected base config update: {e}"))?;
            }

            // Apply Link-Specific Configurations
            self.apply_link_definitions(provider_id, prepared_config.link_definitions, &wrpc)
                .await?;

            info!(provider_id, "Provider configured successfully.");
            Ok(Some(prepared_config.config_bundle))
        } else {
            Err(anyhow::anyhow!(
                "Provider {} not found in extensions map after bind",
                provider_id
            ))
        }
    }

    /// Applies a set of link definitions to a provider, calling the appropriate
    /// import or export configuration update functions.
    async fn apply_link_definitions(
        &self,
        provider_id: &str,
        link_definitions: Vec<InterfaceLinkDefinition>,
        wrpc: &wrpc_transport_nats::Client,
    ) -> anyhow::Result<()> {
        for link in link_definitions {
            let metadata = WitMetadata {
                namespace: link.wit_namespace.clone(),
                package: link.wit_package.clone(),
                interfaces: link.interfaces.clone(),
            };
            if link.source_id == provider_id {
                // Provider is the source (caller), so this is an IMPORT configuration.
                let config = crate::bindings::wrpc::extension::types::InterfaceConfig {
                    metadata,
                    config: link.source_config.into_iter().collect(),
                    secrets: link.source_secrets.map(|secrets| {
                        secrets
                            .into_iter()
                            .map(|(k, v)| (k, convert_secret_value(v)))
                            .collect()
                    }),
                };
                info!(target_id = %link.target, link_name = %link.name, "Applying import configuration to provider");
                configurable::update_interface_import_config(
                    wrpc,
                    None,
                    &link.target,
                    &link.name,
                    &config,
                )
                .await?
                .map_err(|e| anyhow::anyhow!("Provider rejected import config: {e}"))?;
            } else if link.target == provider_id {
                // Provider is the target (callee), so this is an EXPORT configuration.
                let config = crate::bindings::wrpc::extension::types::InterfaceConfig {
                    metadata,
                    config: link.target_config.into_iter().collect(),
                    secrets: link.target_secrets.map(|secrets| {
                        secrets
                            .into_iter()
                            .map(|(k, v)| (k, convert_secret_value(v)))
                            .collect()
                    }),
                };
                info!(source_id = %link.source_id, link_name = %link.name, "Applying export configuration to provider");
                configurable::update_interface_export_config(
                    wrpc,
                    None,
                    &link.source_id,
                    &link.name,
                    &config,
                )
                .await?
                .map_err(|e| anyhow::anyhow!("Provider rejected export config: {e}"))?;
            }
        }
        Ok(())
    }

    /// Bind to a provider, exchange public keys, and store the provider's public key.
    async fn bind_provider(
        &self,
        wrpc_client: &wrpc_transport_nats::Client,
        provider_id: &str,
    ) -> anyhow::Result<()> {
        let bind_request = manageable::BindRequest {
            identity_token: None,
            host_xkey: Some(self.secrets_xkey.public_key().as_bytes().to_vec().into()),
        };

        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", crate::WasmbusHost::host_source_id());

        debug!(provider_id, "Binding to provider");

        let response = match manageable::bind(wrpc_client, Some(headers), &bind_request).await {
            Ok(Ok(response)) => {
                info!(provider_id, "Successfully bound to provider");
                response
            }
            Ok(Err(app_error)) => {
                return Err(anyhow::anyhow!(
                    "Provider {} bind request failed (application error): {}",
                    provider_id,
                    app_error
                ));
            }
            Err(transport_error) => {
                return Err(transport_error.context(format!(
                    "Failed to communicate with provider {} for bind request",
                    provider_id
                )));
            }
        };

        // Extract and store the provider's public key from the response
        if let Some(pubkey_bytes) = response.provider_pubkey.as_deref() {
            let pubkey_str = std::str::from_utf8(pubkey_bytes)
                .context("Provider public key was not valid UTF-8")?;
            let xkey = XKey::from_public_key(pubkey_str)
                .context("Failed to decode provider xkey from bind response")?;

            let mut extensions = self.extensions.write().await;
            if let Some(ext) = extensions.get_mut(provider_id) {
                ext.xkey_public = Some(xkey);
                info!(provider_id, "Successfully stored provider's public key.");
            } else {
                return Err(anyhow::anyhow!(
                    "Provider {} not found in host extensions during bind",
                    provider_id
                ));
            }
        } else {
            warn!(
                provider_id,
                "Provider did not return a public key in bind response. Cannot send secrets."
            );
        }

        Ok(())
    }

    /// Start provider tasks for a managed provider
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_managed_provider(
        self: Arc<Self>,
        path: PathBuf,
        claims_token: Option<Token<CapabilityProvider>>,
        ext_data: ExtensionData,
        provider_id: &str,
        config_names: Vec<String>,
        annotations: BTreeMap<String, String>,
        shutdown: Arc<AtomicBool>,
    ) -> anyhow::Result<JoinSet<()>> {
        trace!("starting managed provider tasks for {}", provider_id);

        // Create the single wRPC client that will be shared by all tasks for this provider
        let wrpc_client = Arc::new(
            self.provider_manager
                .produce_extension_wrpc_client(provider_id)
                .await?,
        );

        // Get the supervision future from `run_managed_provider`
        let supervision_task_future = Arc::clone(&self)
            .run_managed_provider(
                path,
                ext_data,
                provider_id.to_string(),
                config_names,
                claims_token,
                annotations,
                shutdown,
                Arc::clone(&wrpc_client),
            )
            .await?;

        let mut tasks = JoinSet::new();

        // Spawn the main supervision task
        tasks.spawn(supervision_task_future);

        // Spawn the separate, periodic health checker task
        tasks.spawn(check_health(
            wrpc_client,
            self.event_publisher.clone(),
            self.host_key.public_key(),
            provider_id.to_string(),
        ));

        Ok(tasks)
    }

    /// Start provider tasks for an external provider.
    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_external_provider(
        self: Arc<Self>,
        claims_token: Option<Token<CapabilityProvider>>,
        provider_id: &str,
        config_names: Vec<String>,
        annotations: BTreeMap<String, String>,
    ) -> anyhow::Result<JoinSet<()>> {
        trace!("starting external provider tasks for {}", provider_id);

        let mut tasks = JoinSet::new();

        let wrpc_client = Arc::new(
            self.provider_manager
                .produce_extension_wrpc_client(provider_id)
                .await?,
        );

        // Perform the full health-check, bind, and configuration flow.
        let config_bundle = self
            .complete_provider_configuration(
                provider_id,
                &config_names,
                claims_token.as_ref(),
                &annotations,
                &wrpc_client,
            )
            .await?;

        // Spawn the periodic health checker.
        tasks.spawn(check_health(
            Arc::clone(&wrpc_client),
            self.event_publisher.clone(),
            self.host_key.public_key(),
            provider_id.to_string(),
        ));

        // Spawn the config watcher task if provider supports configuration.
        // This task will be aborted via tasks.abort_all() when the provider stops.
        if let Some(bundle) = config_bundle {
            let provider_id_owned = provider_id.to_string();
            tasks.spawn(async move {
                let config_bundle_arc = Arc::new(RwLock::new(bundle));
                watch_config(
                    self.rpc_nats.clone(),
                    config_bundle_arc,
                    self.host_config.lattice.clone(),
                    Arc::from(self.host_key.public_key()),
                    provider_id_owned.clone(),
                )
                .await;
                trace!(provider_id = %provider_id_owned, "config watcher finished, external provider task exiting");
            });
        }

        Ok(tasks)
    }

    /// Run and supervise a binary provider, restarting it if it exits prematurely.
    #[allow(clippy::too_many_arguments)]
    async fn run_managed_provider(
        self: Arc<Self>,
        path: PathBuf,
        extension_data: ExtensionData,
        provider_id: String,
        config_names: Vec<String>,
        claims_token: Option<Token<CapabilityProvider>>,
        annotations: BTreeMap<String, String>,
        shutdown: Arc<AtomicBool>,
        wrpc_client: Arc<wrpc_transport_nats::Client>,
    ) -> anyhow::Result<impl Future<Output = ()>> {
        let extension_data_bytes =
            serde_json::to_vec(&extension_data).context("failed to serialize provider data")?;

        // If there's any issues starting the provider, we want to exit immediately
        let child = Arc::new(RwLock::new(
            provider_command(&path, extension_data_bytes)
                .await
                .context("failed to configure binary provider command")?,
        ));

        Ok(async move {
            // Perform initial configuration
            let config_bundle = match self
                .complete_provider_configuration(
                    &provider_id,
                    &config_names,
                    claims_token.as_ref(),
                    &annotations,
                    &wrpc_client,
                )
                .await
            {
                Ok(bundle) => bundle,
                Err(e) => {
                    error!(error = %e, provider_id, "Failed during initial provider configuration, shutting down.");
                    shutdown.store(true, Ordering::Relaxed);
                    return;
                }
            };

            // Now that initial config is done, start the config watcher if provider supports configuration
            let mut config_task = JoinSet::new();
            if let Some(bundle) = config_bundle {
                let config_bundle_arc = Arc::new(RwLock::new(bundle));
                config_task.spawn(watch_config(
                    Arc::clone(&self.rpc_nats),
                    config_bundle_arc,
                    Arc::clone(&self.host_config.lattice),
                    Arc::from(self.host_key.public_key()),
                    provider_id.clone(),
                ));
            }

            // supervision loop
            loop {
                let mut child_guard = child.write().await;
                match child_guard.wait().await {
                    Ok(status) => {
                        if shutdown.load(Ordering::Relaxed) {
                            trace!(path = ?path.display(), %status, "provider exited but will not be restarted during shutdown");
                            return;
                        }

                        warn!(
                            path = ?path.display(),
                            %status,
                            "restarting provider that exited while being supervised"
                        );

                        // Respawn the process
                        let extension_data = match self.prepare_extension_data(&provider_id).await {
                            Ok(d) => d,
                            Err(e) => {
                                error!(error = %e, provider_id, "failed to prepare extension data while restarting, shutting down");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                        };
                        let extension_data_bytes = match serde_json::to_vec(&extension_data) {
                            Ok(b) => b,
                            Err(e) => {
                                error!(error = %e, provider_id, "failed to serialize extension data while restarting, shutting down");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                        };
                        let new_child = match provider_command(&path, extension_data_bytes).await {
                            Ok(c) => c,
                            Err(e) => {
                                error!(error = %e, path = %path.display(), "failed to restart provider process, shutting down");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                        };
                        *child_guard = new_child;

                        // Drop the write guard before the await point of re-configuration
                        drop(child_guard);

                        // Re-run the configuration process for the new instance
                        let new_config_bundle = match self
                            .complete_provider_configuration(
                                &provider_id,
                                &config_names,
                                claims_token.as_ref(),
                                &annotations,
                                &wrpc_client,
                            )
                            .await
                        {
                            Ok(bundle) => bundle,
                            Err(e) => {
                                error!(error = %e, provider_id, "Failed during provider restart configuration, shutting down.");
                                shutdown.store(true, Ordering::Relaxed);
                                return;
                            }
                        };

                        // Stop the old config watcher and start a new one if provider supports configuration
                        config_task.abort_all();
                        if let Some(bundle) = new_config_bundle {
                            let config_bundle_arc = Arc::new(RwLock::new(bundle));
                            config_task.spawn(watch_config(
                                Arc::clone(&self.rpc_nats),
                                config_bundle_arc,
                                Arc::clone(&self.host_config.lattice),
                                Arc::from(self.host_key.public_key()),
                                provider_id.clone(),
                            ));
                        }

                        // To avoid a tight loop, we wait before checking the status again.
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(e) => {
                        error!(path = %path.display(), err = ?e, "failed to wait for provider to execute, shutting down");
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

/// Watch for health check responses from the provider using the manageable interface
///
/// Returns a future that should be polled to continually check provider
/// health every 30 seconds until the task is cancelled
pub(crate) fn check_health(
    wrpc_client: Arc<wrpc_transport_nats::Client>,
    event_publisher: Arc<dyn EventPublisher + Send + Sync>,
    host_id: String,
    provider_id: String,
) -> impl Future<Output = ()> {
    // Check the health of the provider every 30 seconds
    let mut health_check = tokio::time::interval(Duration::from_secs(30));
    let mut previous_healthy = false;

    async move {
        loop {
            let _ = health_check.tick().await;
            trace!(
                ?provider_id,
                "performing provider health check via manageable interface"
            );

            let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
            headers.insert("source-id", crate::WasmbusHost::host_source_id());

            // Perform health check via manageable interface
            match manageable::health_request(wrpc_client.as_ref(), Some(headers)).await {
                Ok(Ok(health_response)) => {
                    let currently_healthy = health_response.healthy;

                    match (currently_healthy, previous_healthy) {
                        (true, false) => {
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
                        (false, true) => {
                            trace!(
                                ?provider_id,
                                message = health_response.message,
                                "provider health check failed"
                            );
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
                        _ => {
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
                    }
                }
                Ok(Err(app_error)) => {
                    warn!(
                        ?provider_id,
                        error = app_error,
                        "provider health check failed (application error), retrying in 30 seconds"
                    );
                }
                Err(transport_error) => {
                    warn!(
                        ?provider_id,
                        error = %transport_error,
                        "failed to communicate with provider for health check, retrying in 30 seconds"
                    );
                }
            }
        }
    }
}

/// Watch for config updates and send them to the provider via the configurable wRPC interface
///
/// Returns a future that continually checks provider config changes
/// until the config receiver gets a message
pub(crate) fn watch_config(
    nats_client: Arc<Client>,
    config: Arc<RwLock<ConfigBundle>>,
    lattice: Arc<str>,
    host_id: Arc<str>,
    provider_id: String,
) -> impl Future<Output = ()> {
    use crate::wasmbus::injector_to_headers;
    use wasmcloud_tracing::context::TraceContextInjector;

    trace!(?provider_id, "starting config update listener");
    async move {
        // Create wRPC client for extension interface (host-specific subject)
        let prefix = format!(
            "wasmbus.ctl.v1.{}.extension.{}.{}",
            &lattice, &provider_id, &host_id
        );
        let wrpc_client = match wrpc_transport_nats::Client::new(
            Arc::clone(&nats_client),
            prefix.clone(),
            Some(prefix.into()),
        )
        .await
        {
            Ok(client) => client,
            Err(err) => {
                error!(%err, ?provider_id, ?lattice, "failed to create wrpc client for config updates");
                return;
            }
        };

        loop {
            let mut config = config.write().await;
            if let Ok(update) = config.changed().await {
                trace!(?provider_id, "provider config bundle changed");

                use crate::bindings::wrpc::extension::types::BaseConfig;

                let config_list: Vec<(String, String)> =
                    update.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                // note(luk3ark) - Base config updates don't include secrets on config updates.
                // Secrets are only included on initial config update on binding/initialization.
                // This can be improved in the future by making reactive to changes in secret store as well.
                let base_config = BaseConfig {
                    config: config_list,
                    secrets: Vec::new(), // No secrets provided on update
                };

                let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
                headers.insert("source-id", crate::WasmbusHost::host_source_id());

                trace!(
                    ?provider_id,
                    "sending config update via configurable interface"
                );
                match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    configurable::update_base_config(&wrpc_client, Some(headers), &base_config),
                )
                .await
                {
                    Ok(Ok(Ok(()))) => {
                        trace!(?provider_id, "config update sent successfully");
                    }
                    Ok(Ok(Err(app_error))) => {
                        error!(?provider_id, ?lattice, %app_error, "provider rejected config update");
                    }
                    Ok(Err(transport_error)) => {
                        error!(?provider_id, ?lattice, %transport_error, "failed to send config update to provider");
                    }
                    Err(_) => {
                        error!(
                            ?provider_id,
                            ?lattice,
                            "config update timed out after 10 seconds"
                        );
                    }
                }
            } else {
                break;
            };
        }
    }
}
