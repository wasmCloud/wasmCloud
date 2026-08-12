mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed_async;
mod nats;

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use in_memory::InMemoryMessaging;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::{
    BrokerMessage, InMemoryMsgBackend, InMemoryMsgProvider, MsgBackend, MsgId, MsgProvider,
    MultiplexedMessaging, NatsMsgBackend, NatsMsgProvider,
};
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed_async::MultiplexedAsyncMessaging;
pub use nats::NatsMessaging;

/// A messaging failure, classified into the named cases of the async
/// `wasmcloud:messaging@0.3.0` `error` variant.
///
/// The two messaging surfaces disagree on how an error is spelled: `@0.2.0`
/// returns a bare `string`, `@0.3.0` a non-exhaustive variant. Producers of an
/// error therefore classify once, into this type, and each host impl lowers it —
/// to a string via [`Display`](std::fmt::Display) for the sync path, into the WIT
/// variant for the async one (see `multiplexed_async`).
///
/// Every case carries the human-readable detail. The sync path prints it, so its
/// error strings are unchanged from before this type existed; the async path
/// keeps it only for [`MsgError::Other`], since the WIT's named cases have no
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgError {
    /// The subject is malformed, empty, or otherwise rejected by the broker.
    SubjectInvalid(String),
    /// The component is not permitted to publish to this subject.
    AccessDenied(String),
    /// A `request` did not receive a reply within its `timeout_ms`.
    Timeout(String),
    /// The broker is unreachable or otherwise unavailable.
    BrokerUnavailable(String),
    /// The message body exceeded the broker's maximum payload size.
    MessageTooLarge(String),
    /// A quota or rate limit was exceeded.
    QuotaExceeded(String),
    /// A backend-specific failure that maps to no named case above.
    Other(String),
}

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            MsgError::SubjectInvalid(d)
            | MsgError::AccessDenied(d)
            | MsgError::Timeout(d)
            | MsgError::BrokerUnavailable(d)
            | MsgError::MessageTooLarge(d)
            | MsgError::QuotaExceeded(d)
            | MsgError::Other(d) => d,
        };
        f.write_str(detail)
    }
}

impl std::error::Error for MsgError {}

/// The lowering used by the sync `@0.2.0` surface, whose WIT `error` is a `string`.
impl From<MsgError> for String {
    fn from(e: MsgError) -> Self {
        e.to_string()
    }
}

/// Expands the [`MsgError`] lowering into one `bindgen!`-generated `@0.3.0`
/// `error` type.
///
/// Each messaging plugin has its own `bindgen!` invocation — they implement the
/// generated host traits for different backends, so they cannot share one — and
/// each therefore gets its own `error` Rust type even though the WIT is a single
/// package. The lowering is identical in every case, so it is written once here
/// and expanded per module rather than copied three times and left to drift.
///
/// Note this is the only capability that needs such a thing, which is why no
/// sibling has one: `wasmcloud:keyvalue` and `wasmcloud:blobstore` asyncified
/// only their *multiplexed* plugin, leaving one async `bindgen!` apiece and so
/// nothing to share (keyvalue's exported `watcher` is declared in WIT but never
/// served host-side). Messaging asyncifies all three plugins, because its
/// exported `handler` — the subscription path, served by the standalone NATS and
/// in-memory plugins that `wash dev` and `wash host` register — is the half of
/// the capability that receives messages.
///
/// The named WIT cases carry no payload, so their host-side detail is dropped by
/// the lowering; it survives only on `other`.
macro_rules! async_messaging_conversions {
    (error: $async_error:ty $(,)?) => {
        impl From<$crate::plugin::wasmcloud_messaging::MsgError> for $async_error {
            fn from(e: $crate::plugin::wasmcloud_messaging::MsgError) -> Self {
                use $crate::plugin::wasmcloud_messaging::MsgError as E;
                match e {
                    E::SubjectInvalid(_) => Self::SubjectInvalid,
                    E::AccessDenied(_) => Self::AccessDenied,
                    E::Timeout(_) => Self::Timeout,
                    E::BrokerUnavailable(_) => Self::BrokerUnavailable,
                    E::MessageTooLarge(_) => Self::MessageTooLarge,
                    E::QuotaExceeded(_) => Self::QuotaExceeded,
                    E::Other(d) => Self::Other(d),
                }
            }
        }
    };
}

pub(crate) use async_messaging_conversions;

/// Drain a `@0.3.0` message body (`stream<u8>`) into memory.
///
/// The `@0.3.0` WIT carries bodies as native streams, but every current backend
/// sends a message as one bounded payload (core NATS caps it at `max_payload`),
/// so the host consumes the guest's stream fully before handing bytes to the
/// backend — the buffered fallback the WIT documents. A backend that can
/// forward a stream incrementally can bypass this helper.
///
/// One helper serves every plugin because [`StreamReader`] is a wasmtime type,
/// not a `bindgen!`-generated one. An `Err` from the oneshot means the consumer
/// was torn down without observing end-of-stream, which is surfaced as a
/// [`MsgError`] rather than silently treating the body as empty.
///
/// Collection is bounded by [`MAX_COLLECTED_BODY_BYTES`]: past the cap it fails
/// with `message-too-large` without buffering the excess, so a guest streaming
/// unboundedly cannot balloon host memory — the send-side broker limit (NATS
/// `max_payload`) only rejects a message AFTER it is fully in memory, which is
/// too late to be the bound.
pub(crate) async fn collect_body<T, D>(
    accessor: &wasmtime::component::Accessor<T, D>,
    body: wasmtime::component::StreamReader<u8>,
) -> wasmtime::Result<Result<Vec<u8>, MsgError>>
where
    T: 'static,
    D: wasmtime::component::HasData,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, MsgError>>();
    accessor.with(|mut a| {
        body.pipe(
            &mut a,
            CollectConsumer {
                buf: Vec::new(),
                limit: MAX_COLLECTED_BODY_BYTES,
                done: Some(tx),
            },
        )
    })?;
    Ok(match rx.await {
        Ok(outcome) => outcome,
        Err(_) => Err(MsgError::Other(
            "message body stream ended without delivering data".to_string(),
        )),
    })
}

/// Upper bound on a collected message body, bounding host memory against a
/// guest that streams unboundedly. Deliberately above any sane broker payload
/// limit (NATS `max_payload` defaults to 1 MiB and tops out well below this),
/// so the broker's own limit stays the effective one for sendable messages and
/// this cap only stops runaway streams.
const MAX_COLLECTED_BODY_BYTES: usize = 16 * 1024 * 1024;

/// A [`StreamConsumer`] that accumulates every byte the guest writes and hands
/// the buffer back once the stream ends. The runtime drops the consumer at
/// end-of-stream, which fires [`Drop`] and delivers the bytes over `done`.
/// Mirrors the blobstore `write-data` consumer.
///
/// [`StreamConsumer`]: wasmtime::component::StreamConsumer
struct CollectConsumer {
    buf: Vec<u8>,
    limit: usize,
    done: Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, MsgError>>>,
}

impl Drop for CollectConsumer {
    fn drop(&mut self) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(Ok(std::mem::take(&mut self.buf)));
        }
    }
}

impl<D> wasmtime::component::StreamConsumer<D> for CollectConsumer {
    type Item = u8;

    fn poll_consume(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        store: wasmtime::StoreContextMut<D>,
        src: wasmtime::component::Source<Self::Item>,
        finish: bool,
    ) -> std::task::Poll<wasmtime::Result<wasmtime::component::StreamResult>> {
        use wasmtime::component::StreamResult;
        let this = self.get_mut();
        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            // No items offered (count == 0). This is an unbounded in-memory
            // sink, so it is always ready to accept; the actual end-of-stream
            // is observed via `Drop`.
            return std::task::Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        let n = bytes.len();
        if this.buf.len().saturating_add(n) > this.limit {
            // Refuse the excess instead of buffering it; delivering the error
            // here (rather than via `Drop`) is what lets the caller see
            // `message-too-large` instead of a truncated body.
            if let Some(tx) = this.done.take() {
                let _ = tx.send(Err(MsgError::MessageTooLarge(format!(
                    "message body exceeded the host collection limit of {} bytes",
                    this.limit
                ))));
            }
            return std::task::Poll::Ready(Ok(StreamResult::Cancelled));
        }
        this.buf.extend_from_slice(bytes);
        src.mark_read(n);
        std::task::Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// Mint a `stream<u8>` carrying `bytes`, for handing a message body to a guest.
pub(crate) fn mint_body<T, D>(
    accessor: &wasmtime::component::Accessor<T, D>,
    bytes: Vec<u8>,
) -> wasmtime::Result<wasmtime::component::StreamReader<u8>>
where
    T: 'static,
    D: wasmtime::component::HasData,
{
    accessor.with(|mut a| wasmtime::component::StreamReader::new(&mut a, bytes))
}

/// Expands the per-message handler dispatch shared by the standalone plugins.
///
/// A subscriber loop pre-instantiates the component once and then invokes its
/// `handle-message` export per message. Which export that is depends on the
/// revision the guest was built against, so this generates a two-variant
/// `HandlerPre`/`HandlerProxy` pair that resolves the revision once, at
/// pre-instantiation, and dispatches on it thereafter.
///
/// Both standalone plugins need exactly this over their own `bindgen!` types
/// (see [`async_messaging_conversions`] for why the types are per-module), so it
/// is written once here.
macro_rules! messaging_handler_dispatch {
    (sync: $sync:ident, async: $async:ident $(,)?) => {
        /// A pre-instantiated messaging handler component, at whichever revision
        /// of `wasmcloud:messaging/handler` it exports.
        enum HandlerPre {
            V0_2($sync::MessagingPre<$crate::engine::ctx::SharedCtx>),
            V0_3($async::AsyncMessagingPre<$crate::engine::ctx::SharedCtx>),
        }

        /// An instantiated handler, ready to receive one message.
        enum HandlerProxy {
            V0_2($sync::Messaging),
            V0_3($async::AsyncMessaging),
        }

        impl HandlerPre {
            /// Resolve the exported handler revision, preferring the async
            /// `@0.3.0` one. `MessagingPre::new` is what actually type-checks the
            /// export against a revision, so trying `@0.3.0` first and falling
            /// back is both the check and the selection.
            fn new(
                instance_pre: wasmtime::component::InstancePre<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<Self> {
                match $async::AsyncMessagingPre::new(instance_pre.clone()) {
                    Ok(p) => {
                        // Dual export: `@0.3.0` wins; warn so the dead `@0.2.0`
                        // export is visible rather than silently ignored.
                        if $sync::MessagingPre::new(instance_pre).is_ok() {
                            tracing::warn!(
                                served = "wasmcloud:messaging/handler@0.3.0",
                                ignored = "wasmcloud:messaging/handler@0.2.0",
                                "component exports both messaging handler revisions; only \
                                 the @0.3.0 handler will be invoked — export exactly one \
                                 messaging handler revision"
                            );
                        }
                        Ok(HandlerPre::V0_3(p))
                    }
                    Err(async_err) => match $sync::MessagingPre::new(instance_pre) {
                        Ok(p) => Ok(HandlerPre::V0_2(p)),
                        // Report the `@0.3.0` failure too: a guest that meant to
                        // export the async handler but got it subtly wrong would
                        // otherwise only ever be reported against `@0.2.0`.
                        Err(sync_err) => Err(sync_err.context(format!(
                            "component exports neither wasmcloud:messaging/handler@0.3.0 \
                             ({async_err:#}) nor @0.2.0"
                        ))),
                    },
                }
            }

            async fn instantiate(
                &self,
                store: &mut wasmtime::Store<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<HandlerProxy> {
                match self {
                    HandlerPre::V0_2(p) => p.instantiate_async(store).await.map(HandlerProxy::V0_2),
                    HandlerPre::V0_3(p) => p.instantiate_async(store).await.map(HandlerProxy::V0_3),
                }
            }
        }

        impl HandlerProxy {
            /// Deliver one message, normalizing the handler's `result` to a
            /// `Result<(), String>` for the ack/log path — `@0.2.0` already
            /// returns a string, `@0.3.0` a `handle-message-error` disposition
            /// rendered by [`render_handle_error`].
            async fn call_handle_message(
                &self,
                store: &mut wasmtime::Store<$crate::engine::ctx::SharedCtx>,
                msg: &types_p2::BrokerMessage,
            ) -> wasmtime::Result<Result<(), String>> {
                match self {
                    HandlerProxy::V0_2(proxy) => {
                        proxy
                            .wasmcloud_messaging0_2_0_handler()
                            .call_handle_message(store, msg)
                            .await
                    }
                    // An `async func` export binds through the concurrent ABI,
                    // so it takes an `Accessor` and must be driven inside
                    // `run_concurrent` — which also lets the guest await its own
                    // imports (e.g. replying via `consumer.publish`) while the
                    // host keeps the store pumping. The `@0.3.0` body is a
                    // native `stream<u8>`, so the delivered bytes are minted
                    // into a stream the guest drains.
                    HandlerProxy::V0_3(proxy) => {
                        let (subject, body, reply_to) =
                            (msg.subject.clone(), msg.body.clone(), msg.reply_to.clone());
                        let outcome = store
                            .run_concurrent(async move |accessor| {
                                let body =
                                    $crate::plugin::wasmcloud_messaging::mint_body(accessor, body)?;
                                let wit_msg =
                                    $async::wasmcloud::messaging0_3_0::types::BrokerMessage {
                                        subject,
                                        body,
                                        reply_to,
                                    };
                                proxy
                                    .wasmcloud_messaging0_3_0_handler()
                                    .call_handle_message(accessor, wit_msg)
                                    .await
                            })
                            .await??;
                        Ok(outcome.map_err(render_handle_error))
                    }
                }
            }
        }

        /// Render the `@0.3.0` handler disposition for the ack/log path:
        /// payload-less cases as the case name, `other` keeping its detail.
        fn render_handle_error(
            e: $async::wasmcloud::messaging0_3_0::types::HandleMessageError,
        ) -> String {
            use $async::wasmcloud::messaging0_3_0::types::HandleMessageError as E;
            match e {
                E::Reject => "reject".to_string(),
                E::Retry => "retry".to_string(),
                E::Other(d) => format!("other: {d}"),
            }
        }
    };
}

pub(crate) use messaging_handler_dispatch;

/// What an unset `maxInFlight` resolves to: how many messages one component
/// may process at once.
///
/// A messaging-triggered component gets a fresh instance per message, so this
/// is equally a ceiling on instances. 32 of a Componentize-Go component (the
/// worst measured shape, at 5 core instances each) is 160 core instances —
/// a bound on one workload's blast radius, well inside the host's pool.
pub const DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT: usize = 32;

/// What the host-wide ceiling defaults to: how many messages *every* messaging
/// component on this host may process at once, added together.
///
/// The per-component ceiling alone does not bound the host — twenty components
/// at 32 is 640 in flight, which at 5 core instances apiece is 3200 against a
/// pool of 1000 ([`crate::engine`] sizes `total_core_instances` from
/// `max_instances`, default 1000). Sizing against the worst measured component
/// shape, 1000 / 5 = 200 is the real ceiling; 128 sits below it and keeps
/// roughly a third of the pool for HTTP-triggered work, warm pools, and
/// long-lived services — the workloads that otherwise fail to *start* when a
/// messaging burst drains the pool.
///
/// This is a bound, not a reservation: nothing is preallocated, so the default
/// costs nothing on a host that never bursts.
pub const DEFAULT_MAX_IN_FLIGHT_HOST: usize = 128;

/// The two-level admission ceiling on messaging-triggered work, built once per
/// host and shared by every messaging backend on it.
///
/// Both levels are needed and neither subsumes the other: the per-component
/// ceiling stops one runaway workload taking the host, and the host-wide
/// ceiling stops a crowd of components each sitting inside its own limit from
/// adding up to more than the pool can carry.
#[derive(Clone, Debug)]
pub struct MessagingLimits {
    /// Shared across every component on this host. Cloned into each
    /// subscriber loop's [`Admission`].
    host: Arc<Semaphore>,
    host_total: usize,
    per_component_default: usize,
}

impl Default for MessagingLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_IN_FLIGHT_HOST,
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
        )
    }
}

impl MessagingLimits {
    /// Build the host's messaging ceilings.
    ///
    /// Both arguments are clamped to at least 1: a zero would mean "process no
    /// messages", which is never what an operator meant. The CLI rejects an
    /// explicit zero outright (see `wash`'s config layer) so this is a
    /// belt-and-braces floor for programmatic callers.
    pub fn new(host_total: usize, per_component_default: usize) -> Self {
        let host_total = host_total.max(1);
        let per_component_default = per_component_default.max(1);
        if per_component_default > host_total {
            // Harmless — the host semaphore gates first regardless — but
            // almost certainly an operator mixing the two knobs up. Mirrors
            // the warning `connection_quotas` emits for the same shape.
            tracing::warn!(
                per_component_default,
                host_total,
                "the per-component messaging ceiling exceeds the host-wide total; \
                 the host-wide cap will gate first"
            );
        }
        Self {
            host: Arc::new(Semaphore::new(host_total)),
            host_total,
            per_component_default,
        }
    }

    /// The host-wide ceiling.
    pub fn host_total(&self) -> usize {
        self.host_total
    }

    /// What an unset component field resolves to.
    pub fn per_component_default(&self) -> usize {
        self.per_component_default
    }

    /// Resolve one component's wire value into its admission gate.
    ///
    /// `max_in_flight` is the wire field, where non-positive spells "unset"
    /// exactly as the other instance limits do. A component asking for more
    /// than the host-wide total is clamped to it rather than rejected: the host
    /// semaphore would gate first anyway, so honouring the larger number would
    /// only mislead whoever reads it back.
    pub(crate) fn admission(&self, max_in_flight: i32) -> Admission {
        let requested = usize::try_from(max_in_flight)
            .ok()
            .filter(|v| *v > 0)
            .unwrap_or(self.per_component_default);
        let resolved = requested.min(self.host_total);
        Admission {
            component: Arc::new(Semaphore::new(resolved)),
            host: Arc::clone(&self.host),
            limit: resolved,
        }
    }
}

/// One component's admission gate: its own ceiling plus the shared host one.
#[derive(Clone, Debug)]
pub(crate) struct Admission {
    component: Arc<Semaphore>,
    host: Arc<Semaphore>,
    limit: usize,
}

impl Admission {
    /// Take one slot, parking until both levels have room.
    ///
    /// Returns `None` once either semaphore is closed, which is how a
    /// shutting-down component tells its subscriber loop to stop.
    ///
    /// **Order matters: component first, then host.** A task holding a *host*
    /// permit while it waited for a component permit would block every other
    /// component on the host; holding a component permit while waiting for the
    /// host's only throttles the component that is already at its own limit.
    /// There is no deadlock either way — two semaphores, one consistent order,
    /// no cycle — but only this order confines the head-of-line blocking to
    /// the workload responsible for it.
    ///
    /// Cancel-safe: dropping the returned future before it resolves releases
    /// whichever permit it had already taken.
    pub(crate) async fn acquire(&self) -> Option<AdmissionPermit> {
        let component = Arc::clone(&self.component).acquire_owned().await.ok()?;
        let host = Arc::clone(&self.host).acquire_owned().await.ok()?;
        Some(AdmissionPermit {
            _component: component,
            _host: host,
        })
    }

    /// The resolved per-component ceiling, after defaulting and clamping.
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }
}

/// Both permits for one in-flight message, released together on drop.
///
/// Held by the spawned handler task, so a slot is freed when the handler
/// returns — whether it succeeded, failed, or trapped — and equally on the
/// error paths between admission and spawn, where this is simply a local that
/// falls out of scope.
#[derive(Debug)]
pub(crate) struct AdmissionPermit {
    _component: OwnedSemaphorePermit,
    _host: OwnedSemaphorePermit,
}

/// Returns `true` if the world exports the `wasmcloud:messaging/handler`
/// interface at any version. Matches via [`WitInterface::contains`] rather
/// than set equality, so an exported `handler@0.2.x` is recognized no matter
/// which exact version the component was built against.
pub(crate) fn exports_messaging_handler(world: &crate::wit::WitWorld) -> bool {
    let handler = crate::wit::WitInterface::from("wasmcloud:messaging/handler");
    world.exports.iter().any(|e| e.contains(&handler))
}

/// Returns `true` if the workload declared its `wasmcloud:messaging` host
/// interface at the async revision (`>= 0.3.0`).
///
/// A versionless declaration is deliberately *not* async: it cannot be told
/// apart from a `@0.2.0` one, and binding the wrong ABI fails at instantiation
/// with an opaque type mismatch. Declaring the version is how a workload selects
/// a surface, and defaulting to sync keeps pre-`@0.3.0` workloads working.
pub(crate) fn declares_async_messaging(interfaces: &crate::plugin::WitInterfaces<'_>) -> bool {
    const ASYNC_MIN: semver::Version = semver::Version::new(0, 3, 0);
    interfaces.iter().any(|i| {
        i.namespace == "wasmcloud"
            && i.package == "messaging"
            && i.version.as_ref().is_some_and(|v| *v >= ASYNC_MIN)
    })
}

/// Parses a comma-separated `subscriptions` config value into trimmed,
/// non-empty subjects. Shared by the in-memory and NATS backends so they
/// agree on how a configured subscription string maps to subjects.
pub(crate) fn parse_subscriptions(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        MsgError, declares_async_messaging, exports_messaging_handler, parse_subscriptions,
    };
    use crate::plugin::WitInterfaces;
    use crate::wit::{WitInterface, WitWorld};
    use std::collections::HashSet;

    fn messaging_iface(version: Option<&str>) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["consumer".to_string()].into_iter().collect(),
            version: version.map(|v| semver::Version::parse(v).unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        }
    }

    /// Which surface a workload gets is decided by the version it declares.
    #[test]
    fn async_surface_selected_by_declared_version() {
        for (version, expected) in [
            (Some("0.3.0"), true),
            (Some("0.4.0"), true),
            (Some("0.2.0"), false),
        ] {
            let set = HashSet::from([messaging_iface(version)]);
            assert_eq!(
                declares_async_messaging(&WitInterfaces::new(&set)),
                expected,
                "version {version:?}"
            );
        }
    }

    /// A versionless declaration must resolve to the SYNC surface: it is
    /// indistinguishable from a `@0.2.0` one, and defaulting the other way would
    /// silently break every workload written before `@0.3.0` existed.
    #[test]
    fn versionless_declaration_defaults_to_sync() {
        let set = HashSet::from([messaging_iface(None)]);
        assert!(!declares_async_messaging(&WitInterfaces::new(&set)));
    }

    /// An unrelated package must never flip the messaging surface.
    #[test]
    fn other_packages_do_not_select_the_async_surface() {
        let mut iface = messaging_iface(Some("0.3.0"));
        iface.package = "keyvalue".to_string();
        let set = HashSet::from([iface]);
        assert!(!declares_async_messaging(&WitInterfaces::new(&set)));
    }

    /// The sync `@0.2.0` surface prints the classified error's detail verbatim,
    /// so its error strings are unchanged from before `MsgError` existed.
    #[test]
    fn sync_lowering_preserves_error_text() {
        let text: String = MsgError::Timeout("request timed out after 5000ms".into()).into();
        assert_eq!(text, "request timed out after 5000ms");
        let text: String =
            MsgError::BrokerUnavailable("failed to send message: broken pipe".into()).into();
        assert_eq!(text, "failed to send message: broken pipe");
    }

    #[test]
    fn recognizes_exported_handler_at_any_version() {
        for export in [
            "wasmcloud:messaging/handler",
            "wasmcloud:messaging/handler@0.2.0",
            "wasmcloud:messaging/handler@0.2.2",
        ] {
            let world = WitWorld {
                imports: HashSet::new(),
                exports: HashSet::from([WitInterface::from(export)]),
            };
            assert!(exports_messaging_handler(&world), "should match {export}");
        }
    }

    #[test]
    fn ignores_non_handler_worlds() {
        // Importing the handler is not exporting it
        let importer = WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.2.0")]),
            exports: HashSet::new(),
        };
        assert!(!exports_messaging_handler(&importer));

        // Exporting other messaging interfaces does not count
        let consumer = WitWorld {
            imports: HashSet::new(),
            exports: HashSet::from([WitInterface::from("wasmcloud:messaging/consumer,types")]),
        };
        assert!(!exports_messaging_handler(&consumer));
    }

    #[test]
    fn parses_single_subject() {
        assert_eq!(
            parse_subscriptions(Some("tasks.task-worker")),
            vec!["tasks.task-worker".to_string()]
        );
    }

    #[test]
    fn parses_multiple_subjects() {
        assert_eq!(
            parse_subscriptions(Some("a,b,c")),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn trims_surrounding_whitespace_and_drops_empties() {
        assert_eq!(
            parse_subscriptions(Some(" tasks.leet , tasks.reverse ,, ")),
            vec!["tasks.leet".to_string(), "tasks.reverse".to_string()]
        );
        assert!(parse_subscriptions(None).is_empty());
    }

    // --- Admission ceilings -------------------------------------------------

    use super::{DEFAULT_MAX_IN_FLIGHT_HOST, DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT, MessagingLimits};
    use futures::FutureExt as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn unset_resolves_to_the_per_component_default() {
        let limits = MessagingLimits::default();
        // Non-positive is how the wire spells "unset", for all three of the
        // signed limits. Every spelling must land on the same default.
        for unset in [0, -1, i32::MIN] {
            assert_eq!(
                limits.admission(unset).limit(),
                DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
                "{unset} should resolve to the default"
            );
        }
    }

    #[test]
    fn an_explicit_value_is_honored() {
        let limits = MessagingLimits::new(128, 32);
        assert_eq!(limits.admission(4).limit(), 4);
        assert_eq!(limits.admission(64).limit(), 64);
    }

    #[test]
    fn a_component_may_not_exceed_the_host_total() {
        // Clamped rather than rejected: the host semaphore gates first anyway,
        // so honouring the larger number would only mislead whoever reads it.
        let limits = MessagingLimits::new(16, 32);
        assert_eq!(limits.admission(1024).limit(), 16);
        // ...including via the default, when the default itself is the larger.
        assert_eq!(limits.admission(0).limit(), 16);
    }

    #[test]
    fn zero_ceilings_floor_to_one_rather_than_meaning_unbounded() {
        // The CLI rejects an explicit zero outright; this is the belt-and-braces
        // floor for programmatic callers. Zero must never read as "no limit".
        let limits = MessagingLimits::new(0, 0);
        assert_eq!(limits.host_total(), 1);
        assert_eq!(limits.per_component_default(), 1);
        assert_eq!(limits.admission(0).limit(), 1);
    }

    #[test]
    fn defaults_are_the_documented_pair() {
        let limits = MessagingLimits::default();
        assert_eq!(limits.host_total(), DEFAULT_MAX_IN_FLIGHT_HOST);
        assert_eq!(
            limits.per_component_default(),
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT
        );
        // The pairing the sizing arithmetic assumes: the per-component ceiling
        // is below the host total, so several components fit before the host
        // binds.
        const {
            assert!(DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT < DEFAULT_MAX_IN_FLIGHT_HOST);
        }
    }

    #[tokio::test]
    async fn admission_bounds_one_component() {
        let limits = MessagingLimits::new(64, 32);
        let admission = limits.admission(3);

        // Bound individually rather than collected: all three must be held at
        // once, and the release below must drop exactly one of them.
        let admit = || {
            admission
                .acquire()
                .now_or_never()
                .flatten()
                .expect("a free slot should admit immediately")
        };
        let first = admit();
        let _second = admit();
        let _third = admit();

        // Fourth must park rather than admit.
        assert!(
            admission.acquire().now_or_never().is_none(),
            "the ceiling must bind at 3"
        );

        // Releasing ONE frees exactly one slot.
        drop(first);
        assert!(
            admission.acquire().now_or_never().flatten().is_some(),
            "a completed handler must free its slot"
        );
    }

    /// The assertion the per-component test cannot make: three components each
    /// allowed 32 must still peak at the host total across all three, not 96.
    #[tokio::test]
    async fn the_host_total_bounds_every_component_together() {
        let limits = MessagingLimits::new(4, 32);
        let components: Vec<_> = (0..3).map(|_| limits.admission(32)).collect();

        let mut held = Vec::new();
        // Round-robin so no single component could have taken all four.
        for round in 0..4 {
            let a = &components[round % components.len()];
            held.push(
                a.acquire()
                    .now_or_never()
                    .flatten()
                    .expect("within the host total"),
            );
        }

        for (i, a) in components.iter().enumerate() {
            assert!(
                a.acquire().now_or_never().is_none(),
                "component {i} must be blocked by the exhausted host total, \
                 despite having its own slots free"
            );
        }

        drop(held.pop());
        assert!(
            components[0].acquire().now_or_never().flatten().is_some(),
            "freeing a host slot must admit again"
        );
    }

    #[tokio::test]
    async fn a_permit_frees_both_levels() {
        // A host-permit leak is the worst failure mode here: it silently lowers
        // the ceiling for every component on the host, not just the leaker.
        let limits = MessagingLimits::new(1, 1);
        let a = limits.admission(1);
        let b = limits.admission(1);

        let permit = a.acquire().await.expect("first admits");
        assert!(
            b.acquire().now_or_never().is_none(),
            "the host total is exhausted by the other component"
        );

        drop(permit);
        assert!(
            b.acquire().now_or_never().flatten().is_some(),
            "dropping the permit must free the host slot too, not just the component one"
        );
    }

    #[tokio::test]
    async fn acquire_is_cancel_safe() {
        // Cancelling an acquire that is parked on the HOST semaphore must not
        // strand the component permit it already took. If it did, every
        // shutdown race would erode that component's ceiling by one.
        //
        // Host total 2 with a per-component default of 8 gives each component a
        // resolved ceiling of 2 (clamped), so the blocker can exhaust the host
        // while the waiter still has component slots free — the only shape in
        // which a waiter parks on the host level rather than its own.
        let limits = MessagingLimits::new(2, 8);
        let blocker = limits.admission(8);
        let waiter = limits.admission(8);
        assert_eq!(waiter.limit(), 2, "clamped to the host total");

        let held: Vec<_> = (0..2)
            .map(|_| blocker.acquire().now_or_never().flatten().expect("admits"))
            .collect();

        // Box::pin, not tokio::pin!: the latter rebinds the name to a
        // `Pin<&mut F>`, so dropping it would drop the *reference* and leave
        // the future — and its permit — alive on the stack.
        let mut parked = Box::pin(waiter.acquire());
        assert!(
            futures::poll!(parked.as_mut()).is_pending(),
            "the waiter has its own slots free, so it must be the host level parking it"
        );
        drop(parked);

        // With the host freed, the waiter must get its FULL ceiling back. One
        // short would mean the cancelled future kept its component permit.
        drop(held);
        let leak_check = || {
            waiter
                .acquire()
                .now_or_never()
                .flatten()
                .expect("no permit should have leaked from the cancelled acquire")
        };
        // Both held simultaneously: taking them one at a time would pass even
        // if only a single slot had come back.
        let (_a, _b) = (leak_check(), leak_check());
    }

    #[tokio::test]
    async fn a_closed_semaphore_ends_the_loop() {
        // How a shutting-down component tells its subscriber loop to stop.
        let limits = MessagingLimits::new(4, 4);
        let admission = limits.admission(4);
        admission.component.close();
        assert!(
            admission.acquire().await.is_none(),
            "a closed component semaphore must report None, not park forever"
        );
    }

    #[tokio::test]
    async fn permits_are_handed_out_in_arrival_order() {
        let limits = MessagingLimits::new(8, 1);
        let admission = limits.admission(1);
        let held = admission.acquire().await.expect("first admits");

        let order = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..3 {
            let admission = admission.clone();
            let order = Arc::clone(&order);
            // Stagger so the waiters queue in a known order.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            handles.push(tokio::spawn(async move {
                let _p = admission.acquire().await.expect("eventually admitted");
                (i, order.fetch_add(1, Ordering::SeqCst))
            }));
        }

        drop(held);
        let mut seen: Vec<(usize, usize)> = Vec::new();
        for h in handles {
            seen.push(h.await.expect("waiter task"));
        }
        seen.sort_by_key(|(_, position)| *position);
        assert_eq!(
            seen.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "tokio's Semaphore is FIFO-fair, so a hot component cannot starve the rest"
        );
    }
}
