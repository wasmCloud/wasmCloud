// NOTE: This comes from wrpc?
#![recursion_limit = "256"]

use std::collections::HashMap;
use std::sync::Arc;

use crate::wasmcloud::wadm::wadm_types::{
    DeleteModelResponse, DeployResponse, GetModelResponse, ModelSummary, PutModelResponse,
    StatusResponse, VersionResponse,
};
use anyhow::{anyhow, bail, Context as _};
use async_nats::subject::ToSubject;
use async_nats::HeaderMap;
use futures::StreamExt;
use opentelemetry_nats::attach_span_context;
use tokio::fs;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;
use wasmcloud::messaging::types::BrokerMessage;
use wasmcloud::wadm::oam_types::OamManifest;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, run_provider, Context, LinkConfig, Provider,
};

mod config;
use config::WadmProviderConfig;

mod translate;

wit_bindgen_wrpc::generate!({
    additional_derives: [
        serde::Serialize,
        serde::Deserialize,
    ],
});

pub async fn run() -> anyhow::Result<()> {
    WadmProvider::run().await
}

/// [`NatsClientBundle`]s hold a NATS client and information (subscriptions)
/// related to it.
///
/// This struct is necssary because subscriptions are *not* automatically removed on client drop,
/// meaning that we must keep track of all subscriptions to close once the client is done
#[derive(Debug)]
struct NatsClientBundle {
    pub client: async_nats::Client,
    pub sub_handles: Vec<(String, JoinHandle<()>)>,
}

impl Drop for NatsClientBundle {
    fn drop(&mut self) {
        for handle in &self.sub_handles {
            handle.1.abort();
        }
    }
}

#[derive(Clone)]
pub struct WadmProvider {
    default_config: WadmProviderConfig,
    handler_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    consumer_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
}

impl Default for WadmProvider {
    fn default() -> Self {
        WadmProvider {
            handler_components: Arc::new(RwLock::new(HashMap::new())),
            consumer_components: Arc::new(RwLock::new(HashMap::new())),
            default_config: Default::default(),
        }
    }
}

impl WadmProvider {
    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "wadm-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
        )
        .await
    }

    /// Build a [`WadmProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> WadmProvider {
        let config = WadmProviderConfig::from_map(&host_data.config);
        if let Ok(config) = config {
            WadmProvider {
                default_config: config,
                ..Default::default()
            }
        } else {
            warn!("Failed to build connection configuration, falling back to default");
            WadmProvider::default()
        }
    }

    /// Attempt to connect to nats url (with jwt credentials, if provided)
    async fn connect(
        &self,
        cfg: WadmProviderConfig,
        component_id: &str,
    ) -> anyhow::Result<NatsClientBundle> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(&seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = &cfg.tls_ca {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = &cfg.tls_ca_file {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = cfg.cluster_uris.first().unwrap();

        // Override inbox prefix if specified
        if let Some(prefix) = cfg.custom_inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix);
        }

        let client = opts
            .name("NATS Messaging Provider") // allow this to show up uniquely in a NATS connection list
            .connect(url)
            .await?;

        // Connections
        let mut sub_handles = Vec::new();
        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue.to_string())),
                None => (sub.as_str(), None),
            };

            sub_handles.push((
                sub.to_string(),
                self.subscribe(&client, component_id, sub.to_string(), queue)
                    .await?,
            ));
        }

        Ok(NatsClientBundle {
            client,
            sub_handles,
        })
    }

    /// Add a regular or queue subscription
    async fn subscribe(
        &self,
        client: &async_nats::Client,
        component_id: &str,
        sub: impl ToSubject,
        queue: Option<String>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut subscriber = match queue {
            Some(queue) => client.queue_subscribe(sub, queue).await,
            None => client.subscribe(sub).await,
        }?;

        debug!(?component_id, "spawning listener for component");

        let component_id = Arc::new(component_id.to_string());
        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(75));

            // Listen for NATS message(s)
            while let Some(mut msg) = subscriber.next().await {
                debug!(?msg, ?component_id, "received messsage");
                // Set up tracing context for the NATS message
                let span = tracing::debug_span!("handle_message", ?component_id);
                match msg.headers {
                    // If there are some headers on the message they might contain a span context
                    // so attempt to attach them.
                    Some(ref h) if !h.is_empty() => {
                        span.in_scope(|| {
                            attach_span_context(&msg);
                        });
                    }
                    // If the header map is completely missing or present but empty, create a new trace context add it
                    // to the message that is flowing through -- i.e. None or Some(h) where h is empty
                    _ => {
                        let mut headers = HeaderMap::new();
                        TraceContextInjector::default_with_span()
                            .iter()
                            .for_each(|(k, v)| headers.insert(k.as_str(), v.as_str()));
                        msg.headers = Some(headers);
                    }
                };

                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Work pool has been closed, exiting queue subscribe");
                        break;
                    }
                };

                let component_id = Arc::clone(&component_id);
                tokio::spawn(async move {
                    dispatch_msg(component_id.as_str(), msg, permit)
                        .instrument(span)
                        .await;
                });
            }
        });

        Ok(join_handle)
    }

    /// Helper function to get the NATS client from the context
    async fn get_client(&self, ctx: Option<Context>) -> anyhow::Result<async_nats::Client> {
        if let Some(ref source_id) = ctx
            .as_ref()
            .and_then(|Context { component, .. }| component.clone())
        {
            let actors = self.consumer_components.read().await;
            let nats_bundle = match actors.get(source_id) {
                Some(nats_bundle) => nats_bundle,
                None => {
                    error!("actor not linked: {source_id}");
                    bail!("actor not linked: {source_id}")
                }
            };
            Ok(nats_bundle.client.clone())
        } else {
            error!("no actor in request");
            bail!("no actor in request")
        }
    }
}

#[instrument(level = "debug", skip_all, fields(component_id = %component_id, subject = %nats_msg.subject, reply_to = ?nats_msg.reply))]
async fn dispatch_msg(
    component_id: &str,
    nats_msg: async_nats::Message,
    _permit: OwnedSemaphorePermit,
) {
    let msg = BrokerMessage {
        body: nats_msg.payload.into(),
        reply_to: nats_msg.reply.map(|s| s.to_string()),
        subject: nats_msg.subject.to_string(),
    };
    debug!(
        subject = msg.subject,
        reply_to = ?msg.reply_to,
        component_id = component_id,
        "sending message to actor",
    );
    if let Err(e) = wasmcloud::messaging::handler::handle_message(
        &get_connection().get_wrpc_client(component_id),
        &msg,
    )
    .await
    {
        error!(
            error = %e,
            "Unable to send message"
        );
    }
}

/// Handle provider control commands
/// `put_link` (new actor link command), `del_link` (remove link command), and shutdown
impl Provider for WadmProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = if config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match WadmProviderConfig::from_map(&config) {
                Ok(cc) => self.default_config.merge(&WadmProviderConfig {
                    subscriptions: Vec::new(),
                    ..cc
                }),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build connection config"));
                }
            }
        };

        let mut update_map = self.consumer_components.write().await;
        let bundle = match self.connect(config, source_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        update_map.insert(source_id.into(), bundle);

        Ok(())
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = if config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match WadmProviderConfig::from_map(&config) {
                Ok(cc) => self.default_config.merge(&cc),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build connection config"));
                }
            }
        };

        let mut update_map = self.handler_components.write().await;
        let bundle = match self.connect(config, target_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        update_map.insert(target_id.into(), bundle);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, component_id: &str) -> anyhow::Result<()> {
        if component_id == get_connection().provider_key() {
            return self.delete_link_as_source(component_id).await;
        }

        self.delete_link_as_target(component_id).await
    }

    #[instrument(level = "info", skip(self))]
    async fn delete_link_as_source(&self, target_id: &str) -> anyhow::Result<()> {
        let mut links = self.handler_components.write().await;
        if let Some(bundle) = links.remove(target_id) {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            let client = &bundle.client;
            debug!(
                "dropping NATS client [{}] and associated subscriptions [{}] for (handler) component [{}]...",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
                &bundle.sub_handles.len(),
                target_id
            );
        }

        debug!(
            "finished processing (handler) link deletion for component [{}]",
            target_id
        );

        Ok(())
    }

    #[instrument(level = "info", skip(self))]
    async fn delete_link_as_target(&self, source_id: &str) -> anyhow::Result<()> {
        let mut links = self.consumer_components.write().await;
        if let Some(bundle) = links.remove(source_id) {
            let client = &bundle.client;
            debug!(
                "dropping NATS client [{}] for (consumer) component [{}]...",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
                source_id
            );
        }

        debug!(
            "finished processing (consumer) link deletion for component [{}]",
            source_id
        );

        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        // clear the handler components
        let mut handlers = self.handler_components.write().await;
        handlers.clear();

        // clear the consumer components
        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        // dropping all connections should send unsubscribes and close the connections, so no need
        // to handle that here
        Ok(())
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
impl exports::wasmcloud::wadm::wadm_client::Handler<Option<Context>> for WadmProvider {
    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn deploy_model(
        &self,
        ctx: Option<Context>,
        model_name: String,
        version: Option<String>,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<DeployResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::deploy_model(&client, lattice, &model_name, version).await {
            Ok(response) => Ok(Ok(response.into())),
            Err(err) => {
                error!("Deployment failed: {err}");
                Ok(Err(format!("Deployment failed: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn undeploy_model(
        &self,
        ctx: Option<Context>,
        model_name: String,
        lattice: Option<String>,
        non_destructive: bool,
    ) -> anyhow::Result<Result<DeployResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::undeploy_model(&client, lattice, &model_name, non_destructive).await {
            Ok(response) => Ok(Ok(response.into())),
            Err(err) => {
                error!("Undeployment failed: {err}");
                Ok(Err(format!("Undeployment failed: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model = %model))]
    async fn put_model(
        &self,
        ctx: Option<Context>,
        model: String,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<PutModelResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::put_model(&client, lattice, &model).await {
            Ok(response) => Ok(Ok(response.into())),
            Err(err) => {
                error!("Failed to store model: {err}");
                Ok(Err(format!("Failed to store model: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(manifest = ?manifest))]
    async fn put_manifest(
        &self,
        ctx: Option<Context>,
        manifest: OamManifest,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<PutModelResponse, String>> {
        let client = self.get_client(ctx).await?;

        // Serialize the OamManifest into bytes
        let manifest_bytes =
            serde_json::to_vec(&manifest).context("Failed to serialize OAM manifest")?;

        // Convert the bytes into a string
        let manifest_string = String::from_utf8(manifest_bytes)
            .context("Failed to convert OAM manifest bytes to string")?;

        match wash_lib::app::put_model(&client, lattice, &manifest_string).await {
            Ok(response) => Ok(Ok(response.into())),
            Err(err) => {
                error!("Failed to store manifest: {err}");
                Ok(Err(format!("Failed to store manifest: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn get_model_history(
        &self,
        ctx: Option<Context>,
        model_name: String,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<VersionResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::get_model_history(&client, lattice, &model_name).await {
            Ok(history) => Ok(Ok(history.into())),
            Err(err) => {
                error!("Failed to retrieve model history: {err}");
                Ok(Err(format!("Failed to retrieve model history: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn get_model_status(
        &self,
        ctx: Option<Context>,
        model_name: String,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<StatusResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::get_model_status(&client, lattice, &model_name).await {
            Ok(status) => Ok(Ok(status.into())),
            Err(err) => {
                error!("Failed to retrieve model status: {err}");
                Ok(Err(format!("Failed to retrieve model status: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn get_model_details(
        &self,
        ctx: Option<Context>,
        model_name: String,
        version: Option<String>,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<GetModelResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::get_model_details(&client, lattice, &model_name, version).await {
            Ok(details) => Ok(Ok(details.into())),
            Err(err) => {
                error!("Failed to retrieve model details: {err}");
                Ok(Err(format!("Failed to retrieve model details: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn delete_model_version(
        &self,
        ctx: Option<Context>,
        model_name: String,
        version: Option<String>,
        delete_all: bool,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<DeleteModelResponse, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::delete_model_version(
            &client,
            lattice,
            &model_name,
            version,
            delete_all,
        )
        .await
        {
            Ok(response) => Ok(Ok(response.into())),
            Err(err) => {
                error!("Failed to delete model version: {err}");
                Ok(Err(format!("Failed to delete model version: {err}")))
            }
        }
    }

    #[instrument(level = "debug", skip(self, ctx))]
    async fn get_models(
        &self,
        ctx: Option<Context>,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<Vec<ModelSummary>, String>> {
        let client = self.get_client(ctx).await?;
        match wash_lib::app::get_models(&client, lattice).await {
            Ok(models) => Ok(Ok(models.into_iter().map(|model| model.into()).collect())),
            Err(err) => {
                error!("Failed to retrieve models: {err}");
                Ok(Err(format!("Failed to retrieve models: {err}")))
            }
        }
    }
}

fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates(&[ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}
