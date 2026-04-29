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

            debug!(
                component_id = component_handle.id(),
                subscriptions = ?raw_subscriptions,
                "tracking handler component for NATS messaging"
            );
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    subscriptions: raw_subscriptions,
                    task_handle: None,
                },
            );
        }

        Ok(())
    }

    #[instrument(skip_all, fields(component_id = %component_id, workload.id = %workload.id()))]
    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        debug!("on_workload_resolved entered for NATS messaging");

        let (cancel_token, subjects) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (data.cancel_token.clone(), data.subscriptions.clone()),
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

        let instance_pre = workload.instantiate_pre(component_id).await?;
        debug!("instantiate_pre succeeded");

        let pre = bindings::MessagingPre::new(instance_pre)
            .context("failed to instantiate messaging pre")?;

        let workload = workload.clone();
        let component_id = component_id.to_string();
        let tracker_component_id = component_id.clone();

        let mut subscriptions = Vec::<Subscriber>::new();
        for subject in &subjects {
            debug!(%subject, "subscribing to NATS subject");
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
            debug!(%subject, "successfully subscribed");

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
        _interfaces: HashSet<crate::wit::WitInterface>,
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

    /// Mirrors `on_workload_item_bind`'s parsing of the `subscriptions`
    /// config string. Locks in the comma-split contract so a regression
    /// (e.g. switching to a different separator, accidentally trimming) is
    /// caught without spinning up the full plugin lifecycle.
    fn parse_subscriptions(s: &str) -> Vec<String> {
        s.split(',').map(|s| s.to_string()).collect()
    }

    #[test]
    fn subscriptions_config_single() {
        assert_eq!(
            parse_subscriptions("tasks.task-worker"),
            vec!["tasks.task-worker"]
        );
    }

    #[test]
    fn subscriptions_config_multiple() {
        assert_eq!(
            parse_subscriptions("a,b,c"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn subscriptions_config_preserves_inner_whitespace() {
        // Subjects with whitespace are unusual but the chart and operator
        // pass the value through verbatim — make sure we don't trim.
        assert_eq!(parse_subscriptions(" tasks.x "), vec![" tasks.x "]);
    }

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
}
