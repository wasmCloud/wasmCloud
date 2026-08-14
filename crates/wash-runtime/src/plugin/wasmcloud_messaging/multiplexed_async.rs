//! # Multiplexed async `wasmcloud:messaging` (implements-routed)
//!
//! The async-native counterpart to [`super::multiplexed`]. Binds
//! `wasmcloud:messaging@0.3.0`, whose `consumer.publish`/`consumer.request` are
//! `async func`s, via the same `(implements ..)` / `named_imports` mechanism,
//! routing each named import to a [`MsgBackend`].
//!
//! Because the WIT functions are `async func`s, the generated host traits use
//! wasmtime's *concurrent* ABI: methods are `async fn`s on `SharedCtx` taking an
//! [`Accessor`], rather than `&mut self` methods on `ActiveCtx`. The practical
//! difference for a guest is that `request` no longer blocks the instance — a
//! component can keep many requests in flight and continue serving other work
//! while each awaits its reply.
//!
//! The backends are shared verbatim with the sync `@0.2.0` plugin: `publish` and
//! `request` map 1:1 onto [`MsgBackend`], so there is nothing to adapt beyond
//! lowering [`MsgError`] into the WIT `error` variant and converting between the
//! two versions' `broker-message` records (structurally identical, but distinct
//! generated Rust types).

use std::collections::HashSet;
use std::sync::Arc;

use wasmtime::component::Accessor;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::Multiplexer;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

use super::multiplexed::{BrokerMessage, MsgId, MsgProvider};

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "async-messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        // Only `consumer` is routed per `(implements ..)` label; `types` carries
        // record definitions only and is bound standalone. The key is versioned
        // because both `@0.2.0` and `@0.3.0` are in the runtime's resolve graph.
        named_imports: {
            "wasmcloud:messaging/consumer@0.3.0": crate::plugin::wasmcloud_messaging::multiplexed::MsgId,
        },
    });
}

use bindings::wasmcloud::messaging0_3_0::types::{
    BrokerMessage as AsyncBrokerMessage, Error as AsyncMsgError,
};

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_ASYNC_MESSAGING_ID: &str = "wasmcloud-messaging-async-multiplexed";

super::async_messaging_conversions! {
    error: AsyncMsgError,
}

/// The shared `request` body, used by both the label-routed and plain impls:
/// drain the guest's stream (see [`super::collect_body`]), run the backend
/// request, and mint the reply's body back as a fresh `stream<u8>`.
async fn request_via<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    id: MsgId,
    subject: String,
    body: wasmtime::component::StreamReader<u8>,
    timeout_ms: Option<u32>,
) -> wasmtime::Result<Result<AsyncBrokerMessage, AsyncMsgError>> {
    let bytes = match super::collect_body(accessor, body).await? {
        Ok(bytes) => bytes,
        Err(e) => return Ok(Err(e.into())),
    };
    match id.request(subject, bytes, timeout_ms).await {
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

/// The shared `publish` body: drain the guest's stream and hand the bytes to
/// the backend.
async fn publish_via<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    id: MsgId,
    msg: AsyncBrokerMessage,
) -> wasmtime::Result<Result<(), AsyncMsgError>> {
    let AsyncBrokerMessage {
        subject,
        body,
        reply_to,
    } = msg;
    let bytes = match super::collect_body(accessor, body).await? {
        Ok(bytes) => bytes,
        Err(e) => return Ok(Err(e.into())),
    };
    Ok(id
        .publish(BrokerMessage {
            subject,
            body: bytes,
            reply_to,
        })
        .await
        .map_err(Into::into))
}

// `consumer` routed per `(implements ..)` label: the `MsgId` comes from the label.
impl<T: 'static + Send>
    bindings::named_imports::wasmcloud::messaging0_3_0::consumer::HostWithStore<T> for SharedCtx
{
    async fn request(
        accessor: &Accessor<T, Self>,
        id: MsgId,
        subject: String,
        body: wasmtime::component::StreamReader<u8>,
        timeout_ms: Option<u32>,
    ) -> wasmtime::Result<Result<AsyncBrokerMessage, AsyncMsgError>> {
        request_via(accessor, id, subject, body, timeout_ms).await
    }

    async fn publish(
        accessor: &Accessor<T, Self>,
        id: MsgId,
        msg: AsyncBrokerMessage,
    ) -> wasmtime::Result<Result<(), AsyncMsgError>> {
        publish_via(accessor, id, msg).await
    }
}

impl bindings::named_imports::wasmcloud::messaging0_3_0::consumer::Host for ActiveCtx<'_> {}

/// A plain (unlabeled) `consumer` import: route to the workload's default
/// backend (recorded on the multiplexer at bind) so a component that imports
/// `wasmcloud:messaging/consumer` *without* an `(implements ..)` label still
/// gets a working backend. Identical to the label-routed impl above but for
/// taking its `MsgId` from the default instead of the label.
impl<T: 'static + Send> bindings::wasmcloud::messaging0_3_0::consumer::HostWithStore<T>
    for SharedCtx
{
    async fn request(
        accessor: &Accessor<T, Self>,
        subject: String,
        body: wasmtime::component::StreamReader<u8>,
        timeout_ms: Option<u32>,
    ) -> wasmtime::Result<Result<AsyncBrokerMessage, AsyncMsgError>> {
        let Some(id) = default_backend(accessor).await? else {
            return no_default_backend();
        };
        request_via(accessor, id, subject, body, timeout_ms).await
    }

    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: AsyncBrokerMessage,
    ) -> wasmtime::Result<Result<(), AsyncMsgError>> {
        let Some(id) = default_backend(accessor).await? else {
            return no_default_backend();
        };
        publish_via(accessor, id, msg).await
    }
}

impl bindings::wasmcloud::messaging0_3_0::consumer::Host for ActiveCtx<'_> {}

// `types` carries only record and variant definitions — no host functions.
impl bindings::wasmcloud::messaging0_3_0::types::Host for ActiveCtx<'_> {}

/// The workload's default `wasmcloud:messaging` backend for a PLAIN (unlabeled)
/// import, recorded on the multiplexer at bind. `None` when the workload
/// declared no default (`""`) route.
async fn default_backend<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
) -> wasmtime::Result<Option<MsgId>> {
    let (plugin, workload_id) = accessor.with(|mut a| {
        let ctx = a.get();
        (
            ctx.try_get_plugin::<MultiplexedAsyncMessaging>(MULTIPLEXED_ASYNC_MESSAGING_ID),
            ctx.workload_id.clone(),
        )
    });
    let plugin = plugin?;
    Ok(plugin.mux.default_for(&workload_id))
}

/// The error a plain `consumer` call returns when the workload bound no default
/// backend — the component imported messaging plainly but nothing provides it.
fn no_default_backend<T2>() -> wasmtime::Result<Result<T2, AsyncMsgError>> {
    Ok(Err(AsyncMsgError::Other(
        "no default wasmcloud:messaging backend is bound for this component".to_string(),
    )))
}

/// A messaging [`HostPlugin`] that multiplexes async `wasmcloud:messaging/consumer`
/// across backends selected per `(implements ..)` import. Shares the
/// [`MsgBackend`] providers with [`super::multiplexed::MultiplexedMessaging`].
///
/// [`MsgBackend`]: super::multiplexed::MsgBackend
pub struct MultiplexedAsyncMessaging {
    mux: Multiplexer<MsgId>,
}

impl Default for MultiplexedAsyncMessaging {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedAsyncMessaging {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasmcloud", "messaging", DEFAULT_BACKEND),
        }
    }

    /// Register a backend provider keyed by its `backend_type()`.
    pub fn with_provider(mut self, provider: Arc<MsgProvider>) -> Self {
        self.mux = self.mux.with_provider(provider);
        self
    }

    /// Build the routing registry (host-interface name -> backend) from a
    /// component's matched messaging host interfaces.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<std::collections::HashMap<String, MsgId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedAsyncMessaging {
    fn id(&self) -> &'static str {
        MULTIPLEXED_ASYNC_MESSAGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:messaging/consumer,types@0.3.0",
            )]),
            ..Default::default()
        }
    }

    /// Take only interfaces that name the async revision. The sync
    /// [`MultiplexedMessaging`] plugin serves `@0.2.0` off the same backends and
    /// both are registered on a host at once, so this is what keeps the two from
    /// fighting over one linker instance — and, because an entry that pins no
    /// version world-matches BOTH plugins, it is also what leaves a versionless
    /// entry to the sync plugin (which is the surface a versionless declaration
    /// selects everywhere else; see [`super::declares_async_messaging`]).
    ///
    /// [`MultiplexedMessaging`]: super::MultiplexedMessaging
    fn claims(&self, interface: &WitInterface) -> bool {
        is_async_version(interface)
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // `claims` already refused everything that is not an async messaging
        // interface, so this only narrows to the package (a workload item binds
        // one plugin for one package, but filtering keeps the registry honest).
        let async_ifaces: Vec<&WitInterface> = interfaces
            .iter()
            .filter(|i| {
                i.namespace == "wasmcloud" && i.package == "messaging" && is_async_version(i)
            })
            .collect();
        if async_ifaces.is_empty() {
            return Ok(());
        }

        let registry = self.build_registry(async_ifaces.iter().copied()).await?;

        // Does the component import messaging with an `(implements ..)` label, or
        // plainly (unlabeled), or both? Bind only the matching `consumer` binding
        // so a plain-only component doesn't also get the labeled instance (and
        // vice versa) — `types` is bound standalone regardless.
        let has_labeled = async_ifaces.iter().any(|i| i.name.is_some());
        let has_plain = async_ifaces.iter().any(|i| i.name.is_none());

        // A plain import routes to the workload's default backend (the `""` route
        // from the registry), stashed on the multiplexer so the standard host impl
        // can find it — the shared mechanism every multiplexed plugin uses.
        if has_plain {
            self.mux.set_default(item.workload_id(), &registry);
        }

        let component = item.component().clone();
        let linker = item.linker();

        if has_labeled {
            bindings::named_imports::wasmcloud::messaging0_3_0::consumer::add_to_linker::<
                _,
                SharedCtx,
            >(
                linker,
                &component,
                |name| self.mux.resolve(&registry, name),
                extract_active_ctx,
            )?;
        }
        if has_plain {
            bindings::wasmcloud::messaging0_3_0::consumer::add_to_linker::<_, SharedCtx>(
                linker,
                extract_active_ctx,
            )?;
        }
        bindings::wasmcloud::messaging0_3_0::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        Ok(())
    }
}

/// Whether a matched `wasmcloud:messaging` interface names the async surface.
///
/// An interface with no version is deliberately *not* claimed: it is equally a
/// valid `@0.2.0` import, and guessing wrong binds the wrong ABI and fails at
/// instantiation with an opaque type mismatch. Declaring the version is how a
/// workload selects a surface.
pub(super) fn is_async_version(iface: &WitInterface) -> bool {
    const ASYNC_MIN: semver::Version = semver::Version::new(0, 3, 0);
    iface.version.as_ref().is_some_and(|v| *v >= ASYNC_MIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::wasmcloud_messaging::{InMemoryMsgProvider, MsgError};
    use std::collections::HashMap;

    fn msg_iface(name: Option<&str>, version: Option<&str>) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["consumer".to_string()].into_iter().collect(),
            version: version.map(|v| semver::Version::parse(v).unwrap()),
            config: HashMap::from([("backend".to_string(), "in-memory".to_string())]),
            name: name.map(String::from),
        }
    }

    fn brokered(subject: &str, body: &[u8]) -> BrokerMessage {
        BrokerMessage {
            subject: subject.to_string(),
            reply_to: None,
            body: body.to_vec(),
        }
    }

    #[test]
    fn msg_error_maps_to_async_variant() {
        assert!(matches!(
            AsyncMsgError::from(MsgError::Timeout("timed out after 5000ms".into())),
            AsyncMsgError::Timeout
        ));
        assert!(matches!(
            AsyncMsgError::from(MsgError::BrokerUnavailable("connection reset".into())),
            AsyncMsgError::BrokerUnavailable
        ));
        assert!(matches!(
            AsyncMsgError::from(MsgError::SubjectInvalid("empty subject".into())),
            AsyncMsgError::SubjectInvalid
        ));
        assert!(matches!(
            AsyncMsgError::from(MsgError::MessageTooLarge("max payload".into())),
            AsyncMsgError::MessageTooLarge
        ));
    }

    /// `other` is the one case that keeps its detail — the WIT's named cases
    /// carry no payload, so a backend-specific message has nowhere else to go.
    #[test]
    fn other_preserves_backend_detail() {
        let AsyncMsgError::Other(detail) =
            AsyncMsgError::from(MsgError::Other("no responders".into()))
        else {
            panic!("expected Other");
        };
        assert_eq!(detail, "no responders");
    }

    /// A versionless import is claimed by neither surface: it cannot be told
    /// apart from a `@0.2.0` one, and binding the wrong ABI fails opaquely at
    /// instantiation.
    #[test]
    fn only_claims_explicitly_async_versions() {
        assert!(is_async_version(&msg_iface(None, Some("0.3.0"))));
        assert!(is_async_version(&msg_iface(None, Some("0.4.0"))));
        assert!(!is_async_version(&msg_iface(None, Some("0.2.0"))));
        assert!(!is_async_version(&msg_iface(None, None)));
    }

    /// The plugin's `world()` matches a versionless entry — a missing version is
    /// compatible with every version — so `claims` is what stops this plugin
    /// taking an entry the SYNC multiplexer has to serve. Without it a
    /// versionless `(implements ..)` entry is claimed here and then bound by
    /// nobody, and the workload fails on an unresolved import.
    #[test]
    fn refuses_entries_the_sync_multiplexer_must_serve() {
        let plugin = MultiplexedAsyncMessaging::new();
        let versionless = msg_iface(Some("team-a"), None);

        assert!(
            plugin.world().includes_bidirectional(&versionless),
            "precondition: the world matches a versionless entry, which is why \
             `claims` has to refuse it"
        );
        assert!(!plugin.claims(&versionless));
        assert!(!plugin.claims(&msg_iface(Some("team-a"), Some("0.2.0"))));
        assert!(plugin.claims(&msg_iface(Some("team-a"), Some("0.3.0"))));
    }

    /// The decisive routing case: two named interfaces of the same backend type
    /// route to independent backends, so a publish on one is not seen by the
    /// other. Exercises the shared `MsgBackend` through the async plugin.
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedAsyncMessaging::new().with_provider(Arc::new(InMemoryMsgProvider));
        let interfaces = HashSet::from([
            msg_iface(Some("cluster-a"), Some("0.3.0")),
            msg_iface(Some("cluster-b"), Some("0.3.0")),
        ]);

        let registry = plugin.build_registry(&interfaces).await.unwrap();
        let a = registry.get("cluster-a").expect("a routed").clone();
        let b = registry.get("cluster-b").expect("b routed").clone();

        a.publish(brokered("tasks", b"hi")).await.unwrap();

        assert!(Arc::ptr_eq(&a, &registry.get("cluster-a").unwrap().clone()));
        assert!(!Arc::ptr_eq(&a, &b), "routes must be distinct backends");
    }

    #[tokio::test]
    async fn build_registry_errors_on_unregistered_backend() {
        let plugin = MultiplexedAsyncMessaging::new(); // no providers
        let mut iface = msg_iface(Some("x"), Some("0.3.0"));
        iface
            .config
            .insert("backend".to_string(), "nats".to_string());
        let err = plugin
            .build_registry(&HashSet::from([iface]))
            .await
            .err()
            .expect("expected error for unregistered backend");
        assert!(err.to_string().contains("nats"), "unexpected error: {err}");
    }
}
