use std::collections::HashSet;
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::{ResolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use async_nats::Subscriber;
use futures::stream::StreamExt;
use opentelemetry::KeyValue;
use tokio::sync::RwLock;
use tracing::{Instrument, debug, instrument, warn};
use wasmtime::error::Context as _;

const PLUGIN_MESSAGING_ID: &str = "wasmcloud-messaging";

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

use bindings::wasmcloud::messaging::consumer::Host;
use bindings::wasmcloud::messaging::types;

use crate::plugin::WorkloadTracker;

pub struct ComponentData {
    subscriptions: Vec<String>,
    cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Clone)]
pub struct NatsMessaging {
    tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    client: Arc<async_nats::Client>,
    meters: Arc<RwLock<Meters>>,
}

impl NatsMessaging {
    pub fn new(client: Arc<async_nats::Client>) -> Self {
        Self {
            client,
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meters: Default::default(),
        }
    }
}

impl<'a> Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(subject = %subject, timeout_ms))]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::BrokerMessage, String>> {
        let Some(plugin) = self.get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID) else {
            return Ok(Err("plugin not available".to_string()));
        };

        let timeout_duration = std::time::Duration::from_millis(timeout_ms as u64);
        let request_future = plugin.client.request(subject, body.into());

        let resp = match tokio::time::timeout(timeout_duration, request_future).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                warn!("failed to send request: {e}");
                return Ok(Err(format!("failed to send request: {e}")));
            }
            Err(_) => {
                warn!("request timed out after {timeout_ms}ms");
                return Ok(Err(format!("request timed out after {timeout_ms}ms")));
            }
        };
        let reply_to = resp.reply.as_ref().map(|r| r.to_string());
        Ok(Ok(types::BrokerMessage {
            subject: resp.subject.to_string(),
            reply_to,
            body: resp.payload.into(),
        }))
    }

    #[instrument(skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> wasmtime::Result<Result<(), String>> {
        let Some(plugin) = self.get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID) else {
            return Ok(Err("plugin not available".to_string()));
        };

        let subject = msg.subject;

        if let Some(reply_to) = msg.reply_to {
            plugin
                .client
                .publish_with_reply(subject, reply_to, msg.body.into())
                .await
                .context("failed to send message")?;
        } else {
            plugin
                .client
                .publish(subject, msg.body.into())
                .await
                .context("failed to send message")?;
        }

        Ok(Ok(()))
    }
}

impl<'a> types::Host for ActiveCtx<'a> {}

#[async_trait::async_trait]
impl HostPlugin for NatsMessaging {
    fn id(&self) -> &'static str {
        PLUGIN_MESSAGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:messaging/consumer,types@0.2.0",
            )]),

            exports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.2.0")]),
        }
    }

    async fn inject_meters(&self, meters: &Meters) {
        *self.meters.write().await = meters.clone();
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(interface) = interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "messaging")
        else {
            return Ok(());
        };

        bindings::wasmcloud::messaging::types::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasmcloud::messaging::consumer::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        if interface.interfaces.iter().any(|i| i == "handler") {
            let raw_subscriptions = match interface.config.get("subscriptions") {
                Some(subs) => subs.split(',').map(|s| s.to_string()).collect(),
                None => vec![],
            };

            let WorkloadItem::Component(component_handle) = component_handle else {
                anyhow::bail!("Service can not be tracked");
            };

            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    subscriptions: raw_subscriptions,
                },
            );
        }

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        let (cancel_token, subjects) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (data.cancel_token.clone(), data.subscriptions.clone()),
                None => return Ok(()),
            }
        };

        if subjects.is_empty() {
            return Ok(());
        }

        let instance_pre = workload.instantiate_pre(component_id).await?;

        let pre = bindings::MessagingPre::new(instance_pre)
            .context("failed to instantiate messaging pre")?;

        let workload = workload.clone();
        let component_id = component_id.to_string();

        let mut subscriptions = Vec::<Subscriber>::new();
        for subject in subjects {
            let sub = match self.client.subscribe(subject.clone()).await {
                Ok(sub) => sub,
                Err(e) => {
                    for sub in subscriptions {
                        drop(sub);
                    }
                    return Err(
                        anyhow::anyhow!(e).context(format!("failed to subscribe to {subject}"))
                    );
                }
            };

            subscriptions.push(sub);
        }

        let mut messages = futures::stream::select_all(subscriptions);
        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_msg = messages.next() => {
                        let msg = match maybe_msg {
                            None => {
                                break;
                            }
                            Some(msg) => {
                                msg
                            }
                        };

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for component {component_id}: {e}");
                                continue;
                            }
                            Ok(s) => s,
                        };
                        let proxy = match pre.instantiate_async(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate component {component_id}: {e}");
                                continue;
                            }
                            Ok(p) => p,
                        };
                        let reply_to = msg.reply.as_ref().map(|r| r.to_string());
                        let msg = types::BrokerMessage {
                            subject: msg.subject.to_string(),
                            reply_to,
                            body: msg.payload.into(),
                        };

                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_wasmcloud_message",
                            subject = %msg.subject,
                            reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"),
                        );

                        let fuel_meter = fuel_meter.clone();

                        tokio::spawn(async move {
                            let result = fuel_meter.observe(
                                &[
                                    KeyValue::new("plugin", PLUGIN_MESSAGING_ID),
                                    KeyValue::new("subject", msg.subject.to_string()),
                                ],
                                &mut store,
                                async move |store| {
                                    proxy
                                        .wasmcloud_messaging_handler()
                                        .call_handle_message(store, &msg)
                                        .instrument(span)
                                        .await
                                        .map_err(Into::into)
                                }
                            ).await;

                            match result {
                                Ok(_) => {
                                    debug!("Message handled successfully");
                                }
                                Err(e) => {
                                    warn!("Error handling message: {e}");
                                }
                            };
                        });
                    }
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let workload_cleanup = |_| async {};
        let component_cleanup = |component_data: ComponentData| async move {
            component_data.cancel_token.cancel();
        };

        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(workload_id, workload_cleanup, component_cleanup)
            .await;

        Ok(())
    }
}
