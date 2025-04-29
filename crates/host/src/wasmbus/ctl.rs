use core::sync::atomic::Ordering;

use std::collections::btree_map::Entry as BTreeMapEntry;
use std::collections::{hash_map, HashMap};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use bytes::Bytes;
use futures::join;
use serde_json::json;
use tokio::spawn;
use tokio::time::Instant;
use tracing::{debug, error, info, instrument, trace, warn};
use wasmcloud_control_interface::{
    ComponentAuctionAck, ComponentAuctionRequest, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, HostInventory, HostLabel, HostLabelIdentifier, Link,
    ProviderAuctionAck, ProviderAuctionRequest, RegistryCredential, ScaleComponentCommand,
    StartProviderCommand, StopHostCommand, StopProviderCommand, UpdateComponentCommand,
};
use wasmcloud_tracing::context::TraceContextInjector;

use crate::registry::RegistryCredentialExt;
use crate::wasmbus::{
    human_friendly_uptime, injector_to_headers, Annotations, Claims, Host, Provider, StoredClaims,
};
use crate::ResourceRef;

/// Implementation for the server-side handling of control interface requests.
///
/// This trait is not a part of the `wasmcloud_control_interface` crate yet to allow
/// for the initial implementation to be done in the `wasmcloud_host` (pre 1.0) crate. This
/// will likely move to that crate in the future.
#[async_trait::async_trait]
pub trait ControlInterfaceServer: Send + Sync {
    /// Handle an auction request for a component. This method should return `Ok(None)` if the host
    /// does not want to respond to the auction request.
    async fn handle_auction_component(
        &self,
        request: ComponentAuctionRequest,
    ) -> anyhow::Result<Option<CtlResponse<ComponentAuctionAck>>>;
    /// Handle an auction request for a provider. This method should return `Ok(None)` if the host
    /// does not want to respond to the auction request.
    async fn handle_auction_provider(
        &self,
        request: ProviderAuctionRequest,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>>;

    /// Handle a request to stop the host. This method should return a response indicating success
    /// or failure.
    async fn handle_stop_host(&self, request: StopHostCommand) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to scale a component. This method should return a response indicating success
    /// or failure.
    async fn handle_scale_component(
        self: Arc<Self>,
        request: ScaleComponentCommand,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to update a component. This method should return a response indicating success
    /// or failure.
    async fn handle_update_component(
        self: Arc<Self>,
        request: UpdateComponentCommand,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to start a provider. This method should return a response indicating success
    /// or failure.
    async fn handle_start_provider(
        self: Arc<Self>,
        request: StartProviderCommand,
    ) -> anyhow::Result<Option<CtlResponse<()>>>;

    /// Handle a request to stop a provider. This method should return a response indicating success
    /// or failure.
    async fn handle_stop_provider(
        &self,
        request: StopProviderCommand,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to get the host inventory. This method should return a response containing
    /// the host inventory.
    async fn handle_inventory(&self) -> anyhow::Result<CtlResponse<HostInventory>>;

    /// Handle a request to get the claims for all components and providers. This method should return
    /// a response containing the claims.
    async fn handle_claims(&self) -> anyhow::Result<CtlResponse<Vec<HashMap<String, String>>>>;

    /// Handle a request to get the links for all components. This method should return a response containing
    /// the links.
    async fn handle_links(&self) -> anyhow::Result<Vec<u8>>;

    /// Handle a request to get the configuration for a specific key. This method should return a response
    /// containing the configuration.
    async fn handle_config_get(&self, config_name: &str) -> anyhow::Result<Vec<u8>>;

    /// Handle a request to delete the configuration for a specific key. This method should return a response
    /// indicating success or failure.
    async fn handle_config_delete(&self, config_name: &str) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to put a label on the host. This method should return a response indicating success
    /// or failure.
    async fn handle_label_put(
        &self,
        request: HostLabel,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to delete a label from the host. This method should return a response indicating success
    /// or failure.
    async fn handle_label_del(
        &self,
        request: HostLabelIdentifier,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to put a link on a component. This method should return a response indicating success
    /// or failure.
    async fn handle_link_put(&self, request: Link) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to delete a link from a component. This method should return a response indicating success
    /// or failure.
    async fn handle_link_del(
        &self,
        request: DeleteInterfaceLinkDefinitionRequest,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to put registry credentials. This method should return a response indicating success
    /// or failure.
    async fn handle_registries_put(
        &self,
        request: HashMap<String, RegistryCredential>,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to put configuration data. This method should return a response indicating success
    /// or failure.
    async fn handle_config_put(
        &self,
        config_name: &str,
        data: Bytes,
    ) -> anyhow::Result<CtlResponse<()>>;

    /// Handle a request to ping all hosts in the lattice. This method should return a response containing
    /// the host data.
    async fn handle_ping_hosts(
        &self,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>>;
}

#[async_trait::async_trait]
impl ControlInterfaceServer for Host {
    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_component(
        &self,
        request: ComponentAuctionRequest,
    ) -> anyhow::Result<Option<CtlResponse<ComponentAuctionAck>>> {
        let component_ref = request.component_ref();
        let component_id = request.component_id();
        let constraints = request.constraints();

        info!(
            component_ref,
            component_id,
            ?constraints,
            "handling auction for component"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).is_some_and(|hv| hv == v));
        let component_id_running = self.components.read().await.contains_key(component_id);

        // This host can run the component if all constraints are satisfied and the component is not already running
        if constraints_satisfied && !component_id_running {
            Ok(Some(CtlResponse::ok(
                ComponentAuctionAck::from_component_host_and_constraints(
                    component_ref,
                    component_id,
                    &self.host_key.public_key(),
                    constraints.clone(),
                ),
            )))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_provider(
        &self,
        request: ProviderAuctionRequest,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>> {
        let provider_ref = request.provider_ref();
        let provider_id = request.provider_id();
        let constraints = request.constraints();

        info!(
            provider_ref,
            provider_id,
            ?constraints,
            "handling auction for provider"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).is_some_and(|hv| hv == v));
        let providers = self.providers.read().await;
        let provider_running = providers.contains_key(provider_id);
        if constraints_satisfied && !provider_running {
            Ok(Some(CtlResponse::ok(
                ProviderAuctionAck::builder()
                    .provider_ref(provider_ref.into())
                    .provider_id(provider_id.into())
                    .constraints(constraints.clone())
                    .host_id(self.host_key.public_key())
                    .build()
                    .map_err(|e| anyhow!("failed to build provider auction ack: {e}"))?,
            )))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_host(&self, request: StopHostCommand) -> anyhow::Result<CtlResponse<()>> {
        let timeout = request.timeout();

        info!(?timeout, "handling stop host");

        self.ready.store(false, Ordering::Relaxed);
        self.heartbeat.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);

        Ok(CtlResponse::<()>::success(
            "successfully handled stop host".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_scale_component(
        self: Arc<Self>,
        request: ScaleComponentCommand,
    ) -> anyhow::Result<CtlResponse<()>> {
        let component_ref = request.component_ref();
        let component_id = request.component_id();
        let annotations = request.annotations();
        let max_instances = request.max_instances();
        let config = request.config().clone();
        let allow_update = request.allow_update();
        let host_id = request.host_id();

        debug!(
            component_ref,
            max_instances, component_id, "handling scale component"
        );

        let host_id = host_id.to_string();
        let annotations: Annotations = annotations
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        // Basic validation to ensure that the component is running and that the image reference matches
        // If it doesn't match, we can still successfully scale, but we won't be updating the image reference
        let (original_ref, ref_changed) = {
            self.components
                .read()
                .await
                .get(component_id)
                .map(|v| {
                    (
                        Some(Arc::clone(&v.image_reference)),
                        &*v.image_reference != component_ref,
                    )
                })
                .unwrap_or_else(|| (None, false))
        };

        let mut perform_post_update: bool = false;
        let message = match (allow_update, original_ref, ref_changed) {
            // Updates are not allowed, original ref changed
            (false, Some(original_ref), true) => {
                let msg = format!(
                "Requested to scale existing component to a different image reference: {original_ref} != {component_ref}. The component will be scaled but the image reference will not be updated. If you meant to update this component to a new image ref, use the update command."
            );
                warn!(msg);
                msg
            }
            // Updates are allowed, ref changed and we'll do an update later
            (true, Some(original_ref), true) => {
                perform_post_update = true;
                format!(
                "Requested to scale existing component, with a changed image reference: {original_ref} != {component_ref}. The component will be scaled, and the image reference will be updated afterwards."
            )
            }
            _ => String::with_capacity(0),
        };

        let component_id = Arc::from(component_id);
        let component_ref = Arc::from(component_ref);
        // Spawn a task to perform the scaling and possibly an update of the component afterwards
        spawn(async move {
            // Fetch the component from the reference
            let component_and_claims =
                self.fetch_component(&component_ref)
                    .await
                    .map(|component_bytes| {
                        // Pull the claims token from the component, this returns an error only if claims are embedded
                        // and they are invalid (expired, tampered with, etc)
                        let claims_token =
                            wasmcloud_runtime::component::claims_token(&component_bytes);
                        (component_bytes, claims_token)
                    });
            let (wasm, claims_token, retrieval_error) = match component_and_claims {
                Ok((wasm, Ok(claims_token))) => (Some(wasm), claims_token, None),
                Ok((_, Err(e))) => {
                    if let Err(e) = self
                        .event_publisher
                        .publish_event(
                            "component_scale_failed",
                            crate::event::component_scale_failed(
                                None,
                                &annotations,
                                host_id,
                                &component_ref,
                                &component_id,
                                max_instances,
                                &e,
                            ),
                        )
                        .await
                    {
                        error!(%component_ref, %component_id, err = ?e, "failed to publish component scale failed event");
                    }
                    return;
                }
                Err(e) => (None, None, Some(e)),
            };
            // Scale the component
            if let Err(e) = self
                .handle_scale_component_task(
                    Arc::clone(&component_ref),
                    Arc::clone(&component_id),
                    &host_id,
                    max_instances,
                    &annotations,
                    config,
                    wasm.ok_or_else(|| {
                        retrieval_error.unwrap_or_else(|| anyhow!("unexpected missing wasm binary"))
                    }),
                    claims_token.as_ref(),
                )
                .await
            {
                error!(%component_ref, %component_id, err = ?e, "failed to scale component");
                if let Err(e) = self
                    .event_publisher
                    .publish_event(
                        "component_scale_failed",
                        crate::event::component_scale_failed(
                            claims_token.map(|c| c.claims).as_ref(),
                            &annotations,
                            host_id,
                            &component_ref,
                            &component_id,
                            max_instances,
                            &e,
                        ),
                    )
                    .await
                {
                    error!(%component_ref, %component_id, err = ?e, "failed to publish component scale failed event");
                }
                return;
            }

            if perform_post_update {
                if let Err(e) = self
                    .handle_update_component_task(
                        Arc::clone(&component_id),
                        Arc::clone(&component_ref),
                        &host_id,
                        None,
                    )
                    .await
                {
                    error!(%component_ref, %component_id, err = ?e, "failed to update component after scale");
                }
            }
        });

        Ok(CtlResponse::<()>::success(message))
    }

    // TODO(#1548): With component IDs, new component references, configuration, etc, we're going to need to do some
    // design thinking around how update component should work. Should it be limited to a single host or latticewide?
    // Should it also update configuration, or is that separate? Should scaling be done via an update?
    #[instrument(level = "debug", skip_all)]
    async fn handle_update_component(
        self: Arc<Self>,
        request: UpdateComponentCommand,
    ) -> anyhow::Result<CtlResponse<()>> {
        let component_id = request.component_id();
        let annotations = request.annotations().cloned();
        let new_component_ref = request.new_component_ref();
        let host_id = request.host_id();

        debug!(
            component_id,
            new_component_ref,
            ?annotations,
            "handling update component"
        );

        // Find the component and extract the image reference
        #[allow(clippy::map_clone)]
        // NOTE: clippy thinks, that we can just replace the `.map` below by
        // `.cloned` - we can't, because we need to clone the field
        let Some(component_ref) = self
            .components
            .read()
            .await
            .get(component_id)
            .map(|component| Arc::clone(&component.image_reference))
        else {
            return Ok(CtlResponse::error(&format!(
                "component {component_id} not found"
            )));
        };

        // If the component image reference is the same, respond with an appropriate message
        if &*component_ref == new_component_ref {
            return Ok(CtlResponse::<()>::success(format!(
                "component {component_id} already updated to {new_component_ref}"
            )));
        }

        let host_id = host_id.to_string();
        let message = format!(
            "component {component_id} updating from {component_ref} to {new_component_ref}"
        );
        let component_id = Arc::from(component_id);
        let new_component_ref = Arc::from(new_component_ref);
        spawn(async move {
            if let Err(e) = self
                .handle_update_component_task(
                    Arc::clone(&component_id),
                    Arc::clone(&new_component_ref),
                    &host_id,
                    annotations,
                )
                .await
            {
                error!(%new_component_ref, %component_id, err = ?e, "failed to update component");
            }
        });

        Ok(CtlResponse::<()>::success(message))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider(
        self: Arc<Self>,
        request: StartProviderCommand,
    ) -> anyhow::Result<Option<CtlResponse<()>>> {
        if self
            .providers
            .read()
            .await
            .contains_key(request.provider_id())
        {
            return Ok(Some(CtlResponse::error(
                "provider with that ID is already running",
            )));
        }

        // Avoid responding to start providers for builtin providers if they're not enabled
        if let Ok(ResourceRef::Builtin(name)) = ResourceRef::try_from(request.provider_ref()) {
            if !self.experimental_features.builtin_http_server && name == "http-server" {
                debug!(
                    provider_ref = request.provider_ref(),
                    provider_id = request.provider_id(),
                    "skipping start provider for disabled builtin http provider"
                );
                return Ok(None);
            }
            if !self.experimental_features.builtin_messaging_nats && name == "messaging-nats" {
                debug!(
                    provider_ref = request.provider_ref(),
                    provider_id = request.provider_id(),
                    "skipping start provider for disabled builtin messaging provider"
                );
                return Ok(None);
            }
        }

        // NOTE: We log at info since starting providers can take a while
        info!(
            provider_ref = request.provider_ref(),
            provider_id = request.provider_id(),
            "handling start provider"
        );

        let host_id = request.host_id().to_string();
        spawn(async move {
            let config = request.config();
            let provider_id = request.provider_id();
            let provider_ref = request.provider_ref();
            let annotations = request.annotations();

            if let Err(err) = Arc::clone(&self)
                .handle_start_provider_task(
                    config,
                    provider_id,
                    provider_ref,
                    annotations.cloned().unwrap_or_default(),
                    &host_id,
                )
                .await
            {
                error!(provider_ref, provider_id, ?err, "failed to start provider");
                if let Err(err) = self
                    .event_publisher
                    .publish_event(
                        "provider_start_failed",
                        crate::event::provider_start_failed(
                            provider_ref,
                            provider_id,
                            host_id,
                            &err,
                        ),
                    )
                    .await
                {
                    error!(?err, "failed to publish provider_start_failed event");
                }
            }
        });

        Ok(Some(CtlResponse::<()>::success(
            "successfully started provider".into(),
        )))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_provider(
        &self,
        request: StopProviderCommand,
    ) -> anyhow::Result<CtlResponse<()>> {
        let provider_id = request.provider_id();
        let host_id = request.host_id();

        debug!(provider_id, "handling stop provider");

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(entry) = providers.entry(provider_id.into()) else {
            warn!(
                provider_id,
                "received request to stop provider that is not running"
            );
            return Ok(CtlResponse::error("provider with that ID is not running"));
        };
        let Provider {
            ref annotations,
            mut tasks,
            shutdown,
            ..
        } = entry.remove();

        // Set the shutdown flag to true to stop health checks and config updates. Also
        // prevents restarting the provider but does not stop the provider process.
        shutdown.store(true, Ordering::Relaxed);

        // Send a request to the provider, requesting a graceful shutdown
        let req = serde_json::to_vec(&json!({ "host_id": host_id }))
            .context("failed to encode provider stop request")?;
        let req = async_nats::Request::new()
            .payload(req.into())
            .timeout(self.host_config.provider_shutdown_delay)
            .headers(injector_to_headers(
                &TraceContextInjector::default_with_span(),
            ));
        if let Err(e) = self
            .rpc_nats
            .send_request(
                format!(
                    "wasmbus.rpc.{}.{provider_id}.default.shutdown",
                    self.host_config.lattice
                ),
                req,
            )
            .await
        {
            warn!(
                ?e,
                provider_id,
                "provider did not gracefully shut down in time, shutting down forcefully"
            );
            // NOTE: The provider child process is spawned with [tokio::process::Command::kill_on_drop],
            // so dropping the task will send a SIGKILL to the provider process.
        }

        // Stop the provider and health check / config changes tasks
        tasks.abort_all();

        info!(provider_id, "provider stopped");
        self.event_publisher
            .publish_event(
                "provider_stopped",
                crate::event::provider_stopped(annotations, host_id, provider_id, "stop"),
            )
            .await?;
        Ok(CtlResponse::<()>::success(
            "successfully stopped provider".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_inventory(&self) -> anyhow::Result<CtlResponse<HostInventory>> {
        trace!("handling inventory");
        let inventory = self.inventory().await;
        Ok(CtlResponse::ok(inventory))
    }

    #[instrument(level = "trace", skip_all)]
    async fn handle_claims(&self) -> anyhow::Result<CtlResponse<Vec<HashMap<String, String>>>> {
        trace!("handling claims");

        let (component_claims, provider_claims) =
            join!(self.component_claims.read(), self.provider_claims.read());
        let component_claims = component_claims.values().cloned().map(Claims::Component);
        let provider_claims = provider_claims.values().cloned().map(Claims::Provider);
        let claims: Vec<StoredClaims> = component_claims
            .chain(provider_claims)
            .flat_map(TryFrom::try_from)
            .collect();

        Ok(CtlResponse::ok(
            claims.into_iter().map(std::convert::Into::into).collect(),
        ))
    }

    #[instrument(level = "trace", skip_all)]
    // TODO: Vec<&Link> return?
    async fn handle_links(&self) -> anyhow::Result<Vec<u8>> {
        trace!("handling links");

        let links = self.links.read().await;
        let links: Vec<&Link> = links.values().flatten().collect();
        let res =
            serde_json::to_vec(&CtlResponse::ok(links)).context("failed to serialize response")?;
        Ok(res)
    }

    #[instrument(level = "trace", skip(self))]
    async fn handle_config_get(&self, config_name: &str) -> anyhow::Result<Vec<u8>> {
        trace!(%config_name, "handling get config");
        if let Some(config_bytes) = self.config_store.get(config_name).await? {
            let config_map: HashMap<String, String> = serde_json::from_slice(&config_bytes)
                .context("config data should be a map of string -> string")?;
            serde_json::to_vec(&CtlResponse::ok(config_map)).map_err(anyhow::Error::from)
        } else {
            serde_json::to_vec(&CtlResponse::<()>::success(
                "Configuration not found".into(),
            ))
            .map_err(anyhow::Error::from)
        }
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_delete(&self, config_name: &str) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry deletion");

        self.config_store
            .del(config_name)
            .await
            .context("Unable to delete config data")?;

        self.event_publisher
            .publish_event("config_deleted", crate::event::config_deleted(config_name))
            .await?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted config".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_put(
        &self,
        request: HostLabel,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let key = request.key();
        if key.to_lowercase().starts_with("hostcore.") {
            bail!("hostcore.* labels cannot be set dynamically");
        }

        let value = request.value();
        let mut labels = self.labels.write().await;
        match labels.entry(key.into()) {
            BTreeMapEntry::Occupied(mut entry) => {
                info!(key = entry.key(), value, "updated label");
                entry.insert(value.into());
            }
            BTreeMapEntry::Vacant(entry) => {
                info!(key = entry.key(), value, "set label");
                entry.insert(value.into());
            }
        }

        self.event_publisher
            .publish_event(
                "labels_changed",
                crate::event::labels_changed(host_id, HashMap::from_iter(labels.clone())),
            )
            .await
            .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::<()>::success("successfully put label".into()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_del(
        &self,
        request: HostLabelIdentifier,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let key = request.key();
        let mut labels = self.labels.write().await;
        let value = labels.remove(key);

        if value.is_none() {
            warn!(key, "could not remove unset label");
            return Ok(CtlResponse::<()>::success(
                "successfully deleted label (no such label)".into(),
            ));
        };

        info!(key, "removed label");
        self.event_publisher
            .publish_event(
                "labels_changed",
                crate::event::labels_changed(host_id, HashMap::from_iter(labels.clone())),
            )
            .await
            .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted label".into(),
        ))
    }

    /// Handle a new link by modifying the relevant source [ComponentSpecification]. Once
    /// the change is written to the LATTICEDATA store, each host in the lattice (including this one)
    /// will handle the new specification and update their own internal link maps via [process_component_spec_put].
    #[instrument(level = "debug", skip_all)]
    async fn handle_link_put(&self, request: Link) -> anyhow::Result<CtlResponse<()>> {
        let link_set_result: anyhow::Result<()> = async {
            let source_id = request.source_id();
            let target = request.target();
            let wit_namespace = request.wit_namespace();
            let wit_package = request.wit_package();
            let interfaces = request.interfaces();
            let name = request.name();

            let ns_and_package = format!("{wit_namespace}:{wit_package}");
            debug!(
                source_id,
                target,
                ns_and_package,
                name,
                ?interfaces,
                "handling put wrpc link definition"
            );

            // Validate all configurations
            self.validate_config(
                request
                    .source_config()
                    .clone()
                    .iter()
                    .chain(request.target_config())
            ).await?;

            let mut component_spec = self
                .get_component_spec(source_id)
                .await?
                .unwrap_or_default();

            // If the link is defined from this source on the same interface and link name, but to a different target,
            // we need to reject this link and suggest deleting the existing link or using a different link name.
            if let Some(existing_conflict_link) = component_spec.links.iter().find(|link| {
                link.source_id() == source_id
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == name
                    // Check if interfaces have no intersection
                    && link.interfaces().iter().any(|i| interfaces.contains(i))
                    && link.target() != target
            }) {
                error!(
                    source_id,
                    desired_target = target,
                    existing_target = existing_conflict_link.target(),
                    ns_and_package,
                    name,
                    "link already exists with different target, consider deleting the existing link or using a different link name"
                );
                bail!("link already exists with different target, consider deleting the existing link or using a different link name");
            }

            // If we can find an existing link with the same source, target, namespace, package, and name, update it.
            // Otherwise, add the new link to the component specification.
            if let Some(existing_link_index) = component_spec.links.iter().position(|link| {
                link.source_id() == source_id
                    && link.target() == target
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == name
            }) {
                if let Some(existing_link) = component_spec.links.get_mut(existing_link_index) {
                    *existing_link = request.clone();
                }
            } else {
                component_spec.links.push(request.clone());
            };

            // Update component specification with the new link
            self.store_component_spec(&source_id, &component_spec)
                .await?;
            self.update_host_with_spec(&source_id, &component_spec)
                .await?;

            self.put_backwards_compat_provider_link(&request)
                .await?;

            Ok(())
        }
        .await;

        if let Err(e) = link_set_result {
            self.event_publisher
                .publish_event(
                    "linkdef_set_failed",
                    crate::event::linkdef_set_failed(&request, &e),
                )
                .await?;
            Ok(CtlResponse::error(e.to_string().as_ref()))
        } else {
            self.event_publisher
                .publish_event("linkdef_set", crate::event::linkdef_set(&request))
                .await?;
            Ok(CtlResponse::<()>::success("successfully set link".into()))
        }
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    async fn handle_link_del(
        &self,
        request: DeleteInterfaceLinkDefinitionRequest,
    ) -> anyhow::Result<CtlResponse<()>> {
        let source_id = request.source_id();
        let wit_namespace = request.wit_namespace();
        let wit_package = request.wit_package();
        let link_name = request.link_name();

        let ns_and_package = format!("{wit_namespace}:{wit_package}");

        debug!(
            source_id,
            ns_and_package, link_name, "handling del wrpc link definition"
        );

        let Some(mut component_spec) = self.get_component_spec(source_id).await? else {
            // If the component spec doesn't exist, the link is deleted
            return Ok(CtlResponse::<()>::success(
                "successfully deleted link (spec doesn't exist)".into(),
            ));
        };

        // If we can find an existing link with the same source, namespace, package, and name, remove it
        // and update the component specification.
        let deleted_link = if let Some(existing_link_index) =
            component_spec.links.iter().position(|link| {
                link.source_id() == source_id
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == link_name
            }) {
            // Sanity safety check since `swap_remove` will panic if the index is out of bounds
            if existing_link_index < component_spec.links.len() {
                Some(component_spec.links.swap_remove(existing_link_index))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(link) = deleted_link.as_ref() {
            // Update component specification with the deleted link
            self.store_component_spec(&source_id, &component_spec)
                .await?;
            self.update_host_with_spec(&source_id, &component_spec)
                .await?;

            // Send the link to providers for deletion
            self.del_provider_link(link).await?;
        }

        // For idempotency, we always publish the deleted event, even if the link didn't exist
        let deleted_link_target = deleted_link
            .as_ref()
            .map(|link| String::from(link.target()));
        self.event_publisher
            .publish_event(
                "linkdef_deleted",
                crate::event::linkdef_deleted(
                    source_id,
                    deleted_link_target.as_ref(),
                    link_name,
                    wit_namespace,
                    wit_package,
                    deleted_link.as_ref().map(|link| link.interfaces()),
                ),
            )
            .await?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted link".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_registries_put(
        &self,
        request: HashMap<String, RegistryCredential>,
    ) -> anyhow::Result<CtlResponse<()>> {
        info!(
            registries = ?request.keys(),
            "updating registry config",
        );

        let mut registry_config = self.registry_config.write().await;
        for (reg, new_creds) in request {
            let mut new_config = new_creds.into_registry_config()?;
            match registry_config.entry(reg) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().set_auth(new_config.auth().clone());
                }
                hash_map::Entry::Vacant(entry) => {
                    new_config.set_allow_latest(self.host_config.oci_opts.allow_latest);
                    entry.insert(new_config);
                }
            }
        }

        Ok(CtlResponse::<()>::success(
            "successfully put registries".into(),
        ))
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_put(
        &self,
        config_name: &str,
        data: Bytes,
    ) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry put");
        // Validate that the data is of the proper type by deserialing it
        serde_json::from_slice::<HashMap<String, String>>(&data)
            .context("config data should be a map of string -> string")?;
        self.config_store
            .put(config_name, data)
            .await
            .context("unable to store config data")?;
        // We don't write it into the cached data and instead let the caching thread handle it as we
        // won't need it immediately.
        self.event_publisher
            .publish_event("config_set", crate::event::config_set(config_name))
            .await?;

        Ok(CtlResponse::<()>::success("successfully put config".into()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_ping_hosts(
        &self,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>> {
        trace!("replying to ping");
        let uptime = self.start_at.elapsed();

        let mut host = wasmcloud_control_interface::Host::builder()
            .id(self.host_key.public_key())
            .labels(self.labels.read().await.clone())
            .friendly_name(self.friendly_name.clone())
            .uptime_seconds(uptime.as_secs())
            .uptime_human(human_friendly_uptime(uptime))
            .version(self.host_config.version.clone())
            // TODO(brooksmtownsend): how get this value? Why does it matter?
            // .ctl_host(self.host_config.ctl_nats_url.to_string())
            .rpc_host(self.host_config.rpc_nats_url.to_string())
            .lattice(self.host_config.lattice.to_string());

        if let Some(ref js_domain) = self.host_config.js_domain {
            host = host.js_domain(js_domain.clone());
        }

        let host = host
            .build()
            .map_err(|e| anyhow!("failed to build host message: {e}"))?;

        Ok(CtlResponse::ok(host))
    }
}
