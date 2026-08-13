use std::collections::HashSet;
use std::sync::Arc;

use async_nats::Subscriber;
use futures::stream::StreamExt;
use opentelemetry::KeyValue;
use tokio::sync::RwLock;
use tracing::{Instrument, debug, instrument, trace, warn};
use wasmtime::error::Context as _;

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

/// Bindings for the async `wasmcloud:messaging@0.3.0` surface, served off the
/// same NATS client. A separate `bindgen!` (rather than one shared module)
/// because each plugin implements the generated host traits for its own backend
/// — the same arrangement the sync world already uses across the three plugins.
mod async_bindings {
    crate::wasmtime::component::bindgen!({
        world: "async-messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

// The two messaging surfaces, imported symmetrically: `*P2` is the sync
// `@0.2.0` binding, `*P3` the async `@0.3.0` one. Both are generated from the
// same WIT package but by different `bindgen!` invocations, so they are
// unrelated Rust types with identical names — aliasing both at the top keeps
// every use site below reading as a straight p2/p3 pair.
use bindings::wasmcloud::messaging0_2_0::consumer::{self as consumer_p2, Host as HostP2};
use bindings::wasmcloud::messaging0_2_0::types::{self as types_p2, Host as TypesHostP2};

use async_bindings::wasmcloud::messaging0_3_0::consumer::{
    self as consumer_p3, Host as HostP3, HostWithStore as HostWithStoreP3,
};
use async_bindings::wasmcloud::messaging0_3_0::types::{
    self as types_p3, BrokerMessage as AsyncBrokerMessage, Error as AsyncMsgError,
    Host as TypesHostP3,
};
use wasmtime::component::Accessor;

use super::MsgError;

super::async_messaging_conversions! {
    error: AsyncMsgError,
}

super::messaging_handler_dispatch! {
    sync: bindings,
    async: async_bindings,
}

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::{ResolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::wasmcloud_messaging::Admitted;
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

const PLUGIN_MESSAGING_ID: &str = "wasmcloud-messaging";
const CONSUMER_GROUP_CONFIG: &str = "consumer_group";
const BROADCAST_CONSUMER_GROUP: &str = "broadcast";
const DEFAULT_CONSUMER_GROUP_PREFIX: &str = "wasmcloud";
const MAX_DEFAULT_CONSUMER_GROUP_LEN: usize = 128;

pub struct ComponentData {
    subscriptions: Vec<String>,
    consumer_group: ConsumerGroup,
    cancel_token: tokio_util::sync::CancellationToken,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    /// Bounds how many messages this component processes at once, and so how
    /// many instances the subscription may create. See [`super::Admission`].
    admission: super::Admission,
}

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

#[derive(Clone)]
pub struct NatsMessaging {
    tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    client: Arc<async_nats::Client>,
    meters: Arc<RwLock<Meters>>,
    limits: super::MessagingLimits,
}

impl NatsMessaging {
    /// Build the plugin with the default messaging ceilings
    /// ([`super::DEFAULT_MAX_IN_FLIGHT_HOST`] /
    /// [`super::DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT`]).
    pub fn new(client: Arc<async_nats::Client>) -> Self {
        Self::with_limits(client, super::MessagingLimits::default())
    }

    /// Build the plugin with operator-configured ceilings. The `limits` carry
    /// the host-wide semaphore, so pass the *same* value to every messaging
    /// backend on a host or each gets its own host budget.
    pub fn with_limits(client: Arc<async_nats::Client>, limits: super::MessagingLimits) -> Self {
        Self {
            client,
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meters: Default::default(),
            limits,
        }
    }
}

impl<'a> HostP2 for ActiveCtx<'a> {
    #[instrument(name = "wasmcloud.messaging.request", skip_all, fields(subject = %subject, timeout_ms))]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types_p2::BrokerMessage, String>> {
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
        Ok(Ok(types_p2::BrokerMessage {
            subject: resp.subject.to_string(),
            reply_to,
            body: resp.payload.into(),
        }))
    }

    #[instrument(name = "wasmcloud.messaging.publish", skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(
        &mut self,
        msg: types_p2::BrokerMessage,
    ) -> wasmtime::Result<Result<(), String>> {
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

impl<'a> TypesHostP2 for ActiveCtx<'a> {}

/// The async `@0.3.0` consumer, over the same NATS client as the sync one.
///
/// `async func`s bind through wasmtime's concurrent ABI, so these are `async
/// fn`s on `SharedCtx` taking an [`Accessor`] rather than `&mut self` methods on
/// `ActiveCtx`. Errors are classified into [`MsgError`] and lowered into the WIT
/// `error` variant. Note this differs from the sync `publish` above, which
/// *traps* the guest on a publish failure; the async surface reports it as an
/// ordinary `result` error, which is what the WIT says it is.
impl<T: 'static + Send> HostWithStoreP3<T> for SharedCtx {
    async fn request(
        accessor: &Accessor<T, Self>,
        subject: String,
        body: wasmtime::component::StreamReader<u8>,
        timeout_ms: Option<u32>,
    ) -> wasmtime::Result<Result<AsyncBrokerMessage, AsyncMsgError>> {
        let plugin =
            accessor.with(|mut a| a.get().try_get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID))?;

        // The client takes a complete payload, so the body is drained before the
        // request goes out (see `collect_body`). `timeout-ms` therefore covers
        // only the broker round-trip, not how fast the guest wrote the body.
        let body = match super::collect_body(accessor, body).await? {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Err(e.into())),
        };

        // `None` falls through to the NATS client's own request timeout
        // (10s unless configured otherwise); `TimedOut` classifies below.
        let request_future = plugin.client.request(subject, body.into());
        let resp = match timeout_ms {
            Some(ms) => {
                let duration = std::time::Duration::from_millis(ms as u64);
                match tokio::time::timeout(duration, request_future).await {
                    Ok(Ok(msg)) => msg,
                    Ok(Err(e)) => return Ok(Err(classify_request(&e).into())),
                    Err(_) => {
                        warn!("request timed out after {ms}ms");
                        return Ok(Err(AsyncMsgError::Timeout));
                    }
                }
            }
            None => match request_future.await {
                Ok(msg) => msg,
                Err(e) => return Ok(Err(classify_request(&e).into())),
            },
        };
        let body = super::mint_body(accessor, resp.payload.into())?;
        Ok(Ok(AsyncBrokerMessage {
            subject: resp.subject.to_string(),
            reply_to: resp.reply.as_ref().map(|r| r.to_string()),
            body,
        }))
    }

    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: AsyncBrokerMessage,
    ) -> wasmtime::Result<Result<(), AsyncMsgError>> {
        let plugin =
            accessor.with(|mut a| a.get().try_get_plugin::<NatsMessaging>(PLUGIN_MESSAGING_ID))?;

        let AsyncBrokerMessage {
            subject,
            body,
            reply_to,
        } = msg;
        let body = match super::collect_body(accessor, body).await? {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Err(e.into())),
        };

        let result = if let Some(reply_to) = reply_to {
            plugin
                .client
                .publish_with_reply(subject, reply_to, body.into())
                .await
        } else {
            plugin.client.publish(subject, body.into()).await
        };
        Ok(result.map_err(|e| classify_publish(&e).into()))
    }
}

impl HostP3 for ActiveCtx<'_> {}
impl TypesHostP3 for ActiveCtx<'_> {}

/// Classify an `async_nats` request failure into a [`MsgError`].
///
/// `NoResponders` has no named WIT case — the broker is healthy and the subject
/// is valid, there is simply nothing subscribed — so it stays `Other` rather
/// than being misreported as a timeout or an unavailable broker.
pub(super) fn classify_request(e: &async_nats::RequestError) -> MsgError {
    use async_nats::RequestErrorKind::*;
    let detail = format!("failed to send request: {e}");
    match e.kind() {
        TimedOut => MsgError::Timeout(detail),
        InvalidSubject => MsgError::SubjectInvalid(detail),
        MaxPayloadExceeded => MsgError::MessageTooLarge(detail),
        Other => MsgError::BrokerUnavailable(detail),
        NoResponders => MsgError::Other(detail),
    }
}

/// Classify an `async_nats` publish failure into a [`MsgError`].
pub(super) fn classify_publish(e: &async_nats::PublishError) -> MsgError {
    use async_nats::PublishErrorKind::*;
    let detail = format!("failed to send message: {e}");
    match e.kind() {
        InvalidSubject => MsgError::SubjectInvalid(detail),
        MaxPayloadExceeded => MsgError::MessageTooLarge(detail),
        Send => MsgError::BrokerUnavailable(detail),
    }
}

#[async_trait::async_trait]
impl HostPlugin for NatsMessaging {
    fn id(&self) -> &'static str {
        PLUGIN_MESSAGING_ID
    }

    /// Serves both messaging revisions. A workload selects one by the version on
    /// its `wasmcloud:messaging` host-interface entry; a versionless entry gets
    /// the sync `@0.2.0` surface, preserving the behaviour of workloads written
    /// before `@0.3.0` existed.
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([
                WitInterface::from("wasmcloud:messaging/consumer,types@0.2.0"),
                WitInterface::from("wasmcloud:messaging/consumer,types@0.3.0"),
            ]),
            exports: HashSet::from([
                WitInterface::from("wasmcloud:messaging/handler@0.2.0"),
                WitInterface::from("wasmcloud:messaging/handler@0.3.0"),
            ]),
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
        let interface_max_in_flight = interface.config.get(super::MAX_IN_FLIGHT_CONFIG).cloned();
        let interface_admission_wait = interface.config.get(super::ADMISSION_WAIT_CONFIG).cloned();

        // Bind only the revision(s) the workload actually declared: the two
        // surfaces are separate linker instances, and binding one a component
        // never imports is harmless but binding the wrong one is not.
        if super::declares_async_messaging(&interfaces) {
            types_p3::add_to_linker::<_, SharedCtx>(component_handle.linker(), extract_active_ctx)?;
            consumer_p3::add_to_linker::<_, SharedCtx>(
                component_handle.linker(),
                extract_active_ctx,
            )?;
        } else {
            types_p2::add_to_linker::<_, SharedCtx>(component_handle.linker(), extract_active_ctx)?;
            consumer_p2::add_to_linker::<_, SharedCtx>(
                component_handle.linker(),
                extract_active_ctx,
            )?;
        }

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
        let local_max_in_flight = component_handle
            .local_resources()
            .config
            .get(super::MAX_IN_FLIGHT_CONFIG)
            .cloned();
        let local_admission_wait = component_handle
            .local_resources()
            .config
            .get(super::ADMISSION_WAIT_CONFIG)
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
                // A long-lived handler service has no per-message instance to
                // bound; its delivery path is gated elsewhere.
                WorkloadItem::Service(_) => "service".to_string(),
            };
            let max_in_flight = super::parse_max_in_flight(
                local_max_in_flight
                    .as_deref()
                    .or(interface_max_in_flight.as_deref()),
            );
            let consumer_group = ConsumerGroup::resolve(
                local_consumer_group
                    .as_deref()
                    .or(interface_consumer_group.as_deref()),
                component_handle.workload_namespace(),
                component_handle.workload_name(),
                &component_name,
            )?;

            let admission_wait = super::parse_admission_wait(
                local_admission_wait
                    .as_deref()
                    .or(interface_admission_wait.as_deref()),
            );
            // The same namespace/workload/component triple the consumer group
            // is built from above. It both attributes a shed message to
            // something a manifest author recognizes and selects the gate, so
            // replicas of this deployment on this host share one ceiling
            // rather than getting one apiece.
            let identity = super::AdmissionIdentity::new(
                component_handle.workload_namespace(),
                component_handle.workload_name(),
                &component_name,
            );
            let admission = self
                .limits
                .admission(&identity, max_in_flight)
                .with_admission_wait(admission_wait)
                .with_subscriptions(&raw_subscriptions);

            debug!(
                component_id = component_handle.id(),
                subscriptions = ?raw_subscriptions,
                consumer_group = consumer_group.name().unwrap_or(BROADCAST_CONSUMER_GROUP),
                max_in_flight = admission.limit(),
                "tracking handler component for NATS messaging"
            );
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    subscriptions: raw_subscriptions,
                    consumer_group,
                    task_handle: None,
                    admission,
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

        let (cancel_token, subjects, consumer_group, admission) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (
                    data.cancel_token.clone(),
                    data.subscriptions.clone(),
                    data.consumer_group.clone(),
                    data.admission.clone(),
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
            Ok(instance_pre) => {
                Some(HandlerPre::new(instance_pre).context("failed to instantiate messaging pre")?)
            }
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

                        // Admission. Taken BEFORE the store and instance are
                        // built and held until the handler returns, so permits
                        // held and instances alive are the same number and the
                        // ceiling is structural rather than advisory.
                        //
                        // Waiting here stops us draining the subscription.
                        // That cannot back up the socket or endanger the shared
                        // connection — async-nats' reader `try_send`s into a
                        // per-subscription buffer and drops on overflow rather
                        // than blocking — but the overflow it does cause is
                        // silent, so the wait is bounded and we shed loudly at
                        // the deadline instead of letting the buffer discard
                        // messages nobody counted.
                        //
                        // Selecting on the cancel token keeps shutdown prompt
                        // while parked on a saturated semaphore, and is
                        // cancel-safe: dropping the future releases whichever
                        // permit it had already taken.
                        let permit = tokio::select! {
                            admitted = admission.acquire_before_deadline(&component_id, &subject) => {
                                match admitted {
                                    Admitted::Slot(permit) => permit,
                                    // Already logged and counted; drop this
                                    // message and resume draining.
                                    //
                                    // A request/reply caller is NOT told, and
                                    // waits out its own `timeout_ms`. Telling
                                    // it would mean publishing to `reply_to`,
                                    // and `request` resolves on the first
                                    // message to reach its inbox — so where
                                    // more than one component subscribes to a
                                    // subject, the saturated one's instant
                                    // notice would beat a healthy one's real
                                    // reply and fail a request that was about
                                    // to succeed. The in-memory backend can
                                    // fast-fail precisely because it knows its
                                    // own fan-out; here that is unknowable
                                    // from inside a single subscriber.
                                    Admitted::Shed => continue,
                                    // Closed: the component is going away.
                                    Admitted::Closed => break,
                                }
                            }
                            _ = cancel_token.cancelled() => {
                                debug!(
                                    parent: &span,
                                    component_id = %component_id,
                                    "NATS subscriber loop cancelled while awaiting admission"
                                );
                                break;
                            }
                        };

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for component {component_id}: {e}");
                                continue;
                            }
                            Ok(s) => s,
                        };
                        let proxy = match pre.instantiate(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate component {component_id}: {e}");
                                continue;
                            }
                            Ok(p) => p,
                        };
                        let msg = types_p2::BrokerMessage {
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
                            // Released on completion, trap or not — which is
                            // what frees the instance slot this message holds.
                            let _permit = permit;
                            let result = fuel_meter.observe(
                                &[
                                    KeyValue::new("plugin", PLUGIN_MESSAGING_ID),
                                    KeyValue::new("subject", msg.subject.to_string()),
                                ],
                                &mut store,
                                async move |store| {
                                    proxy
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
            // Wakes a loop parked on a saturated gate with `Admitted::Closed`.
            // The token above covers the same case; this makes the closed
            // semaphore a real signal rather than a documented one that only
            // tests ever produce.
            component_data.admission.close();
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
                    admission: crate::plugin::wasmcloud_messaging::MessagingLimits::default()
                        .admission(
                            &crate::plugin::wasmcloud_messaging::AdmissionIdentity::new(
                                "test-ns", "test-workload", "worker",
                            ),
                            None,
                        ),
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
