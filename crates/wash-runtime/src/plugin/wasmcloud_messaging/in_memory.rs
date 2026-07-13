use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};
use anyhow::Context;
use opentelemetry::KeyValue;
use tokio::sync::{Notify, RwLock, oneshot};
use tracing::{Instrument, debug, instrument, warn};

const PLUGIN_MESSAGING_MEMORY_ID: &str = "wasmcloud-messaging-memory";
const MAX_QUEUE_SIZE: usize = 10000;

/// A component's message inbox, shared between the publisher side
/// (`route_to_subscribers`) and the component's processing task.
type Inbox = Arc<RwLock<VecDeque<types::BrokerMessage>>>;

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

/// Per-workload tracking data. Holds the reply-routing table shared by every
/// component in the workload; message delivery itself is per-component (see
/// [`ComponentData`]).
#[derive(Default)]
struct WorkloadData {
    pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<types::BrokerMessage>>>>,
}

/// Per-component tracking data. Each handler component has its own subject
/// subscriptions and inbox queue, so a published message is delivered only to
/// the components whose subscriptions match its subject.
struct ComponentData {
    cancel_token: tokio_util::sync::CancellationToken,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Subjects this component subscribes to (NATS tokens: `*` one token,
    /// `>` one or more trailing tokens). Empty means "receive everything",
    /// preserving the single-handler behavior of earlier versions.
    subscriptions: Vec<String>,
    /// This component's inbox. `publish`/`request` push matching messages
    /// here; the component's processing task drains it.
    inbox: Inbox,
    notify: Arc<Notify>,
}

/// Returns whether `subject` matches NATS subscription `pattern`, where `*`
/// matches exactly one token and `>` matches one or more trailing tokens.
fn subject_matches(pattern: &str, subject: &str) -> bool {
    let mut subject_tokens = subject.split('.');
    let mut pattern_tokens = pattern.split('.').peekable();
    while let Some(pat) = pattern_tokens.next() {
        if pat == ">" {
            // `>` is only valid as the final token and matches one or more
            // remaining subject tokens.
            return pattern_tokens.peek().is_none() && subject_tokens.next().is_some();
        }
        match subject_tokens.next() {
            Some(sub) if pat == "*" || pat == sub => continue,
            _ => return false,
        }
    }
    // Every pattern token matched; the subject must be fully consumed too.
    subject_tokens.next().is_none()
}

/// Whether any of a component's `subscriptions` match `subject`. An empty
/// subscription list matches everything (single-handler back-compat).
fn subscriptions_match(subscriptions: &[String], subject: &str) -> bool {
    subscriptions.is_empty() || subscriptions.iter().any(|s| subject_matches(s, subject))
}

/// Pushes `msg` onto the inbox of every component in `workload_id` whose
/// subscriptions match its subject, waking each one. Returns an error only if
/// the workload is untracked or a target inbox is full.
async fn route_to_subscribers(
    plugin: &InMemoryMessaging,
    workload_id: &str,
    msg: &types::BrokerMessage,
) -> Result<(), String> {
    let targets: Vec<(Inbox, Arc<Notify>)> = {
        let lock = plugin.tracker.read().await;
        let Some(item) = lock.workloads.get(workload_id) else {
            return Err("workload state not found".to_string());
        };
        item.components
            .values()
            .filter(|c| subscriptions_match(&c.subscriptions, &msg.subject))
            .map(|c| (c.inbox.clone(), c.notify.clone()))
            .collect()
    };

    for (inbox, notify) in targets {
        {
            let mut queue = inbox.write().await;
            if queue.len() >= MAX_QUEUE_SIZE {
                return Err("message queue full".to_string());
            }
            queue.push_back(msg.clone());
        }
        notify.notify_one();
    }
    Ok(())
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
    #[instrument(name = "wasmcloud.messaging.request", skip_all, fields(subject = %subject, timeout_ms))]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::BrokerMessage, String>> {
        let plugin = self.try_get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID)?;

        let workload_id = self.ctx.workload_id.to_string();

        let pending_requests = {
            let lock = plugin.tracker.read().await;
            match lock.get_workload_data(&workload_id) {
                Some(data) => data.pending_requests.clone(),
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
        // Route the request to subscribers of its subject.
        if let Err(e) = route_to_subscribers(&plugin, &workload_id, &msg).await {
            pending_requests.write().await.remove(&reply_to);
            return Ok(Err(e));
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

    #[instrument(name = "wasmcloud.messaging.publish", skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> wasmtime::Result<Result<(), String>> {
        let plugin = self.try_get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID)?;

        let workload_id = self.ctx.workload_id.to_string();
        let pending_requests = {
            let lock = plugin.tracker.read().await;
            match lock.get_workload_data(&workload_id) {
                Some(data) => data.pending_requests.clone(),
                None => wasmtime::bail!("workload state not found"),
            }
        };

        {
            let mut lock = pending_requests.write().await;
            // Check if this is a reply to a pending request. Reply subjects
            // (`_INBOX.*`) are routed here, not to subscribers.
            if let Some(sender) = lock.remove(&msg.subject) {
                debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Responding message");
                // This is a response to a request - send it via the oneshot channel
                let _ = sender.send(msg);
                return Ok(Ok(()));
            }
        }

        debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Publishing message");

        // Regular publish - deliver to every subscriber of this subject.
        match route_to_subscribers(&plugin, &workload_id, &msg).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(e)),
        }
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
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasmcloud", "messaging", &[]) {
            return Ok(());
        }

        self.tracker
            .write()
            .await
            .add_unresolved_workload(workload, WorkloadData::default());
        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasmcloud", "messaging", &[]) {
            return Ok(());
        }

        bindings::wasmcloud::messaging::types::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasmcloud::messaging::consumer::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        // Per-component subscriptions come from this component's
        // `LocalResources.config` (set via `dev.components[].config` or a
        // WorkloadDeployment), so workers in one workload can subscribe to
        // different subjects.
        let subscriptions = super::parse_subscriptions(
            component_handle
                .local_resources()
                .config
                .get("subscriptions")
                .map(String::as_str),
        );

        let WorkloadItem::Component(component_handle) = component_handle else {
            // Only track components
            return Ok(());
        };

        if super::exports_messaging_handler(&component_handle.world()) {
            debug!(?subscriptions, "Tracking component in in-memory messaging");
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    task_handle: None,
                    subscriptions,
                    inbox: Arc::default(),
                    notify: Arc::new(Notify::new()),
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
        let (inbox, notify, cancel_token) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (
                    data.inbox.clone(),
                    data.notify.clone(),
                    data.cancel_token.clone(),
                ),
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
                        // Drain every message queued since the last wakeup, so a
                        // coalesced notification can't strand a message.
                        loop {
                        let msg = inbox.write().await.pop_front();

                        let Some(msg) = msg else {
                            break;
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
        _interfaces: WitInterfaces<'_>,
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

#[cfg(test)]
mod tests {
    use super::{subject_matches, subscriptions_match};

    #[test]
    fn exact_and_literal_tokens() {
        assert!(subject_matches("tasks.leet", "tasks.leet"));
        assert!(!subject_matches("tasks.leet", "tasks.reverse"));
        // Token counts must match for a literal pattern.
        assert!(!subject_matches("tasks.leet", "tasks.leet.extra"));
        assert!(!subject_matches("tasks.leet.extra", "tasks.leet"));
    }

    #[test]
    fn single_token_wildcard() {
        assert!(subject_matches("tasks.*", "tasks.leet"));
        assert!(subject_matches("tasks.*", "tasks.reverse"));
        // `*` matches exactly one token, not zero and not many.
        assert!(!subject_matches("tasks.*", "tasks"));
        assert!(!subject_matches("tasks.*", "tasks.leet.v2"));
    }

    #[test]
    fn multi_token_wildcard() {
        assert!(subject_matches("tasks.>", "tasks.leet"));
        assert!(subject_matches("tasks.>", "tasks.leet.v2"));
        // `>` requires at least one trailing token.
        assert!(!subject_matches("tasks.>", "tasks"));
    }

    #[test]
    fn empty_subscriptions_match_everything() {
        // Back-compat: a handler with no configured subscriptions receives
        // every subject, preserving single-handler behavior.
        assert!(subscriptions_match(&[], "anything.at.all"));
    }

    #[test]
    fn non_empty_subscriptions_match_only_listed_subjects() {
        let subs = vec!["tasks.leet".to_string()];
        assert!(subscriptions_match(&subs, "tasks.leet"));
        assert!(!subscriptions_match(&subs, "tasks.reverse"));
    }
}
