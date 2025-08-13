use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_nats::HeaderMap;
use futures::stream::{AbortHandle, Abortable};
use futures::StreamExt;
use opentelemetry_nats::NatsHeaderInjector;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument as _;
use wadm_client::{Client, ClientConnectOptions};
use wadm_types::wasmcloud::wadm::handler::StatusUpdate;
use wasmcloud_provider_sdk::{
    core::HostData, get_connection, load_host_data, provider::WrpcClient, run_provider, Context,
    LinkConfig, Provider,
};
use wasmcloud_provider_sdk::{initialize_observability, serve_provider_exports, LinkDeleteInfo};

use crate::exports::wasmcloud::wadm::client::{ModelSummary, OamManifest, Status, VersionInfo};

mod config;

use config::{extract_wadm_config, ClientConfig};

wit_bindgen_wrpc::generate!({
    additional_derives: [
        serde::Serialize,
        serde::Deserialize,
    ],
    with: {
        "wasmcloud:wadm/types@0.2.0": wadm_types::wasmcloud::wadm::types,
        "wasmcloud:wadm/handler@0.2.0": generate,
        "wasmcloud:wadm/client@0.2.0": generate
    }
});

pub async fn run() -> anyhow::Result<()> {
    WadmProvider::run().await
}

struct WadmClientBundle {
    pub client: Client,
    pub sub_handles: Vec<(String, AbortHandle)>,
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
    default_config: ClientConfig,
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
    fn name() -> &'static str {
        "wadm-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            WadmProvider::name(),
            std::env::var_os("PROVIDER_SQLDB_POSTGRES_FLAMEGRAPH_PATH")
        );

        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), WadmProvider::name())
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
        serve_provider_exports(&wrpc, provider, shutdown, serve)
            .await
            .context("failed to serve provider exports")
    }

    /// Build a [`WadmProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> WadmProvider {
        let config = ClientConfig::try_from(host_data.config.clone());
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
        cfg: ClientConfig,
        component_id: &str,
        make_status_sub: bool,
    ) -> anyhow::Result<WadmClientBundle> {
        let ca_path: Option<PathBuf> = cfg.ctl_tls_ca_file.as_ref().map(PathBuf::from);

        let url = format!("{}:{}", cfg.ctl_host, cfg.ctl_port);
        let client_opts = ClientConnectOptions {
            url: Some(url),
            seed: cfg.ctl_seed,
            jwt: cfg.ctl_jwt,
            creds_path: cfg.ctl_credsfile.as_ref().map(PathBuf::from),
            ca_path,
        };

        let client = Client::new(&cfg.lattice, None, client_opts).await?;

        let mut sub_handles = Vec::new();
        if make_status_sub {
            if let Some(app_name) = &cfg.app_name {
                let handle = self.handle_status(&client, component_id, app_name).await?;
                sub_handles.push(("wadm.status".into(), handle));
            } else {
                bail!("app_name is required for status subscription");
            }
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
    ) -> anyhow::Result<AbortHandle> {
        debug!(
            ?component_id,
            ?app_name,
            "spawning listener for component and app"
        );

        let mut subscriber = client
            .subscribe_to_status(app_name)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to subscribe to status: {}", e))?;

        let component_id = Arc::new(component_id.to_string());
        let app_name = Arc::new(app_name.to_string());

        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        tokio::task::spawn(Abortable::new(
            {
                let semaphore = Arc::new(Semaphore::new(75));
                let wrpc = match get_connection().get_wrpc_client(&component_id).await {
                    Ok(wrpc) => Arc::new(wrpc),
                    Err(err) => {
                        error!(?err, "failed to construct wRPC client");
                        return Err(anyhow!("Failed to construct wRPC client: {:?}", err));
                    }
                };
                async move {
                    // Listen for NATS message(s)
                    while let Some(msg) = subscriber.next().await {
                        // Parse the message into a StatusResponse
                        match serde_json::from_slice::<wadm_types::api::Status>(&msg.payload) {
                            Ok(status) => {
                                debug!(?status, ?component_id, "received status");

                                let span = tracing::debug_span!("handle_message", ?component_id);
                                let permit = match semaphore.clone().acquire_owned().await {
                                    Ok(p) => p,
                                    Err(_) => {
                                        warn!("Work pool has been closed, exiting queue subscribe");
                                        break;
                                    }
                                };

                                let component_id = Arc::clone(&component_id);
                                let wrpc = Arc::clone(&wrpc);
                                let app_name = Arc::clone(&app_name);
                                tokio::spawn(async move {
                                    dispatch_status_update(
                                        &wrpc,
                                        component_id.as_str(),
                                        &app_name,
                                        status.into(),
                                        permit,
                                    )
                                    .instrument(span)
                                    .await;
                                });
                            }
                            Err(e) => {
                                warn!("Failed to deserialize message: {}", e);
                            }
                        };
                    }
                }
            },
            abort_registration,
        ));

        Ok(abort_handle)
    }

    /// Helper function to get the NATS client from the context
    async fn get_client(&self, ctx: Option<Context>) -> anyhow::Result<Client> {
        if let Some(ref source_id) = ctx
            .as_ref()
            .and_then(|Context { component, .. }| component.clone())
        {
            let components = self.consumer_components.read().await;
            let wadm_bundle = match components.get(source_id) {
                Some(wadm_bundle) => wadm_bundle,
                None => {
                    error!("component not linked: {source_id}");
                    bail!("component not linked: {source_id}")
                }
            };
            Ok(wadm_bundle.client.clone())
        } else {
            error!("no component in request");
            bail!("no component in request")
        }
    }
}

#[instrument(level = "debug", skip_all, fields(component_id = %component_id, app_name = %app))]
async fn dispatch_status_update(
    wrpc: &WrpcClient,
    component_id: &str,
    app: &str,
    status: Status,
    _permit: OwnedSemaphorePermit,
) {
    let update = StatusUpdate {
        app: app.to_string(),
        status,
    };
    debug!(
        app = app,
        component_id = component_id,
        "sending status to component",
    );

    let cx: HeaderMap = NatsHeaderInjector::default_with_span().into();

    if let Err(e) = wasmcloud::wadm::handler::handle_status_update(wrpc, Some(cx), &update).await {
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
        link_config @ LinkConfig { source_id, .. }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = extract_wadm_config(&link_config, false)
            .ok_or_else(|| anyhow!("Failed to extract WADM configuration"))?;

        let merged_config = self.default_config.merge(&config);

        let mut update_map = self.consumer_components.write().await;
        let bundle = self
            .connect(merged_config, source_id, false)
            .await
            .context("Failed to connect to NATS")?;

        update_map.insert(source_id.into(), bundle);
        Ok(())
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn receive_link_config_as_source(
        &self,
        link_config @ LinkConfig { target_id, .. }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = extract_wadm_config(&link_config, true)
            .ok_or_else(|| anyhow!("Failed to extract WADM configuration"))?;

        let merged_config = self.default_config.merge(&config);

        let mut update_map = self.handler_components.write().await;
        let bundle = self
            .connect(merged_config, target_id, true)
            .await
            .context("Failed to connect to NATS")?;

        update_map.insert(target_id.into(), bundle);
        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        self.handler_components.write().await.remove(component_id);
        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        self.consumer_components.write().await.remove(component_id);
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut handlers = self.handler_components.write().await;
        handlers.clear();

        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        // dropping all connections should send unsubscribes and close the connections, so no need
        // to handle that here
        Ok(())
    }
}

impl exports::wasmcloud::wadm::client::Handler<Option<Context>> for WadmProvider {
    #[instrument(level = "debug", skip(self, ctx), fields(model_name = %model_name))]
    async fn deploy_model(
        &self,
        ctx: Option<Context>,
        model_name: String,
        version: Option<String>,
        lattice: Option<String>,
    ) -> anyhow::Result<Result<String, String>> {
        let client = self.get_client(ctx).await?;
        match client
            .deploy_manifest(&model_name, version.as_deref())
            .await
        {
            Ok((name, _version)) => Ok(Ok(name)),
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

        let manifest = wadm_types::Manifest::from(manifest);

        match client.put_manifest(manifest).await {
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
