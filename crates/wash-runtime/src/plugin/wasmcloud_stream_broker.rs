//! Tiny per-workload stream broker for demos that need separate component
//! invocations to rendezvous around a long-lived client connection.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    engine::{
        ctx::{ActiveCtx, SharedCtx, extract_active_ctx},
        workload::WorkloadItem,
    },
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};
use tokio::sync::{Mutex, RwLock, broadcast};

const PLUGIN_STREAM_BROKER_ID: &str = "wasmcloud-stream-broker";
const BROKER_CHANNEL_CAPACITY: usize = 1024;

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "broker-host",
        imports: { default: async | trappable | tracing },
        inline: "
            package wasmcloud:patch-stream@0.1.0;

            interface broker {
                register: func() -> u64;
                unregister: func(client-id: u64);
                wait-message: async func(client-id: u64) -> option<string>;
                publish-message: async func(message: string) -> result<_, string>;
            }

            world broker-host {
                import broker;
            }
        ",
    });
}

struct WorkloadBroker {
    next_client_id: u64,
    tx: broadcast::Sender<String>,
    clients: HashMap<u64, Arc<Mutex<broadcast::Receiver<String>>>>,
}

impl Default for WorkloadBroker {
    fn default() -> Self {
        let (tx, _rx) = broadcast::channel(BROKER_CHANNEL_CAPACITY);
        Self {
            next_client_id: 1,
            tx,
            clients: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub struct StreamBroker {
    workloads: Arc<RwLock<HashMap<String, WorkloadBroker>>>,
}

impl StreamBroker {
    pub fn new() -> Self {
        Self::default()
    }
}

// `register` / `unregister` are sync in the WIT but bindgen wraps
// every import in async because the macro is configured with
// `default: async`. We `.await` real RwLock guards rather than
// `blocking_write` — the latter panics because we're already on a
// tokio worker driving other async tasks for this same store.
impl<'a> bindings::wasmcloud::patch_stream::broker::Host for ActiveCtx<'a> {
    async fn register(&mut self) -> wasmtime::Result<u64> {
        let (workloads_arc, workload_id) = {
            let Some(plugin) = self.get_plugin::<StreamBroker>(PLUGIN_STREAM_BROKER_ID) else {
                wasmtime::bail!("StreamBroker plugin not found in context");
            };
            (plugin.workloads.clone(), self.ctx.workload_id.to_string())
        };

        let mut workloads = workloads_arc.write().await;
        let broker = workloads.entry(workload_id).or_default();
        let client_id = broker.next_client_id;
        broker.next_client_id = broker.next_client_id.saturating_add(1).max(1);
        broker
            .clients
            .insert(client_id, Arc::new(Mutex::new(broker.tx.subscribe())));
        Ok(client_id)
    }

    async fn unregister(&mut self, client_id: u64) -> wasmtime::Result<()> {
        let (workloads_arc, workload_id) = {
            let Some(plugin) = self.get_plugin::<StreamBroker>(PLUGIN_STREAM_BROKER_ID) else {
                wasmtime::bail!("StreamBroker plugin not found in context");
            };
            (plugin.workloads.clone(), self.ctx.workload_id.to_string())
        };

        if let Some(broker) = workloads_arc.write().await.get_mut(&workload_id) {
            broker.clients.remove(&client_id);
        }
        Ok(())
    }
}

// Async funcs (`wait-message`, `publish-message`) live on
// `HostWithStore`, which bindgen invokes with an `Accessor` so the
// async body can yield back to the runtime without holding a store
// borrow. We extract the per-workload broker handle (and any plugin
// state) synchronously under `accessor.with(...)`, then await on the
// extracted handle outside the borrow.
impl bindings::wasmcloud::patch_stream::broker::HostWithStore for SharedCtx {
    async fn wait_message<T: 'static>(
        accessor: &wasmtime::component::Accessor<T, Self>,
        client_id: u64,
    ) -> wasmtime::Result<Option<String>> {
        // Extract the cloned Arc + workload_id synchronously under
        // the store borrow; the await on the lock happens outside so
        // we don't pin the store while suspended.
        let (workloads_arc, workload_id) = accessor.with(|mut access| {
            let ctx = access.get();
            let Some(plugin) = ctx.get_plugin::<StreamBroker>(PLUGIN_STREAM_BROKER_ID) else {
                return Err(wasmtime::Error::msg(
                    "StreamBroker plugin not found in context",
                ));
            };
            Ok((plugin.workloads.clone(), ctx.ctx.workload_id.to_string()))
        })?;

        let rx = {
            let workloads = workloads_arc.read().await;
            workloads
                .get(&workload_id)
                .and_then(|broker| broker.clients.get(&client_id))
                .cloned()
        };
        let Some(rx) = rx else {
            return Ok(None);
        };

        let mut rx = rx.lock().await;
        loop {
            match rx.recv().await {
                Ok(message) => return Ok(Some(message)),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(client_id, skipped, "stream broker websocket client lagged");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
            }
        }
    }

    async fn publish_message<T: 'static>(
        accessor: &wasmtime::component::Accessor<T, Self>,
        message: String,
    ) -> wasmtime::Result<Result<(), String>> {
        let extracted = accessor.with(|mut access| {
            let ctx = access.get();
            let Some(plugin) = ctx.get_plugin::<StreamBroker>(PLUGIN_STREAM_BROKER_ID) else {
                return Err("StreamBroker plugin not found in context".to_string());
            };
            Ok((plugin.workloads.clone(), ctx.ctx.workload_id.to_string()))
        });
        let (workloads_arc, workload_id) = match extracted {
            Ok(pair) => pair,
            Err(msg) => return Ok(Err(msg)),
        };

        let mut workloads = workloads_arc.write().await;
        let broker = workloads.entry(workload_id).or_default();
        let client_count = broker.tx.receiver_count();
        let _ = broker.tx.send(message);
        if client_count == 0 {
            tracing::debug!("stream broker publish had no websocket clients");
        }
        Ok(Ok(()))
    }
}

#[async_trait::async_trait]
impl HostPlugin for StreamBroker {
    fn id(&self) -> &'static str {
        PLUGIN_STREAM_BROKER_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:patch-stream/broker@0.1.0")]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(_interface) = interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "patch-stream")
        else {
            return Ok(());
        };

        bindings::wasmcloud::patch_stream::broker::add_to_linker::<_, SharedCtx>(
            item.linker(),
            extract_active_ctx,
        )?;

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        self.workloads.write().await.remove(workload_id);
        Ok(())
    }
}
