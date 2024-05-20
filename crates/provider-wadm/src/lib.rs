// Stops warning from wrpc
#![recursion_limit = "256"]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument as _;
use wadm_client::{Client, ClientConnectOptions};
use wadm_types::wasmcloud::oam::types::OamManifest;
use wadm_types::wasmcloud::wadm::client::ModelSummary;
use wadm_types::wasmcloud::wadm::client::Status;
use wadm_types::wasmcloud::wadm::types::VersionInfo;
use wasmcloud::messaging::types::BrokerMessage;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, run_provider, Context, LinkConfig, Provider,
};

mod config;
use config::WadmConfig;

wit_bindgen_wrpc::generate!({
    additional_derives: [
        serde::Serialize,
        serde::Deserialize,
    ],
    with: {
        "wasmcloud:wadm/types@0.1.0": wadm_types::wasmcloud::wadm::types,
        "wasmcloud:oam/types@0.1.0": wadm_types::wasmcloud::oam::types,
    }
});

pub async fn run() -> anyhow::Result<()> {
    WadmProvider::run().await
}

struct WadmClientBundle {
    pub client: Client,
    pub sub_handles: Vec<(String, JoinHandle<()>)>,
}

impl Drop for WadmClientBundle {
    fn drop(&mut self) {
        for (_topic, handle) in &self.sub_handles {
            handle.abort();
        }
    }
}

#[derive(Clone)]
pub struct WadmProvider {
    default_config: WadmConfig,
    handler_components: Arc<RwLock<HashMap<String, WadmClientBundle>>>,
    consumer_components: Arc<RwLock<HashMap<String, WadmClientBundle>>>,
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
        let config = WadmConfig::try_from(host_data.config.clone());
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

    /// Attempt to connect to nats url and create a wadm client
    /// If 'make_status_sub' is true, the client will subscribe to
    /// wadm status updates for this component
    async fn connect(
        &self,
        cfg: WadmConfig,
        component_id: &str,
        make_status_sub: bool,
    ) -> anyhow::Result<WadmClientBundle> {
        let ca_path: Option<PathBuf> = cfg.tls_ca_file.as_ref().map(PathBuf::from);
        let client_opts = ClientConnectOptions {
            url: cfg.cluster_uris.first().cloned(),
            seed: cfg.auth_seed.clone(),
            jwt: cfg.auth_jwt.clone(),
            creds_path: None,
            ca_path,
        };

        // Create the Wadm Client from the NATS client using the async function
        let client = Client::new(&cfg.lattice, None, client_opts).await?;

        let mut sub_handles = Vec::new();
        if make_status_sub {
            let join_handle = self
                .handle_status(&client, component_id, &cfg.app_name)
                .await?;
            sub_handles.push(("wadm.status".into(), join_handle));
        }

        Ok(WadmClientBundle {
            client,
            sub_handles,
        })
    }

    /// Add a subscription to status events
    #[instrument(level = "debug", skip(self, client))]
    async fn handle_status(
        &self,
        client: &Client,
        component_id: &str,
        app_name: &str,
    ) -> anyhow::Result<JoinHandle<()>> {
        debug!(?component_id, "spawning listener for component");

        let component_id = Arc::new(component_id.to_string());
        let app_name = Arc::new(app_name.to_string());
        let client = Arc::new(client.clone());

        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run for the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(75));

            // Connect to status API
            match client.subscribe_to_status(&app_name).await {
                Ok(mut message_stream) => {
                    // Listen for NATS message(s)
                    while let Some(msg) = message_stream.recv().await {
                        // Here, dispatch the message based on your logic
                        debug!(?msg, ?component_id, "received message");

                        let span = tracing::debug_span!("handle_message", ?component_id);
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
                }
                Err(err) => {
                    // Handle error - log it
                    eprintln!("Error subscribing to status: {:?}", err);
                }
            }
        });

        Ok(join_handle)
    }

    /// Helper function to get the NATS client from the context
    async fn get_client(&self, ctx: Option<Context>) -> anyhow::Result<Client> {
        if let Some(ref source_id) = ctx
            .as_ref()
            .and_then(|Context { component, .. }| component.clone())
        {
            let actors = self.consumer_components.read().await;
            let wadm_bundle = match actors.get(source_id) {
                Some(wadm_bundle) => wadm_bundle,
                None => {
                    error!("actor not linked: {source_id}");
                    bail!("actor not linked: {source_id}")
                }
            };
            Ok(wadm_bundle.client.clone())
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

impl Provider for WadmProvider {
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
            match WadmConfig::try_from(config.clone()) {
                Ok(cc) => self.default_config.merge(&WadmConfig { ..cc }),
                Err(e) => {
                    error!("Failed to build WADM configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build WADM config"));
                }
            }
        };

        let mut update_map = self.consumer_components.write().await;
        let bundle = match self.connect(config, source_id, false).await {
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
            match WadmConfig::try_from(config.clone()) {
                Ok(cc) => self.default_config.merge(&cc),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build connection config"));
                }
            }
        };

        let mut update_map = self.handler_components.write().await;
        let bundle = match self.connect(config, target_id, true).await {
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
            // Note: subscriptions will be closed via Drop on the WadmClientBundle
            debug!(
                "dropping Wadm client and associated subscriptions [{}] for (handler) component [{}]...",
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
            debug!(
                "dropping Wadm client and associated subscriptions [{}] for (consumer) component [{}]...",
                &bundle.sub_handles.len(),
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

/// Implement the 'wasmcloud:wadm' capability provider interface
impl exports::wasmcloud::wadm::client::Handler<Option<Context>> for WadmProvider {
    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn deploy_model(
        &self,
        ctx: Option<Context>,
        model_name: String,
        version: Option<String>,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<(), String>> {
        let client = self.get_client(ctx).await?;
        match client
            .deploy_manifest(&model_name, version.as_deref())
            .await
        {
            Ok(_) => Ok(Ok(())),
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
    ) -> anyhow::Result<Result<(), String>> {
        let client = self.get_client(ctx).await?;
        match client.undeploy_manifest(&model_name).await {
            Ok(_) => Ok(Ok(())),
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
    ) -> anyhow::Result<Result<(String, String), String>> {
        let client = self.get_client(ctx).await?;
        match client.put_manifest(&model).await {
            Ok(response) => Ok(Ok(response)),
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
    ) -> anyhow::Result<Result<(String, String), String>> {
        let client = self.get_client(ctx).await?;

        // Serialize the OamManifest into bytes
        let manifest_bytes =
            serde_json::to_vec(&manifest).context("Failed to serialize OAM manifest")?;

        // Convert the bytes into a string
        let manifest_string = String::from_utf8(manifest_bytes)
            .context("Failed to convert OAM manifest bytes to string")?;

        match client.put_manifest(&manifest_string).await {
            Ok(response) => Ok(Ok(response)),
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
    ) -> anyhow::Result<Result<Vec<VersionInfo>, String>> {
        let client = self.get_client(ctx).await?;
        match client.list_versions(&model_name).await {
            Ok(history) => {
                // Use map to convert each item in the history list
                let converted_history: Vec<_> =
                    history.into_iter().map(|item| item.into()).collect();
                Ok(Ok(converted_history))
            }
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
    ) -> anyhow::Result<Result<Status, String>> {
        let client = self.get_client(ctx).await?;
        match client.get_manifest_status(&model_name).await {
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
    ) -> anyhow::Result<Result<OamManifest, String>> {
        let client = self.get_client(ctx).await?;
        match client.get_manifest(&model_name, version.as_deref()).await {
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
    ) -> anyhow::Result<Result<bool, String>> {
        let client = self.get_client(ctx).await?;
        match client
            .delete_manifest(&model_name, version.as_deref())
            .await
        {
            Ok(response) => Ok(Ok(response)),
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
        match client.list_manifests().await {
            Ok(models) => Ok(Ok(models.into_iter().map(|model| model.into()).collect())),
            Err(err) => {
                error!("Failed to retrieve models: {err}");
                Ok(Err(format!("Failed to retrieve models: {err}")))
            }
        }
    }
}
