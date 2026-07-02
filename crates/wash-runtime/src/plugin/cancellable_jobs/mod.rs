//! Cancellation control plugin backing the `examples/cancellable-counter`
//! example.
//!
//! It owns the one thing a guest cannot reach — the host-side per-invocation
//! cancellation handles — plus a small per-id lifecycle record. A workload
//! `register`s the calling invocation's own handle under a request-id and
//! reports `progress`/`complete`; a separate request `cancel`s it (tripping
//! the handle, which the runtime's epoch interruption turns into a trap that
//! tears down the work's store) and `status` reads the outcome back. The
//! frozen `count` under a `cancelled` status is the proof the work stopped.
//!
//! The plugin is inert unless a workload declares `demo:jobs/control`.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    engine::{
        ctx::{ActiveCtx, SharedCtx, extract_active_ctx},
        workload::WorkloadItem,
    },
    plugin::HostPlugin,
    plugin::WitInterfaces,
    wasmtime,
    wit::{WitInterface, WitWorld},
};
use tokio::sync::RwLock;
use tracing::debug;

const CANCELLABLE_JOBS_PLUGIN_ID: &str = "cancellable_jobs";

mod bindings {
    crate::wasmtime::component::bindgen!({
    world: "cancellable-jobs",
    imports: { default: async | trappable },
        });
}

struct Registration {
    cancel_handle: Arc<AtomicBool>,
}

type RequestID = String;

#[derive(Default)]
pub struct CancellableJobsPlugin {
    registrations: Arc<RwLock<HashMap<RequestID, Registration>>>,
}

use bindings::wasmcloud::cancellable_jobs::control::RegisterError;

impl<'a> bindings::wasmcloud::cancellable_jobs::control::Host for ActiveCtx<'a> {
    async fn register(
        &mut self,
        request_id: String,
    ) -> wasmtime::Result<Result<(), RegisterError>> {
        let plugin = self.try_get_plugin::<CancellableJobsPlugin>(CANCELLABLE_JOBS_PLUGIN_ID)?;

        let cancel_handle = self.cancel_handle.clone();
        let mut registrations = plugin.registrations.write().await;
        if registrations.contains_key(&request_id) {
            return Ok(Err(RegisterError::DuplicateKey(request_id)));
        }

        registrations.insert(request_id.clone(), Registration { cancel_handle });
        debug!(request_id, "work registered");
        Ok(Ok(()))
    }

    async fn cancel(&mut self, request_id: String) -> wasmtime::Result<bool> {
        let plugin = self.try_get_plugin::<CancellableJobsPlugin>(CANCELLABLE_JOBS_PLUGIN_ID)?;

        // NOTE:
        // Maybe we need a write lock here
        // because other wise we check if it contains and milliseconds after it is already
        // written?
        // Get write lock so that you dont cancel it two times after each other
        let mut registrations = plugin.registrations.write().await;
        if let Some(registration) = registrations.get(&request_id) {
            registration.cancel_handle.store(true, Ordering::Relaxed);
            registrations.remove(&request_id);
            debug!(request_id, "work cancelled");
            return Ok(true);
        }
        debug!(request_id, "work does not exist");
        Ok(false)
    }
}

#[async_trait::async_trait]
impl HostPlugin for CancellableJobsPlugin {
    fn id(&self) -> &'static str {
        CANCELLABLE_JOBS_PLUGIN_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:cancellable-jobs/control@0.1.0",
            )]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "cancellable_jobs")
        {
            tracing::warn!(
                "TracingLogger plugin requested for non-wasmcloud:cancellable_jobs interface(s): {:?}",
                interfaces
            );

            return Ok(());
        }

        bindings::wasmcloud::cancellable_jobs::control::add_to_linker::<_, SharedCtx>(
            component_handle.linker(),
            extract_active_ctx,
        )?;
        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        _workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // TODO: Implement this, maybe we remove all the registered invocations
        // of that workload that unbinds?
        Ok(())
    }
}
