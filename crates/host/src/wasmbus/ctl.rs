//! Implementation of the control interface server for the wasmCloud host runtime
use std::collections::hash_map::{self, Entry};
use std::collections::HashMap;
use std::env;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{bail, ensure, Context as _};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::future::Either;
use futures::stream::SelectAll;
use futures::{join, stream, Stream, StreamExt, TryFutureExt};
use nkeys::{KeyPair, XKey};
use secrecy::Secret;
use serde::Serialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tokio::{process, select, spawn};
use tracing::{debug, error, info, instrument, trace, warn};
use uuid::Uuid;
use wascap::jwt;
use wasmcloud_control_interface::{
    ComponentAuctionAck, ComponentAuctionRequest, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, HostInventory, HostLabel, InterfaceLinkDefinition,
    ProviderAuctionAck, ProviderAuctionRequest, RegistryCredential, ScaleComponentCommand,
    StartProviderCommand, StopHostCommand, StopProviderCommand, UpdateComponentCommand,
};
use wasmcloud_core::{HealthCheckResponse, HostData, OtelConfig, CTL_API_VERSION_1};
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_tracing::context::TraceContextInjector;

use crate::wasmbus::claims::{Claims, StoredClaims};
use crate::wasmbus::config::ConfigBundle;
use crate::wasmbus::handler::Handler;
use crate::wasmbus::{
    component_import_links, event, human_friendly_uptime, injector_to_headers, Annotations,
    Component, ComponentSpecification, Host, Provider,
};
use crate::{fetch_component, PolicyResponse, RegistryConfig};

#[derive(Debug)]
pub(crate) struct Queue {
    all_streams: SelectAll<async_nats::Subscriber>,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.all_streams.poll_next_unpin(cx)
    }
}

impl Queue {
    #[instrument]
    pub(crate) async fn new(
        nats: &async_nats::Client,
        topic_prefix: &str,
        lattice: &str,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let host_id = host_key.public_key();
        let streams = futures::future::join_all([
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.registry.put",
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.ping",
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.*.auction",
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link.*"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link",),
            )),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims.get"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims"),
            )),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.component.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.provider.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.label.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.*.{host_id}"
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config.>"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config"),
            )),
        ])
        .await
        .into_iter()
        .collect::<Result<Vec<_>, async_nats::SubscribeError>>()
        .context("failed to subscribe to queues")?;
        Ok(Self {
            all_streams: futures::stream::select_all(streams),
        })
    }
}

impl Host {
    #[instrument(level = "trace", skip_all, fields(subject = %message.subject))]
    pub(crate) async fn handle_ctl_message(self: Arc<Self>, message: async_nats::Message) {
        // NOTE: if log level is not `trace`, this won't have an effect, since the current span is
        // disabled. In most cases that's fine, since we aren't aware of any control interface
        // requests including a trace context
        opentelemetry_nats::attach_span_context(&message);
        // Skip the topic prefix, the version, and the lattice
        // e.g. `wasmbus.ctl.v1.{prefix}`
        let subject = message.subject;
        let mut parts = subject
            .trim()
            .trim_start_matches(&self.ctl_topic_prefix)
            .trim_start_matches('.')
            .split('.')
            .skip(2);
        trace!(%subject, "handling control interface request");

        // This response is a wrapped Result<Option<Result<Vec<u8>>>> for a good reason.
        // The outer Result is for reporting protocol errors in handling the request, e.g. failing to
        //    deserialize the request payload.
        // The Option is for the case where the request is handled successfully, but the handler
        //    doesn't want to send a response back to the client, like with an auction.
        // The inner Result is purely for the success or failure of serializing the [CtlResponse], which
        //    should never fail but it's a result we must handle.
        // And finally, the Vec<u8> is the serialized [CtlResponse] that we'll send back to the client
        let ctl_response = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            // Component commands
            (Some("component"), Some("auction"), None, None) => self
                .handle_auction_component(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("component"), Some("scale"), Some(host_id), None) => Arc::clone(&self)
                .handle_scale_component(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("component"), Some("update"), Some(host_id), None) => Arc::clone(&self)
                .handle_update_component(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Provider commands
            (Some("provider"), Some("auction"), None, None) => self
                .handle_auction_provider(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("provider"), Some("start"), Some(host_id), None) => Arc::clone(&self)
                .handle_start_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("provider"), Some("stop"), Some(host_id), None) => self
                .handle_stop_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Host commands
            (Some("host"), Some("get"), Some(_host_id), None) => self
                .handle_inventory()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("ping"), None, None) => self
                .handle_ping_hosts(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("stop"), Some(host_id), None) => self
                .handle_stop_host(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Claims commands
            (Some("claims"), Some("get"), None, None) => self
                .handle_claims()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Link commands
            (Some("link"), Some("del"), None, None) => self
                .handle_link_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("link"), Some("get"), None, None) => {
                // Explicitly returning a Vec<u8> for non-cloning efficiency within handle_links
                self.handle_links().await.map(|bytes| Some(Ok(bytes)))
            }
            (Some("link"), Some("put"), None, None) => self
                .handle_link_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Label commands
            (Some("label"), Some("del"), Some(host_id), None) => self
                .handle_label_del(host_id, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("label"), Some("put"), Some(host_id), None) => self
                .handle_label_put(host_id, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Registry commands
            (Some("registry"), Some("put"), None, None) => self
                .handle_registries_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Config commands
            (Some("config"), Some("get"), Some(config_name), None) => self
                .handle_config_get(config_name)
                .await
                .map(|bytes| Some(Ok(bytes))),
            (Some("config"), Some("put"), Some(config_name), None) => self
                .handle_config_put(config_name, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("config"), Some("del"), Some(config_name), None) => self
                .handle_config_delete(config_name)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Topic fallback
            _ => {
                warn!(%subject, "received control interface request on unsupported subject");
                Ok(serialize_ctl_response(Some(CtlResponse::error(
                    "unsupported subject",
                ))))
            }
        };

        if let Err(err) = &ctl_response {
            error!(%subject, ?err, "failed to handle control interface request");
        } else {
            trace!(%subject, "handled control interface request");
        }

        if let Some(reply) = message.reply {
            let headers = injector_to_headers(&TraceContextInjector::default_with_span());

            let payload = match ctl_response {
                Ok(Some(Ok(payload))) => Some(payload.into()),
                // No response from the host (e.g. auctioning provider)
                Ok(None) => None,
                Err(e) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        // This should never fail to serialize, but the fallback ensures that we send
                        // something back to the client even if we somehow fail.
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
                // This would only occur if we failed to serialize a valid CtlResponse. This is
                // programmer error.
                Ok(Some(Err(e))) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
            };

            if let Some(payload) = payload {
                if let Err(err) = self
                    .ctl_nats
                    .publish_with_headers(reply.clone(), headers, payload)
                    .err_into::<anyhow::Error>()
                    .and_then(|()| self.ctl_nats.flush().err_into::<anyhow::Error>())
                    .await
                {
                    error!(%subject, ?err, "failed to publish reply to control interface request");
                }
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_auction_component(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ComponentAuctionAck>>> {
        let ComponentAuctionRequest {
            component_ref,
            component_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component auction command")?;

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
        let component_id_running = self.components.read().await.contains_key(&component_id);

        // This host can run the component if all constraints are satisfied and the component is not already running
        if constraints_satisfied && !component_id_running {
            Ok(Some(CtlResponse::ok(ComponentAuctionAck {
                component_ref,
                component_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>> {
        let ProviderAuctionRequest {
            provider_ref,
            provider_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider auction command")?;

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
        let provider_running = providers.contains_key(&provider_id);
        if constraints_satisfied && !provider_running {
            Ok(Some(CtlResponse::ok(ProviderAuctionAck {
                provider_ref,
                provider_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_stop_host(
        &self,
        payload: impl AsRef<[u8]>,
        transport_host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        // Allow an empty payload to be used for stopping hosts
        let timeout = if payload.as_ref().is_empty() {
            None
        } else {
            let StopHostCommand { timeout, host_id } =
                serde_json::from_slice::<StopHostCommand>(payload.as_ref())
                    .context("failed to deserialize stop command")?;

            // If the Host ID was provided (i..e not the empty string, due to #[serde(default)]), then
            // we should check it against the known transport-provided host_id, and this actual host's ID
            if !host_id.is_empty() {
                anyhow::ensure!(
                    host_id == transport_host_id && host_id == self.host_key.public_key(),
                    "invalid host_id [{host_id}]"
                );
            }
            timeout
        };

        // It *should* be impossible for the transport-derived host ID to not match at this point
        anyhow::ensure!(
            transport_host_id == self.host_key.public_key(),
            "invalid host_id [{transport_host_id}]"
        );

        info!(?timeout, "handling stop host");

        self.heartbeat.abort();
        self.data_watch.abort();
        self.queue.abort();
        self.policy_manager.policy_changes.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(CtlResponse::success())
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_component<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Component>>,
        wasm: Vec<u8>,
        claims: Option<jwt::Claims<jwt::Component>>,
        component_ref: Arc<str>,
        component_id: Arc<str>,
        max_instances: NonZeroUsize,
        annotations: &Annotations,
        config: ConfigBundle,
        secrets: HashMap<String, Secret<SecretValue>>,
    ) -> anyhow::Result<&'a mut Arc<Component>> {
        debug!(?component_ref, ?max_instances, "starting new component");

        if let Some(ref claims) = claims {
            self.store_claims(Claims::Component(claims.clone()))
                .await
                .context("failed to store claims")?;
        }

        let component_spec = self
            .get_component_spec(&component_id)
            .await?
            .unwrap_or_else(|| ComponentSpecification::new(&component_ref));
        self.store_component_spec(&component_id, &component_spec)
            .await?;

        // Map the imports to pull out the result types of the functions for lookup when invoking them
        let handler = Handler {
            nats: Arc::clone(&self.rpc_nats),
            config_data: Arc::new(RwLock::new(config)),
            lattice: Arc::clone(&self.host_config.lattice),
            component_id: Arc::clone(&component_id),
            secrets: Arc::new(RwLock::new(secrets)),
            targets: Arc::default(),
            trace_ctx: Arc::default(),
            instance_links: Arc::new(RwLock::new(component_import_links(&component_spec.links))),
            invocation_timeout: Duration::from_secs(10), // TODO: Make this configurable
        };
        let component = wasmcloud_runtime::Component::new(&self.runtime, &wasm)?;
        let component = self
            .instantiate_component(
                annotations,
                Arc::clone(&component_ref),
                Arc::clone(&component_id),
                max_instances,
                component,
                handler,
            )
            .await
            .context("failed to instantiate component")?;

        info!(?component_ref, "component started");
        self.publish_event(
            "component_scaled",
            event::component_scaled(
                claims.as_ref(),
                annotations,
                self.host_key.public_key(),
                max_instances,
                &component_ref,
                &component_id,
            ),
        )
        .await?;

        Ok(entry.insert(component))
    }

    #[instrument(level = "debug", skip_all)]
    async fn stop_component(&self, component: &Component, _host_id: &str) -> anyhow::Result<()> {
        trace!(component_id = %component.id, "stopping component");

        component.exports.abort();

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn fetch_component(&self, component_ref: &str) -> anyhow::Result<Vec<u8>> {
        let registry_config = self.registry_config.read().await;
        fetch_component(
            component_ref,
            self.host_config.allow_file_load,
            &self.host_config.oci_opts.additional_ca_paths,
            &registry_config,
        )
        .await
        .context("failed to fetch component")
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_scale_component(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let ScaleComponentCommand {
            component_ref,
            component_id,
            annotations,
            max_instances,
            config,
            allow_update,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component scale command")?;

        debug!(
            component_ref,
            max_instances, component_id, "handling scale component"
        );

        let host_id = host_id.to_string();
        let annotations: Annotations = annotations.unwrap_or_default().into_iter().collect();

        // Basic validation to ensure that the component is running and that the image reference matches
        // If it doesn't match, we can still successfully scale, but we won't be updating the image reference
        let (original_ref, ref_changed) = {
            self.components
                .read()
                .await
                .get(&component_id)
                .map(|v| {
                    (
                        Some(Arc::clone(&v.image_reference)),
                        &*v.image_reference != component_ref.as_str(),
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
            let (wasm, claims_token) = match component_and_claims {
                Ok((wasm, Ok(claims_token))) => (wasm, claims_token),
                Err(e) | Ok((_, Err(e))) => {
                    if let Err(e) = self
                        .publish_event(
                            "component_scale_failed",
                            event::component_scale_failed(
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
            };
            // Scale the component
            match self
                .handle_scale_component_task(
                    Arc::clone(&component_ref),
                    Arc::clone(&component_id),
                    &host_id,
                    max_instances,
                    &annotations,
                    config,
                    wasm,
                    claims_token.as_ref(),
                )
                .await
            {
                Ok(event) => {
                    if let Err(e) = self.publish_event("component_scaled", event).await {
                        error!(%component_ref, %component_id, err = ?e, "failed to publish component scaled event");
                    }
                }
                Err(e) => {
                    error!(%component_ref, %component_id, err = ?e, "failed to scale component");
                    if let Err(e) = self
                        .publish_event(
                            "component_scale_failed",
                            event::component_scale_failed(
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

        Ok(CtlResponse {
            success: true,
            message,
            response: None,
        })
    }

    #[instrument(level = "debug", skip_all)]
    /// Handles scaling an component to a supplied number of `max` concurrently executing instances.
    /// Supplying `0` will result in stopping that component instance.
    ///
    /// Should return a serialized event to publish to the events topic, or an error if the scaling failed.
    #[allow(clippy::too_many_arguments)]
    async fn handle_scale_component_task(
        &self,
        component_ref: Arc<str>,
        component_id: Arc<str>,
        host_id: &str,
        max_instances: u32,
        annotations: &Annotations,
        config: Vec<String>,
        wasm: Vec<u8>,
        claims_token: Option<&jwt::Token<jwt::Component>>,
    ) -> anyhow::Result<serde_json::Value> {
        trace!(?component_ref, max_instances, "scale component task");

        let claims = claims_token.map(|c| c.claims.clone());
        match self
            .policy_manager
            .evaluate_start_component(
                &component_id,
                &component_ref,
                max_instances,
                annotations,
                claims.as_ref(),
            )
            .await?
        {
            PolicyResponse {
                permitted: false,
                message: Some(message),
                ..
            } => bail!("Policy denied request to scale component `{component_id}`: `{message:?}`"),
            PolicyResponse {
                permitted: false, ..
            } => bail!("Policy denied request to scale component `{component_id}`"),
            PolicyResponse {
                permitted: true, ..
            } => (),
        };

        match (
            self.components
                .write()
                .await
                .entry(component_id.to_string()),
            NonZeroUsize::new(max_instances as usize),
        ) {
            // No component is running and we requested to scale to zero, noop.
            // We still publish the event to indicate that the component has been scaled to zero
            (hash_map::Entry::Vacant(_), None) => Ok(event::component_scaled(
                claims.as_ref(),
                annotations,
                host_id,
                0_usize,
                &component_ref,
                &component_id,
            )),
            // No component is running and we requested to scale to some amount, start with specified max
            (hash_map::Entry::Vacant(entry), Some(max)) => {
                let (config, secrets) = self
                    .fetch_config_and_secrets(
                        &config,
                        claims_token.as_ref().map(|c| &c.jwt),
                        annotations.get("wasmcloud.dev/appspec"),
                    )
                    .await?;

                self.start_component(
                    entry,
                    wasm,
                    claims.clone(),
                    Arc::clone(&component_ref),
                    Arc::clone(&component_id),
                    max,
                    annotations,
                    config,
                    secrets,
                )
                .await
                .context("failed to start component while processing scale request")?;

                Ok(event::component_scaled(
                    claims.as_ref(),
                    annotations,
                    host_id,
                    max,
                    &component_ref,
                    &component_id,
                ))
            }
            // Component is running and we requested to scale to zero instances, stop component
            (hash_map::Entry::Occupied(entry), None) => {
                let component = entry.remove();
                self.stop_component(&component, host_id)
                    .await
                    .context("failed to stop component in response to scale to zero")?;

                info!(?component_ref, "component stopped");
                Ok(event::component_scaled(
                    claims.as_ref(),
                    &component.annotations,
                    host_id,
                    0_usize,
                    &component.image_reference,
                    &component.id,
                ))
            }
            // Component is running and we requested to scale to some amount or unbounded, scale component
            (hash_map::Entry::Occupied(mut entry), Some(max)) => {
                let component = entry.get_mut();
                let config_changed =
                    &config != component.handler.config_data.read().await.config_names();

                // Create the event first to avoid borrowing the component
                // This event is idempotent.
                let event = event::component_scaled(
                    claims.as_ref(),
                    &component.annotations,
                    host_id,
                    max,
                    &component.image_reference,
                    &component.id,
                );

                // Modify scale only if the requested max differs from the current max or if the configuration has changed
                if component.max_instances != max || config_changed {
                    // We must partially clone the handler as we can't be sharing the targets between components
                    let handler = component.handler.copy_for_new();
                    if config_changed {
                        let (config, secrets) = self
                            .fetch_config_and_secrets(
                                &config,
                                claims_token.as_ref().map(|c| &c.jwt),
                                annotations.get("wasmcloud.dev/appspec"),
                            )
                            .await?;
                        *handler.config_data.write().await = config;
                        *handler.secrets.write().await = secrets;
                    }
                    let instance = self
                        .instantiate_component(
                            annotations,
                            Arc::clone(&component_ref),
                            Arc::clone(&component.id),
                            max,
                            component.component.clone(),
                            handler,
                        )
                        .await
                        .context("failed to instantiate component")?;
                    let component = entry.insert(instance);
                    self.stop_component(&component, host_id)
                        .await
                        .context("failed to stop component after scaling")?;

                    info!(?component_ref, ?max, "component scaled");
                } else {
                    debug!(?component_ref, ?max, "component already at desired scale");
                }
                Ok(event)
            }
        }
    }

    // TODO(#1548): With component IDs, new component references, configuration, etc, we're going to need to do some
    // design thinking around how update component should work. Should it be limited to a single host or latticewide?
    // Should it also update configuration, or is that separate? Should scaling be done via an update?
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_update_component(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let UpdateComponentCommand {
            component_id,
            annotations,
            new_component_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component update command")?;

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
            .get(&component_id)
            .map(|component| Arc::clone(&component.image_reference))
        else {
            return Ok(CtlResponse::error(&format!(
                "component {component_id} not found"
            )));
        };

        // If the component image reference is the same, respond with an appropriate message
        if &*component_ref == new_component_ref.as_str() {
            return Ok(CtlResponse {
                success: true,
                message: format!("component {component_id} already updated to {new_component_ref}"),
                response: None,
            });
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

        Ok(CtlResponse {
            success: true,
            message,
            response: None,
        })
    }

    async fn handle_update_component_task(
        &self,
        component_id: Arc<str>,
        new_component_ref: Arc<str>,
        host_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> anyhow::Result<()> {
        // NOTE: This block is specifically scoped to ensure we drop the read lock on `self.components` before
        // we attempt to grab a write lock.
        let component = {
            let components = self.components.read().await;
            let existing_component = components
                .get(&*component_id)
                .context("component not found")?;
            let annotations = annotations.unwrap_or_default().into_iter().collect();

            // task is a no-op if the component image reference is the same
            if existing_component.image_reference == new_component_ref {
                info!(%component_id, %new_component_ref, "component already updated");
                return Ok(());
            }

            let new_component = self.fetch_component(&new_component_ref).await?;
            let new_component = wasmcloud_runtime::Component::new(&self.runtime, &new_component)
                .context("failed to initialize component")?;
            let new_claims = new_component.claims().cloned();
            if let Some(ref claims) = new_claims {
                self.store_claims(Claims::Component(claims.clone()))
                    .await
                    .context("failed to store claims")?;
            }

            let max = existing_component.max_instances;
            let Ok(component) = self
                .instantiate_component(
                    &annotations,
                    Arc::clone(&new_component_ref),
                    Arc::clone(&component_id),
                    max,
                    new_component,
                    existing_component.handler.copy_for_new(),
                )
                .await
            else {
                bail!("failed to instantiate component from new reference");
            };

            info!(%new_component_ref, "component updated");
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    new_claims.as_ref(),
                    &component.annotations,
                    host_id,
                    max,
                    new_component_ref,
                    &component_id,
                ),
            )
            .await?;

            // TODO(#1548): If this errors, we need to rollback
            self.stop_component(&component, host_id)
                .await
                .context("failed to stop old component")?;
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    component.claims(),
                    &component.annotations,
                    host_id,
                    0_usize,
                    &component.image_reference,
                    &component.id,
                ),
            )
            .await?;

            component
        };

        self.components
            .write()
            .await
            .insert(component_id.to_string(), component);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_start_provider(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StartProviderCommand {
            config,
            provider_id,
            provider_ref,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider start command")?;

        if self.providers.read().await.contains_key(&provider_id) {
            return Ok(CtlResponse::error(
                "provider with that ID is already running",
            ));
        }

        info!(provider_ref, provider_id, "handling start provider"); // Log at info since starting providers can take a while

        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_start_provider_task(
                    &config,
                    &provider_id,
                    &provider_ref,
                    annotations.unwrap_or_default(),
                    &host_id,
                )
                .await
            {
                error!(provider_ref, provider_id, ?err, "failed to start provider");
                if let Err(err) = self
                    .publish_event(
                        "provider_start_failed",
                        event::provider_start_failed(provider_ref, provider_id, &err),
                    )
                    .await
                {
                    error!(?err, "failed to publish provider_start_failed event");
                }
            }
        });
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider_task(
        &self,
        config: &[String],
        provider_id: &str,
        provider_ref: &str,
        annotations: HashMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        trace!(provider_ref, provider_id, "start provider task");

        let registry_config = self.registry_config.read().await;
        let (path, claims_token) = crate::fetch_provider(
            provider_ref,
            host_id,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch provider")?;
        let claims = claims_token.as_ref().map(|t| t.claims.clone());

        if let Some(claims) = claims.clone() {
            self.store_claims(Claims::Provider(claims))
                .await
                .context("failed to store claims")?;
        }

        let annotations: Annotations = annotations.into_iter().collect();

        let PolicyResponse {
            permitted,
            request_id,
            message,
        } = self
            .policy_manager
            .evaluate_start_provider(provider_id, provider_ref, &annotations, claims.as_ref())
            .await?;
        ensure!(
            permitted,
            "policy denied request to start provider `{request_id}`: `{message:?}`",
        );

        let component_specification = self
            .get_component_spec(provider_id)
            .await?
            .unwrap_or_else(|| ComponentSpecification::new(provider_ref));

        self.store_component_spec(&provider_id, &component_specification)
            .await?;

        // TODO(#1648): Implement redelivery of changed configuration when `config.changed()` is true
        let (config, secrets) = self
            .fetch_config_and_secrets(
                config,
                claims_token.as_ref().map(|t| &t.jwt),
                annotations.get("wasmcloud.dev/appspec"),
            )
            .await?;

        let mut providers = self.providers.write().await;
        if let hash_map::Entry::Vacant(entry) = providers.entry(provider_id.into()) {
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
            };

            let provider_xkey = XKey::new();
            // The provider itself needs to know its private key
            let provider_xkey_private_key = if let Ok(seed) = provider_xkey.seed() {
                seed
            } else if self.host_config.secrets_topic_prefix.is_none() {
                "".to_string()
            } else {
                // This should never happen since this returns an error when an Xkey is
                // created from a public key, but if we can't generate one for whatever
                // reason, we should bail.
                bail!("failed to generate seed for provider xkey")
            };
            // We only need to store the public key of the provider xkey, as the private key is only needed by the provider
            let xkey = XKey::from_public_key(&provider_xkey.public_key())
                .context("failed to create XKey from provider public key xkey")?;

            // Prepare startup links by generating the source and target configs. Note that because the provider may be the source
            // or target of a link, we need to iterate over all links to find the ones that involve the provider.
            let all_links = self.links.read().await;
            let provider_links = all_links
                .values()
                .flatten()
                .filter(|link| link.source_id == provider_id || link.target == provider_id);
            let link_definitions = stream::iter(provider_links)
                .filter_map(|link| async {
                    if link.source_id == provider_id || link.target == provider_id {
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
                                    source_id = link.source_id,
                                    target = link.target,
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
                config: config.get_config().await.clone(),
                secrets,
                provider_xkey_private_key,
                host_xkey_public_key: self.secrets_xkey.public_key(),
                cluster_issuers: vec![],
                default_rpc_timeout_ms,
                log_level: Some(self.host_config.log_level.clone()),
                structured_logging: self.host_config.enable_structured_logging,
                otel_config,
            };
            let host_data =
                serde_json::to_vec(&host_data).context("failed to serialize provider data")?;

            trace!("spawn provider process");

            let mut child_cmd = process::Command::new(&path);
            // Prevent the provider from inheriting the host's environment, with the exception of
            // the following variables we manually add back
            child_cmd.env_clear();

            if cfg!(windows) {
                // Proxy SYSTEMROOT to providers. Without this, providers on Windows won't be able to start
                child_cmd.env(
                    "SYSTEMROOT",
                    env::var("SYSTEMROOT")
                        .context("SYSTEMROOT is not set. Providers cannot be started")?,
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
                .write_all(STANDARD.encode(&host_data).as_bytes())
                .await
                .context("failed to write provider data")?;
            stdin
                .write_all(b"\r\n")
                .await
                .context("failed to write newline")?;
            stdin.shutdown().await.context("failed to close stdin")?;

            // TODO: Change method receiver to Arc<Self> and `move` into the closure
            let rpc_nats = self.rpc_nats.clone();
            let ctl_nats = self.ctl_nats.clone();
            let event_builder = self.event_builder.clone();
            // NOTE: health_ prefix here is to allow us to move the variables into the closure
            let health_lattice = self.host_config.lattice.clone();
            let health_host_id = host_id.to_string();
            let health_provider_id = provider_id.to_string();
            let child = spawn(async move {
                // Check the health of the provider every 30 seconds
                let mut health_check = tokio::time::interval(Duration::from_secs(30));
                let mut previous_healthy = false;
                // Allow the provider 5 seconds to initialize
                health_check.reset_after(Duration::from_secs(5));
                let health_topic =
                    format!("wasmbus.rpc.{health_lattice}.{health_provider_id}.health");
                // TODO: Refactor this logic to simplify nesting
                loop {
                    select! {
                        _ = health_check.tick() => {
                            trace!(provider_id=health_provider_id, "performing provider health check");
                            let request = async_nats::Request::new()
                                .payload(Bytes::new())
                                .headers(injector_to_headers(&TraceContextInjector::default_with_span()));
                            if let Ok(async_nats::Message { payload, ..}) = rpc_nats.send_request(
                                health_topic.clone(),
                                request,
                                ).await {
                                    match (serde_json::from_slice::<HealthCheckResponse>(&payload), previous_healthy) {
                                        (Ok(HealthCheckResponse { healthy: true, ..}), false) => {
                                            trace!(provider_id=health_provider_id, "provider health check succeeded");
                                            previous_healthy = true;
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_passed",
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check succeeded event",
                                                );
                                            }
                                        },
                                        (Ok(HealthCheckResponse { healthy: false, ..}), true) => {
                                            trace!(provider_id=health_provider_id, "provider health check failed");
                                            previous_healthy = false;
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_failed",
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check failed event",
                                                );
                                            }
                                        }
                                        // If the provider health status didn't change, we simply publish a health check status event
                                        (Ok(_), _) => {
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_status",
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check status event",
                                                );
                                            }
                                        },
                                        _ => warn!(
                                            provider_id = health_provider_id,
                                            "failed to deserialize provider health check response"
                                        ),
                                    }
                                }
                                else {
                                    warn!(provider_id = health_provider_id, "failed to request provider health, retrying in 30 seconds");
                                }
                        }
                        exit_status = child.wait() => match exit_status {
                            Ok(status) => {
                                debug!("`{}` exited with `{status:?}`", path.display());
                                break;
                            }
                            Err(e) => {
                                warn!("failed to wait for `{}` to execute: {e}", path.display());
                                break;
                            }
                        }
                    }
                }
            });
            info!(provider_ref, provider_id, "provider started");
            self.publish_event(
                "provider_started",
                event::provider_started(
                    claims.as_ref(),
                    &annotations,
                    host_id,
                    provider_ref,
                    provider_id,
                ),
            )
            .await?;
            entry.insert(Provider {
                child,
                annotations,
                claims_token,
                image_ref: provider_ref.to_string(),
                xkey,
            });
        } else {
            bail!("provider is already running with that ID")
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StopProviderCommand { provider_id, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider stop command")?;

        debug!(provider_id, "handling stop provider");

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(entry) = providers.entry(provider_id.clone()) else {
            warn!(
                provider_id,
                "received request to stop provider that is not running"
            );
            return Ok(CtlResponse::error("provider with that ID is not running"));
        };
        let Provider {
            child, annotations, ..
        } = entry.remove();

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
        }
        child.abort();
        info!(provider_id, "provider stopped");
        self.publish_event(
            "provider_stopped",
            event::provider_stopped(&annotations, host_id, provider_id, "stop"),
        )
        .await?;
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_inventory(&self) -> anyhow::Result<CtlResponse<HostInventory>> {
        trace!("handling inventory");
        let inventory = self.inventory().await;
        Ok(CtlResponse::ok(inventory))
    }

    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn handle_claims(
        &self,
    ) -> anyhow::Result<CtlResponse<Vec<HashMap<String, String>>>> {
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
    pub(crate) async fn handle_links(&self) -> anyhow::Result<Vec<u8>> {
        trace!("handling links");

        let links = self.links.read().await;
        let links: Vec<&InterfaceLinkDefinition> = links.values().flatten().collect();
        let res =
            serde_json::to_vec(&CtlResponse::ok(links)).context("failed to serialize response")?;
        Ok(res)
    }

    #[instrument(level = "trace", skip(self))]
    pub(crate) async fn handle_config_get(&self, config_name: &str) -> anyhow::Result<Vec<u8>> {
        trace!(%config_name, "handling get config");
        if let Some(config_bytes) = self.config_data.get(config_name).await? {
            let config_map: HashMap<String, String> = serde_json::from_slice(&config_bytes)
                .context("config data should be a map of string -> string")?;
            serde_json::to_vec(&CtlResponse::ok(config_map)).map_err(anyhow::Error::from)
        } else {
            serde_json::to_vec(&CtlResponse::<()> {
                success: true,
                response: None,
                message: "Configuration not found".to_string(),
            })
            .map_err(anyhow::Error::from)
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_label_put(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, value } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize put label request")?;
        let mut labels = self.labels.write().await;
        match labels.entry(key) {
            Entry::Occupied(mut entry) => {
                info!(key = entry.key(), value, "updated label");
                entry.insert(value);
            }
            Entry::Vacant(entry) => {
                info!(key = entry.key(), value, "set label");
                entry.insert(value);
            }
        }

        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, labels.clone()),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_label_del(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize delete label request")?;
        let mut labels = self.labels.write().await;
        let value = labels.remove(&key);

        if value.is_none() {
            warn!(key, "could not remove unset label");
            return Ok(CtlResponse::success());
        };

        info!(key, "removed label");
        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, labels.clone()),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::success())
    }

    /// Handle a new link by modifying the relevant source [ComponentSpeficication]. Once
    /// the change is written to the LATTICEDATA store, each host in the lattice (including this one)
    /// will handle the new specification and update their own internal link maps via [process_component_spec_put].
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_link_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let interface_link_definition: InterfaceLinkDefinition = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;
        let InterfaceLinkDefinition {
            source_id,
            target,
            wit_namespace,
            wit_package,
            interfaces,
            name,
            source_config: _,
            target_config: _,
        } = interface_link_definition.clone();

        let ns_and_package = format!("{wit_namespace}:{wit_package}");
        debug!(
            source_id,
            target,
            ns_and_package,
            name,
            ?interfaces,
            "handling put wrpc link definition"
        );

        self.validate_config(
            interface_link_definition
                .source_config
                .iter()
                .chain(&interface_link_definition.target_config),
        )
        .await?;

        let mut component_spec = self
            .get_component_spec(&source_id)
            .await?
            .unwrap_or_default();

        // If we can find an existing link with the same source, target, namespace, package, and name, update it.
        // Otherwise, add the new link to the component specification.
        if let Some(existing_link_index) = component_spec.links.iter().position(|link| {
            link.source_id == source_id
                && link.target == target
                && link.wit_namespace == wit_namespace
                && link.wit_package == wit_package
                && link.name == name
        }) {
            if let Some(existing_link) = component_spec.links.get_mut(existing_link_index) {
                *existing_link = interface_link_definition.clone();
            }
        } else {
            component_spec.links.push(interface_link_definition.clone());
        };

        // Update component specification with the new link
        self.store_component_spec(&source_id, &component_spec)
            .await?;

        let set_event = event::linkdef_set(&interface_link_definition);
        self.publish_event("linkdef_set", set_event).await?;

        self.put_backwards_compat_provider_link(&interface_link_definition)
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    pub(crate) async fn handle_link_del(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let DeleteInterfaceLinkDefinitionRequest {
            source_id,
            wit_namespace,
            wit_package,
            name,
        } = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;

        let ns_and_package = format!("{wit_namespace}:{wit_package}");

        debug!(
            source_id,
            ns_and_package, name, "handling del wrpc link definition"
        );

        let Some(mut component_spec) = self.get_component_spec(&source_id).await? else {
            // If the component spec doesn't exist, the link is deleted
            return Ok(CtlResponse::success());
        };

        // If we can find an existing link with the same source, namespace, package, and name, remove it
        // and update the component specification.
        let deleted_link_target = if let Some(existing_link_index) =
            component_spec.links.iter().position(|link| {
                link.source_id == source_id
                    && link.wit_namespace == wit_namespace
                    && link.wit_package == wit_package
                    && link.name == name
            }) {
            // Sanity safety check since `swap_remove` will panic if the index is out of bounds
            if existing_link_index < component_spec.links.len() {
                Some(component_spec.links.swap_remove(existing_link_index).target)
            } else {
                None
            }
        } else {
            None
        };

        // Update component specification with the new link
        self.store_component_spec(&source_id, &component_spec)
            .await?;

        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(&source_id, name, wit_namespace, wit_package),
        )
        .await?;

        self.del_provider_link(&source_id, deleted_link_target, payload.to_owned().into())
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_registries_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let registry_creds: HashMap<String, RegistryCredential> =
            serde_json::from_slice(payload.as_ref())
                .context("failed to deserialize registries put command")?;

        info!(
            registries = ?registry_creds.keys(),
            "updating registry config",
        );

        let mut registry_config = self.registry_config.write().await;
        for (reg, new_creds) in registry_creds {
            let mut new_config = RegistryConfig::from(new_creds);
            match registry_config.entry(reg) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().auth = new_config.auth;
                }
                hash_map::Entry::Vacant(entry) => {
                    new_config.allow_latest = self.host_config.oci_opts.allow_latest;
                    entry.insert(new_config);
                }
            }
        }

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    pub(crate) async fn handle_config_put(
        &self,
        config_name: &str,
        data: Bytes,
    ) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry put");
        // Validate that the data is of the proper type by deserialing it
        serde_json::from_slice::<HashMap<String, String>>(&data)
            .context("config data should be a map of string -> string")?;
        self.config_data
            .put(config_name, data)
            .await
            .context("unable to store config data")?;
        // We don't write it into the cached data and instead let the caching thread handle it as we
        // won't need it immediately.
        self.publish_event("config_set", event::config_set(config_name))
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    pub(crate) async fn handle_config_delete(
        &self,
        config_name: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry deletion");

        self.config_data
            .purge(config_name)
            .await
            .context("Unable to delete config data")?;

        self.publish_event("config_deleted", event::config_deleted(config_name))
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn handle_ping_hosts(
        &self,
        _payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>> {
        trace!("replying to ping");
        let uptime = self.start_at.elapsed();

        Ok(CtlResponse::ok(wasmcloud_control_interface::Host {
            id: self.host_key.public_key(),
            labels: self.labels.read().await.clone(),
            friendly_name: self.friendly_name.clone(),
            uptime_seconds: uptime.as_secs(),
            uptime_human: Some(human_friendly_uptime(uptime)),
            version: Some(self.host_config.version.clone()),
            js_domain: self.host_config.js_domain.clone(),
            ctl_host: Some(self.host_config.ctl_nats_url.to_string()),
            rpc_host: Some(self.host_config.rpc_nats_url.to_string()),
            lattice: self.host_config.lattice.to_string(),
        }))
    }
}

/// Helper function to serialize `CtlResponse`<T> into a Vec<u8> if the response is Some
fn serialize_ctl_response<T: Serialize>(
    ctl_response: Option<CtlResponse<T>>,
) -> Option<anyhow::Result<Vec<u8>>> {
    ctl_response.map(|resp| serde_json::to_vec(&resp).map_err(anyhow::Error::from))
}
