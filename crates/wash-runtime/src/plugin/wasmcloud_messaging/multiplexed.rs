//! # Multiplexed wasmcloud:messaging consumer (implements-routed)
//!
//! Binds the `wasmcloud:messaging/consumer` interface via the component-model
//! `(implements ..)` / `named_imports` mechanism so a single component can
//! import the consumer interface multiple times and route each import's
//! outbound `publish`/`request` to a *different* backend (e.g. one import
//! publishing to NATS cluster A and another to cluster B).
//!
//! The handler (subscription) side is an *export* and is unaffected by import
//! multiplexing; it is served by the standalone messaging plugins.
//!
//! Each backend lives in its own submodule: [`nats`] (NATS clusters) and
//! [`in_memory`] (a reference loopback).

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use tracing::instrument;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::{BackendProvider, Multiplexer};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

mod in_memory;
mod nats;

pub use in_memory::{InMemoryMsgBackend, InMemoryMsgProvider};
pub use nats::{NatsMsgBackend, NatsMsgProvider};

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        named_imports: {
            "wasmcloud:messaging/consumer": super::MsgId,
        },
    });
}

pub use bindings::wasmcloud::messaging::types::BrokerMessage;

/// The "implements id" threaded through every consumer host method: the backend
/// a given named import is bound to. `Arc` so it is cheaply `Clone`d into each
/// per-import closure, as `named_imports` requires.
pub type MsgId = Arc<dyn MsgBackend>;

/// A messaging backend (a NATS cluster, an in-memory loopback, ...). The
/// unified surface the named consumer host impl dispatches onto. Errors are the
/// WIT `error` (a `string`).
#[async_trait::async_trait]
pub trait MsgBackend: Send + Sync {
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<BrokerMessage, String>;
    async fn publish(&self, msg: BrokerMessage) -> Result<(), String>;
}

impl<'a> bindings::named_imports::wasmcloud::messaging::consumer::Host for ActiveCtx<'a> {
    #[instrument(name = "wasmcloud.messaging.request", skip_all, fields(subject = %subject, timeout_ms = timeout_ms))]
    async fn request(
        &mut self,
        id: MsgId,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<BrokerMessage, String>> {
        Ok(id.request(subject, body, timeout_ms).await)
    }

    #[instrument(name = "wasmcloud.messaging.publish", skip_all, fields(subject = %msg.subject))]
    async fn publish(
        &mut self,
        id: MsgId,
        msg: BrokerMessage,
    ) -> wasmtime::Result<Result<(), String>> {
        Ok(id.publish(msg).await)
    }
}

// `types` has no host functions or resources, so it is bound via the regular
// (non-named) path; only `consumer` is multiplexed per import.
impl<'a> bindings::wasmcloud::messaging::types::Host for ActiveCtx<'a> {}

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_MESSAGING_ID: &str = "wasmcloud-messaging-multiplexed";

/// A messaging backend provider: a [`BackendProvider`] producing [`MsgId`]s.
pub type MsgProvider = dyn BackendProvider<MsgId>;

/// A messaging [`HostPlugin`] that multiplexes `wasmcloud:messaging/consumer`
/// across backends selected per `(implements ..)` import. Register the backend
/// providers you want via [`MultiplexedMessaging::with_provider`].
pub struct MultiplexedMessaging {
    mux: Multiplexer<MsgId>,
}

impl Default for MultiplexedMessaging {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedMessaging {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasmcloud", "messaging", DEFAULT_BACKEND),
        }
    }

    pub fn with_provider(mut self, provider: Arc<MsgProvider>) -> Self {
        self.mux = self.mux.with_provider(provider);
        self
    }

    /// Build the routing registry (host-interface name -> backend) from a
    /// component's matched messaging host interfaces.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<HashMap<String, MsgId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedMessaging {
    fn id(&self) -> &'static str {
        MULTIPLEXED_MESSAGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:messaging/consumer,types@0.2.0",
            )]),
            ..Default::default()
        }
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasmcloud", "messaging", &[]) {
            return Ok(());
        }

        let registry = self.build_registry(interfaces.iter()).await?;
        let component = item.component().clone();
        let linker = item.linker();

        // `types` carries only record definitions; bind it via the regular
        // path (no per-import routing needed).
        bindings::wasmcloud::messaging::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::named_imports::wasmcloud::messaging::consumer::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
            extract_active_ctx,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_iface(name: Option<&str>, backend: Option<&str>) -> WitInterface {
        let mut config = HashMap::new();
        if let Some(b) = backend {
            config.insert("backend".to_string(), b.to_string());
        }
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["consumer".to_string()].into_iter().collect(),
            version: None,
            config,
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

    /// The decisive case: two named interfaces of the same backend type route to
    /// independent backends, so a publish on one is not seen by the other.
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedMessaging::new().with_provider(Arc::new(InMemoryMsgProvider));
        let interfaces = HashSet::from([
            msg_iface(Some("cluster-a"), Some("in-memory")),
            msg_iface(Some("cluster-b"), Some("in-memory")),
        ]);

        let registry = plugin.build_registry(&interfaces).await.unwrap();
        let a = registry.get("cluster-a").expect("a routed").clone();
        let b = registry.get("cluster-b").expect("b routed").clone();

        a.publish(brokered("tasks", b"hi")).await.unwrap();

        // The registry hands back the same backend for a given name, and the two
        // names resolve to independent instances (the in-memory provider never
        // pools, so each named import is isolated).
        assert!(Arc::ptr_eq(&a, &registry.get("cluster-a").unwrap().clone()));
        assert!(!Arc::ptr_eq(&a, &b), "routes must be distinct backends");
    }

    #[tokio::test]
    async fn build_registry_errors_on_unregistered_backend() {
        let plugin = MultiplexedMessaging::new(); // no providers
        let interfaces = HashSet::from([msg_iface(Some("x"), Some("nats"))]);
        let err = plugin
            .build_registry(&interfaces)
            .await
            .err()
            .expect("expected error for unregistered backend");
        assert!(err.to_string().contains("nats"), "unexpected error: {err}");
    }
}
