use std::collections::HashSet;
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::{ResolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};
use async_nats::Subscriber;
use futures::stream::StreamExt;
use opentelemetry::KeyValue;
use tokio::sync::RwLock;
use tracing::{Instrument, debug, instrument, trace, warn};
use wasmtime::error::Context as _;

const PLUGIN_MESSAGING_ID: &str = "wasmcloud-messaging";
const CONSUMER_GROUP_CONFIG: &str = "consumer_group";
const BROADCAST_CONSUMER_GROUP: &str = "broadcast";
const DEFAULT_CONSUMER_GROUP_PREFIX: &str = "wasmcloud";
const MAX_DEFAULT_CONSUMER_GROUP_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConsumerGroup {
    Grouped(String),
    Broadcast,
}

impl ConsumerGroup {
    fn resolve(
        configured: Option<&str>,
        workload_namespace: &str,
        workload_name: &str,
        component_name: &str,
    ) -> anyhow::Result<Self> {
        match configured {
            None => Ok(Self::Grouped(default_consumer_group(
                workload_namespace,
                workload_name,
                component_name,
            ))),
            Some(value) if value == BROADCAST_CONSUMER_GROUP => Ok(Self::Broadcast),
            Some(value) => {
                validate_consumer_group(value)?;
                Ok(Self::Grouped(value.to_string()))
            }
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Grouped(name) => Some(name),
            Self::Broadcast => None,
        }
    }
}

fn validate_consumer_group(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty(),
        "`{CONSUMER_GROUP_CONFIG}` cannot be empty; omit it for the default group or set it to `{BROADCAST_CONSUMER_GROUP}` for broadcast delivery"
    );
    anyhow::ensure!(
        !value
            .chars()
            .any(|c| c.is_whitespace() || c == '*' || c == '>'),
        "invalid `{CONSUMER_GROUP_CONFIG}` `{value}`: NATS consumer groups cannot contain whitespace, `*`, or `>`"
    );
    Ok(())
}

/// Return a stable, NATS-safe queue name for every replica of a logical
/// component. The readable prefix helps operators identify the consumer while
/// the FNV-1a suffix preserves distinctions lost through sanitization or
/// truncation without adding a hashing dependency to the runtime.
fn default_consumer_group(namespace: &str, workload: &str, component: &str) -> String {
    let identity = format!("{namespace}\0{workload}\0{component}");
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in identity.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    let readable = [namespace, workload, component]
        .into_iter()
        .map(|part| {
            part.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(".");
    let suffix = format!(".{hash:016x}");
    let max_readable_len = MAX_DEFAULT_CONSUMER_GROUP_LEN
        .saturating_sub(DEFAULT_CONSUMER_GROUP_PREFIX.len() + 1 + suffix.len());
    let readable = &readable[..readable.floor_char_boundary(max_readable_len)];
    format!("{DEFAULT_CONSUMER_GROUP_PREFIX}.{readable}{suffix}")
}

/// Server-side synchronization barrier for the NATS client.
///
/// `client.flush()` in `async_nats` only flushes the local TCP write buffer
/// — it does not wait for the server to acknowledge that prior SUBs have
/// been registered. After flush returns, NATS may not yet have processed
/// our subscriptions, so an immediate request on a subscribed subject can
/// race ahead and hit "no responders" (this is exactly the failure mode
/// of #5074 in environments where the data path is slow enough to widen
/// the race window — kubernetes with TLS).
///
/// To bound the race, this helper subscribes to a fresh inbox, publishes a
/// single byte to it, and awaits the round-tripped message. NATS processes
/// per-connection commands in order, so once we receive the sentinel back
/// every earlier SUB on this connection is guaranteed to be active.
async fn sync_with_server(client: &async_nats::Client) -> anyhow::Result<()> {
    use futures::stream::StreamExt;

    let inbox = client.new_inbox();
    let mut sentinel = client
        .subscribe(inbox.clone())
        .await
        .context("failed to subscribe to sync inbox")?;
    client
        .publish(inbox, bytes::Bytes::from_static(&[0]))
        .await
        .context("failed to publish sync message")?;
    // Tight bound — if NATS is genuinely unreachable we'll bail; otherwise
    // the round trip is sub-millisecond locally, low single-digit ms in
    // kubernetes.
    match tokio::time::timeout(std::time::Duration::from_secs(5), sentinel.next()).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => anyhow::bail!("sync inbox subscription closed before sentinel arrived"),
        Err(_) => anyhow::bail!("sync with NATS timed out after 5s"),
    }
}

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
    consumer_group: ConsumerGroup,
    cancel_token: tokio_util::sync::CancellationToken,
    task_handle: Option<tokio::task::JoinHandle<()>>,
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
    #[instrument(name = "wasmcloud.messaging.request", skip_all, fields(subject = %subject, timeout_ms))]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::BrokerMessage, String>> {
        let plugin = self.try_get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID)?;

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

    #[instrument(name = "wasmcloud.messaging.publish", skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> wasmtime::Result<Result<(), String>> {
        let plugin = self.try_get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID)?;

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
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let Some(interface) = interfaces.get("wasmcloud", "messaging", &[]) else {
            return Ok(());
        };

        // Subscriptions come from this component's own `LocalResources.config`
        // (so workers in one workload can subscribe to different subjects),
        // falling back to the workload-scoped host interface config. Capture
        // the host-interface fallback before borrowing the component.
        let interface_subscriptions = interface.config.get("subscriptions").cloned();
        let interface_consumer_group = interface.config.get(CONSUMER_GROUP_CONFIG).cloned();

        bindings::wasmcloud::messaging::types::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        bindings::wasmcloud::messaging::consumer::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;

        let local_subscriptions = component_handle
            .local_resources()
            .config
            .get("subscriptions")
            .cloned();
        let local_consumer_group = component_handle
            .local_resources()
            .config
            .get(CONSUMER_GROUP_CONFIG)
            .cloned();

        // Track a handler component OR a long-lived handler service:
        // `WorkloadItem` derefs to the underlying metadata for both, so the
        // subscriber loop is set up either way (and its receive loop delivers to
        // the running service when one is registered). Works whether or not the
        // workload declares a `wasmcloud:messaging` host interface entry, and
        // matches the handler export version-tolerantly.
        if super::exports_messaging_handler(&component_handle.world()) {
            let raw = local_subscriptions.or(interface_subscriptions);
            let raw_subscriptions = super::parse_subscriptions(raw.as_deref());
            let component_name = match component_handle {
                WorkloadItem::Component(component) => component.name().to_string(),
                WorkloadItem::Service(_) => "service".to_string(),
            };
            let consumer_group = ConsumerGroup::resolve(
                local_consumer_group
                    .as_deref()
                    .or(interface_consumer_group.as_deref()),
                component_handle.workload_namespace(),
                component_handle.workload_name(),
                &component_name,
            )?;

            debug!(
                component_id = component_handle.id(),
                subscriptions = ?raw_subscriptions,
                consumer_group = consumer_group.name().unwrap_or(BROADCAST_CONSUMER_GROUP),
                "tracking handler component for NATS messaging"
            );
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    subscriptions: raw_subscriptions,
                    consumer_group,
                    task_handle: None,
                },
            );
        }

        Ok(())
    }

    #[instrument(name = "wasmcloud.messaging.on_workload_resolved", skip_all, fields(component_id = %component_id, workload.id = %workload.id()))]
    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        debug!("on_workload_resolved entered for NATS messaging");

        let (cancel_token, subjects, consumer_group) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (
                    data.cancel_token.clone(),
                    data.subscriptions.clone(),
                    data.consumer_group.clone(),
                ),
                None => {
                    debug!("no tracker entry for component, skipping subscription setup");
                    return Ok(());
                }
            }
        };

        debug!(?subjects, "loaded subscriptions from tracker");

        if subjects.is_empty() {
            debug!("no subscriptions configured, skipping subscription setup");
            return Ok(());
        }

        // A long-lived handler service has no per-component instance to
        // pre-instantiate; its receive loop delivers to the running service
        // instead. Only components get a `MessagingPre` for per-message work.
        let pre = match workload.instantiate_pre(component_id).await {
            Ok(instance_pre) => Some(
                bindings::MessagingPre::new(instance_pre)
                    .context("failed to instantiate messaging pre")?,
            ),
            Err(e) => {
                trace!(component_id, error = %e, "no per-message instance (long-lived service); messages delivered to the service");
                None
            }
        };

        let workload = workload.clone();
        let component_id = component_id.to_string();
        let tracker_component_id = component_id.clone();

        let mut subscriptions = Vec::<Subscriber>::new();
        for subject in &subjects {
            debug!(
                %subject,
                consumer_group = consumer_group.name().unwrap_or(BROADCAST_CONSUMER_GROUP),
                "subscribing to NATS subject"
            );
            let result = match &consumer_group {
                ConsumerGroup::Grouped(group) => {
                    self.client
                        .queue_subscribe(subject.clone(), group.clone())
                        .await
                }
                ConsumerGroup::Broadcast => self.client.subscribe(subject.clone()).await,
            };
            let sub = match result {
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
            debug!(
                %subject,
                consumer_group = consumer_group.name().unwrap_or(BROADCAST_CONSUMER_GROUP),
                "successfully subscribed"
            );

            subscriptions.push(sub);
        }

        // Make sure NATS has actually processed all the subscriptions above
        // before we let `on_workload_resolved` return Ok. `client.flush()`
        // only flushes the local TCP write buffer — NATS may not have seen
        // the SUB protocol messages yet by the time it returns, so a
        // request to the subscribed subject fired immediately after can
        // race ahead and get "no responders". To force a true server-side
        // round-trip we subscribe to a fresh inbox subject, publish a single
        // sentinel byte to it, and wait for the message to come back. NATS
        // processes commands per-connection in order, so by the time the
        // sentinel arrives, every SUB queued earlier on this connection has
        // also been processed. See https://github.com/wasmCloud/wasmCloud/issues/5074.
        if let Err(e) = sync_with_server(&self.client).await {
            warn!(error = ?e, "failed to sync subscriptions with NATS server");
        }

        let mut messages = futures::stream::select_all(subscriptions);
        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        let span = tracing::Span::current();
        let handle = tokio::spawn(async move {
            debug!(
                parent: &span,
                subjects = ?subjects,
                "NATS subscriber loop started"
            );
            loop {
                tokio::select! {
                    maybe_msg = messages.next() => {
                        let msg = match maybe_msg {
                            None => {
                                warn!(
                                    parent: &span,
                                    component_id = %component_id,
                                    "NATS subscriber stream closed unexpectedly; handler will stop receiving messages"
                                );
                                break;
                            }
                            Some(msg) => {
                                msg
                            }
                        };

                        let subject = msg.subject.to_string();
                        let reply_to = msg.reply.as_ref().map(|r| r.to_string());
                        let body: Vec<u8> = msg.payload.into();

                        // If this workload runs a long-lived trigger service for
                        // messaging, deliver to it (preserving its in-memory
                        // state) rather than instantiating a component per message.
                        if workload
                            .http_handler()
                            .has_trigger_service_messaging(workload.id())
                            .await
                        {
                            let broker = crate::host::trigger_service::BrokerMessage {
                                subject: subject.clone(),
                                body,
                                reply_to,
                            };
                            match workload
                                .http_handler()
                                .deliver_trigger_service_message(workload.id(), broker)
                                .await
                            {
                                Ok(Ok(())) => debug!(%subject, "trigger service handled message"),
                                Ok(Err(e)) => {
                                    warn!(%subject, error = %e, "trigger service message handler returned error")
                                }
                                Err(e) => {
                                    warn!(%subject, error = %e, "failed to deliver message to trigger service")
                                }
                            }
                            continue;
                        }

                        let Some(pre) = &pre else {
                            warn!(
                                %subject,
                                component_id = %component_id,
                                "no trigger service registered and no per-message instance; dropping message"
                            );
                            continue;
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
                        let msg = types::BrokerMessage {
                            subject,
                            reply_to,
                            body,
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
                        debug!(
                            parent: &span,
                            component_id = %component_id,
                            "NATS subscriber loop cancelled"
                        );
                        break;
                    }
                }
            }
        });

        {
            let mut lock = self.tracker.write().await;
            if let Some(data) = lock.get_component_data_mut(&tracker_component_id) {
                data.task_handle = Some(handle);
            } else {
                warn!(
                    component_id = %tracker_component_id,
                    "tracker entry vanished before task handle could be stored"
                );
            }
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
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
    //! Locks in the plugin's state-machine invariants without Docker /
    //! NATS / wasmtime: the seam between the plugin and its tracker, and
    //! pure-data parsing. Anything that requires a real
    //! `WorkloadComponent` / `ResolvedWorkload` is exercised by the
    //! integration suite instead.
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::plugin::WorkloadTrackerItem;
    use std::time::Duration;

    /// Tracker round-trip: stored subscriptions and a stored cancellation
    /// token are retrievable by the same component_id; cleanup cancels the
    /// stored token. Does not exercise the NATS client at all — the goal is
    /// to lock in the contract `on_workload_resolved` depends on.
    #[tokio::test]
    async fn tracker_round_trip_with_component_data() {
        use crate::plugin::WorkloadTracker;

        let mut tracker: WorkloadTracker<(), ComponentData> = WorkloadTracker::default();
        // We can't construct a real WorkloadComponent here without the
        // engine, so we simulate `add_component`'s effect directly via the
        // public maps. This documents the invariant the plugin relies on.
        let workload_id = "wl-1".to_string();
        let component_id = "c-1".to_string();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        tracker
            .workloads
            .entry(workload_id.clone())
            .or_insert_with(|| WorkloadTrackerItem {
                workload_data: None,
                components: std::collections::HashMap::new(),
            })
            .components
            .insert(
                component_id.clone(),
                ComponentData {
                    cancel_token,
                    subscriptions: vec!["tasks.x".to_string()],
                    consumer_group: ConsumerGroup::Grouped("workers".to_string()),
                    task_handle: None,
                },
            );
        tracker
            .components
            .insert(component_id.clone(), workload_id.clone());

        let data = tracker
            .get_component_data(&component_id)
            .expect("component should be retrievable");
        assert_eq!(data.subscriptions, vec!["tasks.x".to_string()]);
        assert_eq!(data.consumer_group.name(), Some("workers"));
        assert!(!cancel_token_clone.is_cancelled());

        // Simulate on_workload_unbind's cleanup closure.
        tracker
            .remove_workload_with_cleanup(
                &workload_id,
                |_| async {},
                |cd: ComponentData| async move {
                    cd.cancel_token.cancel();
                },
            )
            .await;

        assert!(
            cancel_token_clone.is_cancelled(),
            "cleanup must propagate cancellation to the clone the spawn loop holds"
        );
        assert!(tracker.get_component_data(&component_id).is_none());
    }

    /// The cancel-token clone the spawn loop holds and the original in the
    /// tracker share state, so cancelling either one wakes the other.
    /// Catches anyone replacing `Clone` with `Copy`-style semantics that
    /// break the unbind→loop-exit signal.
    #[tokio::test]
    async fn cancel_token_clone_shares_state() {
        let original = tokio_util::sync::CancellationToken::new();
        let clone = original.clone();
        original.cancel();
        // cancelled() on the clone resolves immediately because the inner
        // state is shared.
        tokio::time::timeout(Duration::from_millis(50), clone.cancelled())
            .await
            .expect("cloned cancel token should observe the cancellation");
    }

    #[test]
    fn default_group_is_stable_for_component_replicas() {
        let first = default_consumer_group("orders", "processor", "worker");
        let second = default_consumer_group("orders", "processor", "worker");

        assert_eq!(first, second);
        assert!(first.starts_with("wasmcloud.orders.processor.worker."));
    }

    #[test]
    fn default_group_is_isolated_by_logical_component_identity() {
        let base = default_consumer_group("orders", "processor", "worker");

        assert_ne!(base, default_consumer_group("other", "processor", "worker"));
        assert_ne!(base, default_consumer_group("orders", "other", "worker"));
        assert_ne!(base, default_consumer_group("orders", "processor", "other"));
    }

    #[test]
    fn default_group_is_nats_safe_and_bounded() {
        let group = default_consumer_group(
            "namespace with spaces.*",
            &"workload".repeat(40),
            "handler.>",
        );

        assert!(group.len() <= MAX_DEFAULT_CONSUMER_GROUP_LEN);
        assert!(
            !group
                .chars()
                .any(|c| c.is_whitespace() || c == '*' || c == '>')
        );
        assert_eq!(
            group,
            default_consumer_group(
                "namespace with spaces.*",
                &"workload".repeat(40),
                "handler.>",
            )
        );
    }

    #[test]
    fn consumer_group_configuration_selects_default_explicit_or_broadcast() {
        let default = ConsumerGroup::resolve(None, "ns", "workload", "component").unwrap();
        assert_eq!(
            default,
            ConsumerGroup::Grouped(default_consumer_group("ns", "workload", "component"))
        );
        assert_eq!(
            ConsumerGroup::resolve(Some("shared-workers"), "ns", "workload", "component").unwrap(),
            ConsumerGroup::Grouped("shared-workers".to_string())
        );
        assert_eq!(
            ConsumerGroup::resolve(
                Some(BROADCAST_CONSUMER_GROUP),
                "ns",
                "workload",
                "component"
            )
            .unwrap(),
            ConsumerGroup::Broadcast
        );
    }

    #[test]
    fn consumer_group_configuration_rejects_invalid_values() {
        for value in ["", "two groups", "workers.*", "workers.>"] {
            assert!(
                ConsumerGroup::resolve(Some(value), "ns", "workload", "component").is_err(),
                "expected `{value}` to be rejected"
            );
        }
    }
}
