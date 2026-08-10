mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed_async;
mod nats;

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
pub(crate) async fn collect_body<T, D>(
    accessor: &wasmtime::component::Accessor<T, D>,
    body: wasmtime::component::StreamReader<u8>,
) -> wasmtime::Result<Result<Vec<u8>, MsgError>>
where
    T: 'static,
    D: wasmtime::component::HasData,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    accessor.with(|mut a| {
        body.pipe(
            &mut a,
            CollectConsumer {
                buf: Vec::new(),
                done: Some(tx),
            },
        )
    })?;
    Ok(match rx.await {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(MsgError::Other(
            "message body stream ended without delivering data".to_string(),
        )),
    })
}

/// A [`StreamConsumer`] that accumulates every byte the guest writes and hands
/// the buffer back once the stream ends. The runtime drops the consumer at
/// end-of-stream, which fires [`Drop`] and delivers the bytes over `done`.
/// Mirrors the blobstore `write-data` consumer.
///
/// [`StreamConsumer`]: wasmtime::component::StreamConsumer
struct CollectConsumer {
    buf: Vec<u8>,
    done: Option<tokio::sync::oneshot::Sender<Vec<u8>>>,
}

impl Drop for CollectConsumer {
    fn drop(&mut self) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(std::mem::take(&mut self.buf));
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
}
