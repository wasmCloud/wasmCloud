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
use tracing::{Instrument, debug, instrument, trace, warn};

pub(crate) const PLUGIN_MESSAGING_MEMORY_ID: &str = "wasmcloud-messaging-memory";
const MAX_QUEUE_SIZE: usize = 10000;
/// Default `request` timeout when the caller passes `none`, matching the NATS
/// client's default of 10 seconds.
const DEFAULT_REQUEST_TIMEOUT_MS: u32 = 10_000;

/// A component's message inbox, shared between the publisher side
/// (`route_to_subscribers`) and the component's processing task.
type Inbox = Arc<RwLock<VecDeque<QueuedMessage>>>;

/// A message queued for one component, with what the router knew when it was
/// routed.
struct QueuedMessage {
    msg: types_p2::BrokerMessage,
    /// Whether this component was the *only* subscriber the message went to.
    ///
    /// Decides whether shedding this message may fail its requester outright.
    /// If another component also received it, that one may still reply, and
    /// completing the pending request now would fail a request that was about
    /// to succeed — the sole subscriber is exactly the case where "I dropped
    /// it" and "nobody will answer" are the same statement.
    ///
    /// Known only at routing time, which is why it is carried on the message
    /// rather than recomputed in the drain loop: by then the subscriber set may
    /// have changed, and the answer that matters is the one that was true when
    /// the message fanned out.
    sole_subscriber: bool,
}

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

/// Bindings for the async `wasmcloud:messaging@0.3.0` surface, served off the
/// same in-memory core. A separate `bindgen!` (rather than one shared module)
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
use crate::plugin::WorkloadTracker;

super::async_messaging_conversions! {
    error: AsyncMsgError,
}

super::messaging_handler_dispatch! {
    sync: bindings,
    async: async_bindings,
}

/// The workload's reply-routing table: one entry per in-flight `request`,
/// keyed by the `_INBOX.*` subject it is waiting on.
type PendingRequests =
    Arc<RwLock<HashMap<String, oneshot::Sender<Result<types_p2::BrokerMessage, MsgError>>>>>;

/// Per-workload tracking data. Holds the reply-routing table shared by every
/// component in the workload; message delivery itself is per-component (see
/// [`ComponentData`]).
#[derive(Default)]
struct WorkloadData {
    /// In-flight `request` calls, keyed by the `_INBOX.*` subject each is
    /// waiting on.
    ///
    /// Carries a `Result` rather than a bare message so a request the admission
    /// gate sheds can be failed immediately, with the same
    /// [`MsgError::QuotaExceeded`] the NATS backend produces. A shed message is
    /// dropped, and without this the requester cannot tell that from a slow
    /// handler until its own `timeout_ms` expires — under exactly the
    /// saturation that produces shedding.
    pending_requests: PendingRequests,
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
    /// Bounds how many messages this component processes at once, and so how
    /// many instances the receive loop may create. See [`super::Admission`].
    admission: super::Admission,
}

use super::subject_matches;

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
    msg: &types_p2::BrokerMessage,
) -> Result<(), MsgError> {
    let targets: Vec<(Inbox, Arc<Notify>)> = {
        let lock = plugin.tracker.read().await;
        let Some(item) = lock.workloads.get(workload_id) else {
            return Err(MsgError::Other("workload state not found".to_string()));
        };
        item.components
            .values()
            .filter(|c| subscriptions_match(&c.subscriptions, &msg.subject))
            .map(|c| (c.inbox.clone(), c.notify.clone()))
            .collect()
    };

    let sole_subscriber = targets.len() == 1;
    for (inbox, notify) in targets {
        {
            let mut queue = inbox.write().await;
            if queue.len() >= MAX_QUEUE_SIZE {
                // A full inbox is backpressure, not a broker fault: the
                // subscriber is not draining fast enough.
                return Err(MsgError::QuotaExceeded("message queue full".to_string()));
            }
            queue.push_back(QueuedMessage {
                msg: msg.clone(),
                sole_subscriber,
            });
        }
        notify.notify_one();
    }
    Ok(())
}

/// The `request` core, shared by the sync `@0.2.0` and async `@0.3.0` surfaces.
///
/// Synthesizes an `_INBOX.*` reply subject, registers a oneshot for it, routes
/// the message, and awaits the reply. Returns a classified [`MsgError`]; each
/// `Host` impl lowers it for its own WIT spelling.
async fn do_request(
    plugin: &InMemoryMessaging,
    workload_id: &str,
    subject: String,
    body: Vec<u8>,
    timeout_ms: Option<u32>,
) -> wasmtime::Result<Result<types_p2::BrokerMessage, MsgError>> {
    // Mirror the NATS client's default request timeout when the caller passes
    // none, so the two backends agree on what "backend default" means.
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
    let pending_requests = {
        let lock = plugin.tracker.read().await;
        match lock.get_workload_data(workload_id) {
            Some(data) => data.pending_requests.clone(),
            None => wasmtime::bail!("workload state not found"),
        }
    };

    // Generate a unique reply-to subject
    let reply_to = format!("_INBOX.{}", uuid::Uuid::now_v7());

    // Create a oneshot channel for the response
    let (tx, rx) = oneshot::channel();

    // Register the pending request
    {
        let mut lock = pending_requests.write().await;
        lock.insert(reply_to.clone(), tx);
    }

    // Create the request message with reply_to set
    let msg = types_p2::BrokerMessage {
        subject,
        reply_to: Some(reply_to.clone()),
        body,
    };

    debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Sending request");
    // Route the request to subscribers of its subject.
    if let Err(e) = route_to_subscribers(plugin, workload_id, &msg).await {
        pending_requests.write().await.remove(&reply_to);
        return Ok(Err(e));
    }

    // Wait for the response with timeout
    let timeout_duration = std::time::Duration::from_millis(timeout_ms as u64);
    match tokio::time::timeout(timeout_duration, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => {
            // Channel was dropped without sending
            warn!("request channel closed without response");
            Ok(Err(MsgError::Other(
                "request channel closed without response".to_string(),
            )))
        }
        Err(_) => {
            // Timeout - clean up the pending request
            let mut req_lock = pending_requests.write().await;
            // Clean up the pending request since we're failing
            req_lock.remove(&reply_to);
            warn!("request timed out after {timeout_ms}ms");
            Ok(Err(MsgError::Timeout(format!(
                "request timed out after {timeout_ms}ms"
            ))))
        }
    }
}

/// The `publish` core, shared by both surfaces. Reply subjects (`_INBOX.*`) are
/// completed against the pending-request table; anything else is routed to the
/// workload's matching subscribers.
async fn do_publish(
    plugin: &InMemoryMessaging,
    workload_id: &str,
    msg: types_p2::BrokerMessage,
) -> wasmtime::Result<Result<(), MsgError>> {
    let pending_requests = {
        let lock = plugin.tracker.read().await;
        match lock.get_workload_data(workload_id) {
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
            let _ = sender.send(Ok(msg));
            return Ok(Ok(()));
        }
    }

    debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Publishing message");

    // Regular publish - deliver to every subscriber of this subject.
    Ok(route_to_subscribers(plugin, workload_id, &msg).await)
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
    limits: super::MessagingLimits,
}

impl InMemoryMessaging {
    /// Build the backend with the default messaging ceilings
    /// ([`super::DEFAULT_MAX_IN_FLIGHT_HOST`] /
    /// [`super::DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT`]).
    pub fn new() -> Self {
        Self::with_limits(super::MessagingLimits::default())
    }

    /// Build the backend with operator-configured ceilings. The `limits` carry
    /// the host-wide semaphore, so pass the *same* value to every messaging
    /// backend on a host or each gets its own host budget.
    pub fn with_limits(limits: super::MessagingLimits) -> Self {
        Self {
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meters: Default::default(),
            limits,
        }
    }

    /// Route a message to a workload's subscribers, exactly as an inbound publish
    /// would: it is enqueued to every component whose subscriptions match
    /// `subject`, then processed by that component's receive loop. Lets a host or
    /// test inject a message without a component-side `consumer.publish`.
    pub async fn publish(
        &self,
        workload_id: &str,
        subject: &str,
        body: Vec<u8>,
    ) -> Result<(), String> {
        route_to_subscribers(
            self,
            workload_id,
            &types_p2::BrokerMessage {
                subject: subject.to_string(),
                reply_to: None,
                body,
            },
        )
        .await
        .map_err(Into::into)
    }
}

impl Default for InMemoryMessaging {
    fn default() -> Self {
        Self::new()
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
        let plugin = self.try_get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID)?;
        let workload_id = self.ctx.workload_id.to_string();
        Ok(
            do_request(&plugin, &workload_id, subject, body, Some(timeout_ms))
                .await?
                .map_err(Into::into),
        )
    }

    #[instrument(name = "wasmcloud.messaging.publish", skip_all, fields(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>")))]
    async fn publish(
        &mut self,
        msg: types_p2::BrokerMessage,
    ) -> wasmtime::Result<Result<(), String>> {
        let plugin = self.try_get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID)?;
        let workload_id = self.ctx.workload_id.to_string();
        Ok(do_publish(&plugin, &workload_id, msg)
            .await?
            .map_err(Into::into))
    }
}

impl<'a> TypesHostP2 for ActiveCtx<'a> {}

/// The async `@0.3.0` consumer, over the same in-memory core as the sync one.
///
/// `async func`s bind through wasmtime's concurrent ABI, so these are `async
/// fn`s on `SharedCtx` taking an [`Accessor`] rather than `&mut self` methods on
/// `ActiveCtx`. The plugin handle and workload id are read inside
/// `Accessor::with` so no store borrow is held across an await.
impl<T: 'static + Send> HostWithStoreP3<T> for SharedCtx {
    async fn request(
        accessor: &Accessor<T, Self>,
        subject: String,
        body: wasmtime::component::StreamReader<u8>,
        timeout_ms: Option<u32>,
    ) -> wasmtime::Result<Result<AsyncBrokerMessage, AsyncMsgError>> {
        let (plugin, workload_id) = plugin_and_workload(accessor)?;
        // Drain the body (see `collect_body`), then mint the reply's body back
        // as a fresh stream.
        let body = match super::collect_body(accessor, body).await? {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Err(e.into())),
        };
        match do_request(&plugin, &workload_id, subject, body, timeout_ms).await? {
            Ok(reply) => {
                let body = super::mint_body(accessor, reply.body)?;
                Ok(Ok(AsyncBrokerMessage {
                    subject: reply.subject,
                    body,
                    reply_to: reply.reply_to,
                }))
            }
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: AsyncBrokerMessage,
    ) -> wasmtime::Result<Result<(), AsyncMsgError>> {
        let (plugin, workload_id) = plugin_and_workload(accessor)?;
        let AsyncBrokerMessage {
            subject,
            body,
            reply_to,
        } = msg;
        let body = match super::collect_body(accessor, body).await? {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Err(e.into())),
        };
        Ok(do_publish(
            &plugin,
            &workload_id,
            types_p2::BrokerMessage {
                subject,
                body,
                reply_to,
            },
        )
        .await?
        .map_err(Into::into))
    }
}

impl HostP3 for ActiveCtx<'_> {}
impl TypesHostP3 for ActiveCtx<'_> {}

/// Read the plugin handle and workload id out of the store without holding the
/// borrow across an await.
fn plugin_and_workload<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
) -> wasmtime::Result<(Arc<InMemoryMessaging>, String)> {
    let (plugin, workload_id) = accessor.with(|mut a| {
        let ctx = a.get();
        (
            ctx.try_get_plugin::<InMemoryMessaging>(PLUGIN_MESSAGING_MEMORY_ID),
            ctx.workload_id.to_string(),
        )
    });
    Ok((plugin?, workload_id))
}

#[async_trait::async_trait]
impl HostPlugin for InMemoryMessaging {
    fn id(&self) -> &'static str {
        PLUGIN_MESSAGING_MEMORY_ID
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

        // Per-component subscriptions come from this component's
        // `LocalResources.config` (set via `dev.components[].config` or a
        // WorkloadDeployment), so workers in one workload can subscribe to
        // different subjects. `max_in_flight` is read from the same place for
        // the same reason: both are messaging-plugin configuration, scoped to
        // one component.
        let subscriptions = super::parse_subscriptions(
            component_handle
                .local_resources()
                .config
                .get("subscriptions")
                .map(String::as_str),
        );
        let max_in_flight = super::parse_max_in_flight(
            component_handle
                .local_resources()
                .config
                .get(super::MAX_IN_FLIGHT_CONFIG)
                .map(String::as_str),
        );
        let admission_wait = super::parse_admission_wait(
            component_handle
                .local_resources()
                .config
                .get(super::ADMISSION_WAIT_CONFIG)
                .map(String::as_str),
        );

        // Track a handler component OR a long-lived handler service:
        // `WorkloadItem` derefs to the underlying metadata for both, so the
        // subscriber loop is set up either way (and its receive loop delivers to
        // the running service when one is registered).
        if super::exports_messaging_handler(&component_handle.world()) {
            // Manifest identity for the shed metric, resolved exactly as the
            // NATS backend does. A trigger service never reaches the admission
            // gate — the receive loop delivers to it before admitting — so that
            // arm is a bounded placeholder rather than a name that appears.
            let component_name = match component_handle {
                WorkloadItem::Component(component) => component.name().to_string(),
                WorkloadItem::Service(_) => "service".to_string(),
            };
            let identity = super::AdmissionIdentity::new(
                component_handle.workload_namespace(),
                component_handle.workload_name(),
                &component_name,
            );
            let admission = self
                .limits
                .admission(&identity, max_in_flight)
                .with_admission_wait(admission_wait)
                .with_subscriptions(&subscriptions);
            debug!(
                ?subscriptions,
                max_in_flight = admission.limit(),
                "Tracking component in in-memory messaging"
            );
            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    task_handle: None,
                    subscriptions,
                    inbox: Arc::default(),
                    notify: Arc::new(Notify::new()),
                    admission,
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
        let (inbox, notify, cancel_token, admission) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (
                    data.inbox.clone(),
                    data.notify.clone(),
                    data.cancel_token.clone(),
                    data.admission.clone(),
                ),
                None => return Ok(()),
            }
        };
        // Used only to fail a shed request/reply message immediately rather
        // than leaving the requester on its own timeout. Workload-scoped, like
        // the table itself, and registered by `on_workload_bind` before this
        // runs. `None` is not reachable from there — and if it ever were,
        // `request` itself bails on the same lookup, so there would be no
        // requester to notify. Kept as an `Option` rather than defaulted to an
        // empty map so that stays visible instead of becoming a lookup that
        // silently never matches.
        let pending_requests = self
            .tracker
            .read()
            .await
            .get_workload_data(workload.id())
            .map(|data| data.pending_requests.clone());

        // A long-lived handler service has no per-component instance to
        // pre-instantiate; its receive loop delivers to the running service
        // instead. Only components get a `MessagingPre` for per-message work.
        let pre = match workload.instantiate_pre(component_id).await {
            Ok(instance_pre) => Some(
                HandlerTarget::new(instance_pre)
                    .map_err(anyhow::Error::from)
                    .context("failed to instantiate messaging pre")?,
            ),
            Err(e) => {
                trace!(component_id, error = %e, "no per-message instance (long-lived service); messages delivered to the service");
                None
            }
        };

        let workload = workload.clone();
        let component_id = component_id.to_string();

        debug!("Spawning messaging processor for component {component_id}");

        // Spawn the message processing task
        let task_component_id = component_id.clone();
        let guest_meter = self.meters.read().await.guest();
        // As in the NATS backend: bounded labels only, so the subject stays
        // on the span rather than minting a time series per value.
        let attributes = vec![
            KeyValue::new("plugin", PLUGIN_MESSAGING_MEMORY_ID),
            KeyValue::new("operation", super::MESSAGING_OPERATION),
        ];

        let handle = tokio::spawn(async move {
            // Labelled so the inner drain below can end the whole task on
            // cancellation or teardown, not just the current drain pass.
            'task: loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                    _ = notify.notified() => {
                        // Drain every message queued since the last wakeup, so a
                        // coalesced notification can't strand a message.
                        loop {
                        let queued = inbox.write().await.pop_front();

                        let Some(QueuedMessage { msg, sole_subscriber }) = queued else {
                            break;
                        };

                        debug!(subject = %msg.subject, reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"), "Processing message");

                        // If this workload runs a long-lived trigger service for
                        // messaging, deliver to it (preserving its in-memory
                        // state) rather than instantiating a component per message.
                        if workload
                            .http_handler()
                            .has_trigger_service_messaging(workload.id())
                            .await
                        {
                            let broker = crate::host::trigger_service::BrokerMessage {
                                subject: msg.subject.clone(),
                                body: msg.body.clone(),
                                reply_to: msg.reply_to.clone(),
                            };
                            match workload
                                .http_handler()
                                .deliver_trigger_service_message(
                                    workload.id(),
                                    broker,
                                    attributes.clone(),
                                )
                                .await
                            {
                                Ok(Ok(())) => debug!(subject = %msg.subject, "trigger service handled message"),
                                Ok(Err(e)) => {
                                    warn!(subject = %msg.subject, error = %e, "trigger service message handler returned error")
                                }
                                Err(e) => {
                                    warn!(subject = %msg.subject, error = %e, "failed to deliver message to trigger service")
                                }
                            }
                            continue;
                        }

                        let Some(pre) = &pre else {
                            warn!(
                                subject = %msg.subject,
                                component_id = %component_id,
                                "no trigger service registered and no per-message instance; dropping message"
                            );
                            continue;
                        };

                        // Admission. Taken BEFORE any store or instance is
                        // built and held until the handler returns. Mirrors
                        // the NATS backend exactly; see `Admission::acquire`
                        // for why the component level is taken before the host
                        // one, and `DEFAULT_ADMISSION_WAIT` for why the wait is
                        // bounded. Here the bound also keeps the inbox draining:
                        // a loop parked forever lets the inbox reach
                        // `MAX_QUEUE_SIZE`, at which point a guest's `publish`
                        // starts failing with "message queue full".
                        let permit = tokio::select! {
                            admitted = admission.acquire_before_deadline(&component_id, &msg.subject) => {
                                match admitted {
                                    super::Admitted::Slot(permit) => permit,
                                    // Already logged and counted; drop this
                                    // message and resume draining.
                                    //
                                    // A requester is failed now rather than
                                    // left waiting out its own timeout on a
                                    // reply that is never coming — but only
                                    // when this component was the sole
                                    // subscriber. Otherwise another component
                                    // still holds the same message and may
                                    // answer, and completing the pending
                                    // request here would fail a request that
                                    // was about to succeed.
                                    super::Admitted::Shed => {
                                        if sole_subscriber
                                            && let (Some(reply_to), Some(pending)) =
                                                (&msg.reply_to, &pending_requests)
                                            && let Some(sender) =
                                                pending.write().await.remove(reply_to)
                                        {
                                            let _ = sender.send(Err(super::shed_error()));
                                        }
                                        continue;
                                    }
                                    // Closed: the component is going away.
                                    super::Admitted::Closed => break 'task,
                                }
                            }
                            _ = cancel_token.cancelled() => {
                                debug!(
                                    component_id = %component_id,
                                    "in-memory receive loop cancelled while awaiting admission"
                                );
                                break 'task;
                            }
                        };

                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_wasmcloud_message_memory",
                            subject = %msg.subject,
                            reply_to = %msg.reply_to.as_deref().unwrap_or("<none>"),
                        );

                        // As in the NATS backend: an async `@0.3.0` handler on
                        // a component that opted into pooling is served on a
                        // warm instance, honouring the limits it declared.
                        // Everything else keeps a store per message.
                        let pool = if pre.serves_async() {
                            workload.instance_pool_for_component(&component_id).await
                        } else {
                            None
                        };

                        if let Some(pool) = pool {
                            let broker = crate::host::trigger_service::BrokerMessage {
                                subject: msg.subject,
                                body: msg.body,
                                reply_to: msg.reply_to,
                            };
                            let attributes = attributes.clone();
                            let workload = workload.clone();
                            let component_id = component_id.clone();
                            let instance_pre = pre.instance_pre().clone();
                            tokio::spawn(async move {
                                // Released on completion, trap or not — which
                                // is what frees the slot this message holds.
                                let _permit = permit;
                                let result = super::deliver_pooled(
                                    &workload,
                                    &component_id,
                                    &instance_pre,
                                    &pool,
                                    broker,
                                    attributes,
                                )
                                .instrument(span)
                                .await;
                                super::log_delivery(&result);
                            });
                            continue;
                        }

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

                        let guest_meter = guest_meter.clone();

                        // As in the NATS backend: nothing awaits this call, so
                        // its deadline is a timer outliving the store's task.
                        let call = crate::engine::abandon::DispatchedCall::new(
                            "messaging (per-message store)",
                            crate::timeouts::messaging_deliver(),
                        );
                        let abandoned = store.data().abandoned.watch(call.flag());
                        let deadline = call.arm_on_timer();

                        tokio::spawn(async move {
                            // Released on completion, trap or not — which is
                            // what frees the instance slot this message holds.
                            let _permit = permit;
                            let _abandoned = abandoned;
                            let _deadline = deadline;
                            let result = guest_meter.observe(
                                &[
                                    KeyValue::new("plugin", PLUGIN_MESSAGING_MEMORY_ID),
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

                            super::log_delivery(&result);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::WorkloadTrackerItem;

    /// Build a plugin whose one workload has a component per entry in
    /// `subscriptions`, each subscribed to what it names.
    fn plugin_with(subscriptions: &[&[&str]]) -> (InMemoryMessaging, String, Vec<Inbox>) {
        let plugin = InMemoryMessaging::new();
        let workload_id = "workload-1".to_string();
        let mut item = WorkloadTrackerItem {
            workload_data: Some(WorkloadData::default()),
            components: HashMap::new(),
        };
        let mut inboxes = Vec::new();
        for (i, subs) in subscriptions.iter().enumerate() {
            let inbox: Inbox = Arc::default();
            inboxes.push(inbox.clone());
            item.components.insert(
                format!("component-{i}"),
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    task_handle: None,
                    subscriptions: subs.iter().map(|s| s.to_string()).collect(),
                    inbox,
                    notify: Arc::new(Notify::new()),
                    admission: super::super::MessagingLimits::default().admission(
                        &super::super::AdmissionIdentity::new("ns", "workload-1", &format!("c{i}")),
                        None,
                    ),
                },
            );
        }
        plugin
            .tracker
            .try_write()
            .expect("fresh tracker")
            .workloads
            .insert(workload_id.clone(), item);
        (plugin, workload_id, inboxes)
    }

    fn broker(subject: &str) -> types_p2::BrokerMessage {
        types_p2::BrokerMessage {
            subject: subject.to_string(),
            reply_to: Some("_INBOX.abc".to_string()),
            body: Vec::new(),
        }
    }

    /// Only the sole subscriber may fail a requester on shed. With more than
    /// one, the other component still holds the same message and may reply, so
    /// completing the pending request would fail a request about to succeed —
    /// which is exactly the race that made this unsafe to do over NATS, where
    /// the fan-out is not knowable from inside a subscriber.
    #[tokio::test]
    async fn fan_out_is_recorded_so_only_a_sole_subscriber_may_fast_fail() {
        let (plugin, workload_id, inboxes) = plugin_with(&[&["tasks.new"]]);
        route_to_subscribers(&plugin, &workload_id, &broker("tasks.new"))
            .await
            .expect("routes");
        assert!(
            inboxes[0].read().await[0].sole_subscriber,
            "one matching component must be marked as the sole subscriber"
        );

        // Two components on the same subject: neither may fast-fail.
        let (plugin, workload_id, inboxes) = plugin_with(&[&["tasks.new"], &["tasks.*"]]);
        route_to_subscribers(&plugin, &workload_id, &broker("tasks.new"))
            .await
            .expect("routes");
        for (i, inbox) in inboxes.iter().enumerate() {
            assert!(
                !inbox.read().await[0].sole_subscriber,
                "component {i} shares the message and must not fail the requester"
            );
        }

        // A second component that does NOT match leaves the first one sole.
        let (plugin, workload_id, inboxes) = plugin_with(&[&["tasks.new"], &["other.thing"]]);
        route_to_subscribers(&plugin, &workload_id, &broker("tasks.new"))
            .await
            .expect("routes");
        assert!(inboxes[0].read().await[0].sole_subscriber);
        assert!(
            inboxes[1].read().await.is_empty(),
            "a non-matching component must not receive the message at all"
        );
    }

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
