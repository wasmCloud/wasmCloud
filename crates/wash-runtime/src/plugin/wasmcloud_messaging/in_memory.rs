use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use anyhow::Context;
use opentelemetry::KeyValue;
use tokio::sync::{Notify, RwLock, oneshot};
use tracing::{Instrument, debug, instrument, warn};

const PLUGIN_MESSAGING_MEMORY_ID: &str = "wasmcloud-messaging-memory";
const MAX_QUEUE_SIZE: usize = 10000;

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

/// Per-workload tracking data
struct WorkloadData {
    pending_messages: Arc<RwLock<VecDeque<types::BrokerMessage>>>,
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<types::BrokerMessage>>>>,
    notify: Arc<Notify>,
}

impl Default for WorkloadData {
    fn default() -> Self {
        Self {
            pending_messages: Arc::default(),
            pending_requests: Arc::default(),
            notify: Arc::new(Notify::new()),
        }
    }
}

/// Per-component tracking data
struct ComponentData {
    cancel_token: tokio_util::sync::CancellationToken,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

/// In-memory messaging plugin for wash dev and mocking scenarios.
///
/// Messages published by a workload are only handled within that same workload
/// (per-workload isolation). This is useful for testing and development where
/// a full NATS server is not needed.
#[derive(Clone)]
pub struct InMemoryMessaging {
    tracker: Arc<RwLock<WorkloadTracker<WorkloadData, ComponentData>>>,
    meters: Arc<RwLock<Meters>>,
}

impl InMemoryMessaging {
    pub fn new() -> Self {
        Self {
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meters: Default::default(),
        }
    }
}

impl Default for InMemoryMessaging {
    fn default() -> Self {
        Self::new()
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
        let Some(plugin) = self.get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID) else {
            return Ok(Err("plugin not available".to_string()));
        };

        let workload_id = self.ctx.workload_id.to_string();

        let (pending_messages, pending_requests, notify) = {
            let lock = plugin.tracker.read().await;
            match lock.get_workload_data(&workload_id) {
                Some(data) => (
                    data.pending_messages.clone(),
                    data.pending_requests.clone(),
                    data.notify.clone(),
                ),
                None => wasmtime::bail!("workload state not found"),
            }
        };

        // Generate a unique reply-to subject
        let reply_to = format!("_INBOX.{}", uuid::Uuid::new_v4());

        // Create a oneshot channel for the response
        let (tx, rx) = oneshot::channel();

        // Register the pending request
        {
            let mut lock = pending_requests.write().await;
            lock.insert(reply_to.clone(), tx);
        }

        // Create the request message with reply_to set
        let msg = types::BrokerMessage {
            subject,
            reply_to: Some(reply_to.clone()),
            body,
        };

        debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Sending request");
        // Push the message to the queue
        let queue_full = {
            let mut msg_lock = pending_messages.write().await;

            if msg_lock.len() >= MAX_QUEUE_SIZE {
                true
            } else {
                msg_lock.push_back(msg);
                false
            }
        };

        if !queue_full {
            notify.notify_one();
        } else {
            let mut req_lock = pending_requests.write().await;
            req_lock.remove(&reply_to);
            return Ok(Err("message queue full".to_string()));
        }

        // Wait for the response with timeout
        let timeout_duration = std::time::Duration::from_millis(timeout_ms as u64);
        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(response)) => Ok(Ok(response)),
            Ok(Err(_)) => {
                // Channel was dropped without sending
                warn!("request channel closed without response");
                Ok(Err("request channel closed without response".to_string()))
            }
            Err(_) => {
                // Timeout - clean up the pending request
                let mut req_lock = pending_requests.write().await;
                // Clean up the pending request since we're failing
                req_lock.remove(&reply_to);
                warn!("request timed out after {timeout_ms}ms");
                Ok(Err(format!("request timed out after {timeout_ms}ms")))
            }
        }
    }

    #[instrument(skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> wasmtime::Result<Result<(), String>> {
        let Some(plugin) = self.get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID) else {
            return Ok(Err("plugin not available".to_string()));
        };

        let workload_id = self.ctx.workload_id.to_string();
        let (pending_messages, pending_requests, notify) = {
            let lock = plugin.tracker.read().await;
            match lock.get_workload_data(&workload_id) {
                Some(data) => (
                    data.pending_messages.clone(),
                    data.pending_requests.clone(),
                    data.notify.clone(),
                ),
                None => wasmtime::bail!("workload state not found"),
            }
        };

        {
            let mut lock = pending_requests.write().await;
            // Check if this is a reply to a pending request
            if let Some(sender) = lock.remove(&msg.subject) {
                debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Responding message");
                // This is a response to a request - send it via the oneshot channel
                let _ = sender.send(msg);
                return Ok(Ok(()));
            }
        }

        debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Publishing message");

        // Regular publish - push to the pending messages queue
        {
            let mut lock = pending_messages.write().await;
            if lock.len() >= MAX_QUEUE_SIZE {
                return Ok(Err("message queue full".to_string()));
            }

            lock.push_back(msg);
        }
        notify.notify_one();
        Ok(Ok(()))
    }
}

impl<'a> types::Host for ActiveCtx<'a> {}

#[async_trait::async_trait]
impl HostPlugin for InMemoryMessaging {
    fn id(&self) -> &'static str {
        PLUGIN_MESSAGING_MEMORY_ID
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

    async fn on_workload_bind(
        &self,
        workload: &UnresolvedWorkload,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(_interface) = interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "messaging")
        else {
            return Ok(());
        };

        self.tracker
            .write()
            .await
            .add_unresolved_workload(workload, WorkloadData::default());
        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(_interface) = interfaces
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

        let WorkloadItem::Component(component_handle) = component_handle else {
            // Only track components
            return Ok(());
        };

        if component_handle
            .world()
            .exports
            .contains(&WitInterface::from("wasmcloud:messaging/handler@0.2.0"))
        {
            debug!("Tracking component in in-memory messaging");
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    task_handle: None,
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
        let (pending_messages, notify) = {
            let lock = self.tracker.read().await;
            match lock.get_workload_data(workload.id()) {
                Some(data) => (data.pending_messages.clone(), data.notify.clone()),
                None => return Ok(()),
            }
        };

        let cancel_token = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => data.cancel_token.clone(),
                None => return Ok(()),
            }
        };

        let instance_pre = workload.instantiate_pre(component_id).await?;

        let pre = bindings::MessagingPre::new(instance_pre)
            .map_err(anyhow::Error::from)
            .context("failed to instantiate messaging pre")?;

        let workload = workload.clone();
        let component_id = component_id.to_string();

        debug!("Spawning messaging processor for component {component_id}");

        // Spawn the message processing task
        let task_component_id = component_id.clone();
        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                    _ = notify.notified() => {
                        // Try to get a message from the queue
                        let msg = pending_messages.write().await.pop_front();

                        let Some(msg) = msg else {
                            continue;
                        };

                        debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Processing message");

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

                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_wasmcloud_message_memory",
                            subject = %msg.subject,
                            reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"),
                        );

                        let fuel_meter = fuel_meter.clone();

                        tokio::spawn(async move {
                            let result = fuel_meter.observe(
                                &[
                                    KeyValue::new("plugin", PLUGIN_MESSAGING_MEMORY_ID),
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
                }
            }
        });

        // Store the task handle for tracking panics and cleanup
        {
            let mut lock = self.tracker.write().await;
            if let Some(data) = lock.get_component_data_mut(&task_component_id) {
                data.task_handle = Some(handle);
            }
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Clean up tracker
        let workload_cleanup = |_| async {};
        let component_cleanup = |component_data: ComponentData| async move {
            component_data.cancel_token.cancel();
            if let Some(handle) = component_data.task_handle {
                handle.abort();
            }
        };

        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(workload_id, workload_cleanup, component_cleanup)
            .await;

        Ok(())
    }
}
