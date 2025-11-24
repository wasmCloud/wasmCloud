use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bindings::{
    exports::wasmcloud::wadm::client::{self, ModelSummary, OamManifest, Status, VersionInfo},
    wasmcloud::wadm::handler::{self, StatusUpdate},
};
use crate::ext_bindings::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::{anyhow, bail, Context as _};
use async_nats::HeaderMap;
use futures::stream::{AbortHandle, Abortable};
use futures::StreamExt;
use opentelemetry_nats::NatsHeaderInjector;
use tokio::sync::{broadcast, OwnedSemaphorePermit, RwLock, Semaphore};
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument as _;
use wadm_client::{Client, ClientConnectOptions};
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability,
    provider::WrpcClient,
    run_provider, serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};

mod config;

use config::{extract_wadm_config, ClientConfig};

mod bindings {
    wit_bindgen_wrpc::generate!({
        world: "interfaces",
        additional_derives: [
            serde::Serialize,
            serde::Deserialize,
        ],
        with: {
            "wasmcloud:wadm/types@0.2.0": wadm_types::wasmcloud::wadm::types,
            "wasmcloud:wadm/handler@0.2.0": generate,
            "wasmcloud:wadm/client@0.2.0": generate,
        }
    });
}

mod ext_bindings {
    wit_bindgen_wrpc::generate!({
        world: "extension",
        with: {
            "wrpc:extension/types@0.0.1": wasmcloud_provider_sdk::types,
            "wrpc:extension/manageable@0.0.1": generate,
            "wrpc:extension/configurable@0.0.1": generate
        }
    });
}

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

/// Key for identifying a specific link (component_id, link_name)
type LinkKey = (String, String);

#[derive(Clone)]
pub struct WadmProvider {
    default_config: Arc<RwLock<ClientConfig>>,
    handler_components: Arc<RwLock<HashMap<LinkKey, WadmClientBundle>>>,
    consumer_components: Arc<RwLock<HashMap<LinkKey, WadmClientBundle>>>,
    quit_tx: Arc<broadcast::Sender<()>>,
}

impl WadmProvider {
    fn name() -> &'static str {
        "wadm-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(WadmProvider::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = WadmProvider {
            default_config: Arc::default(),
            handler_components: Arc::default(),
            consumer_components: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        };
        let connection = get_connection();
        let (main_client, ext_client) = connection
            .get_wrpc_clients_for_serving()
            .await
            .context("failed to create wRPC clients")?;
        serve_provider_exports(
            &main_client,
            &ext_client,
            provider,
            shutdown,
            bindings::serve,
            ext_bindings::serve,
        )
        .await
        .context("failed to serve provider exports")
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

        let client = Client::new(&cfg.lattice, Some(&cfg.lattice), client_opts).await?;

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
        let ctx = ctx
            .as_ref()
            .ok_or_else(|| anyhow!("no context in request"))?;
        let source_id = ctx
            .component
            .as_ref()
            .ok_or_else(|| anyhow!("no component in request"))?;
        let link_name = ctx.link_name().to_string();
        let link_key = (source_id.clone(), link_name.clone());

        let components = self.consumer_components.read().await;
        let wadm_bundle = match components.get(&link_key) {
            Some(wadm_bundle) => wadm_bundle,
            None => {
                error!("component not linked: {source_id} with link_name: {link_name}");
                bail!("component not linked: {source_id} with link_name: {link_name}")
            }
        };
        Ok(wadm_bundle.client.clone())
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

    if let Err(e) = handler::handle_status_update(wrpc, Some(cx), &update).await {
        error!(
            error = %e,
            "Unable to send message"
        );
    }
}

impl manageable::Handler<Option<Context>> for WadmProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
        let mut handlers = self.handler_components.write().await;
        handlers.clear();

        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for WadmProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let flamegraph_path = config
            .config
            .iter()
            .find(|(k, _)| k == "FLAMEGRAPH_PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROVIDER_WADM_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path, config.config);

        let config_map: HashMap<String, String> = config.config.into_iter().collect();
        let config = ClientConfig::try_from(config_map);
        if let Ok(config) = config {
            *self.default_config.write().await = config;
        }
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id, link_name))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config = extract_wadm_config(&link_config, false)
            .ok_or_else(|| anyhow!("Failed to extract WADM configuration"))?;

        // Merge link-specific config with default config
        let default_config = self.default_config.read().await;
        let merged_config = default_config.merge(&config);

        let mut update_map = self.consumer_components.write().await;
        let bundle = self
            .connect(merged_config, &source_id, false)
            .await
            .context("Failed to connect to NATS")?;

        let link_key = (source_id, link_name);
        update_map.insert(link_key, bundle);

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id, link_name))]
    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        link_name: String,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config = extract_wadm_config(&config, true)
            .ok_or_else(|| anyhow!("Failed to extract WADM configuration"))?;

        // Merge link-specific config with default config
        let default_config = self.default_config.read().await;
        let merged_config = default_config.merge(&config);

        let mut update_map = self.handler_components.write().await;
        let bundle = self
            .connect(merged_config, &target_id, true)
            .await
            .context("Failed to connect to NATS")?;

        let link_key = (target_id, link_name);
        update_map.insert(link_key, bundle);
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id, link_name))]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let link_key = (target_id, link_name);
        self.handler_components.write().await.remove(&link_key);
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id, link_name))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let link_key = (source_id, link_name);
        self.consumer_components.write().await.remove(&link_key);
        Ok(Ok(()))
    }
}

impl client::Handler<Option<Context>> for WadmProvider {
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

        let manifest_bytes =
            serde_json::to_vec(&manifest).context("Failed to serialize OAM manifest")?;

        match client.put_manifest(manifest_bytes).await {
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
