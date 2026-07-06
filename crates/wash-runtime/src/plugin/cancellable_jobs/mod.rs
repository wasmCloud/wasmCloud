//! A Job Cancellation plugin
//!
//! This Plugin implements the `wasmcloud:cancellable-jobs/control@0.1.0` interface
//!
//! # Glossary
//! - cancel handle : represents a boolean state that basically translates into "Should the store that holds your request be dropped or not" : true/false
//! - epoch-cancel : wasmtime engine has a host-wide ticker(if that helps?) that basically starts an increment based counter in a thread and adds checkpoints into any wasm guest code that will execute on it
//! epoch-interruption enables running those checks every X duration (in our case it's 10ms) and then increments the epoch by 1
//! - every store that gets created for the incoming requests, then creates it's own cancellation tracking loop where it sets it's local deadline as +1 of the current epoch,
//! where the cancle handle is checked during those checkpoints that run every 10ms,if the bool is false, increment deadline, if the bool is true then the epoch interruption tears down that store and anything running in it,
//!  the http side handles this gracefully by detecting abort on the receiver for the channel between the store and the http inbound so clients don't see 499.
//!
//! # Features
//! - Registers each incoming long running invocation with a client-handled request-id into a map which tracks : (request_id : (cancle_handle, workload_id))
//! - Allows cancellation of those invocations, mapped by their request-id, the cancel_handle turns Relaxed, checkpoint detects the change and tears down the request store
//! - Secondary helper called 'complete" exists to update the registrations map, to remove the request-id that has successfully completed its call without interruption
//! without this, registrations map will not empty and subsequently chances of hitting a duplicateKey error increase.

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
    /// So that any workload may cancel any request-id, but we track the owner so unbinding
    /// a workload can drop only its own registrations.
    workload_id: WorkloadID,
}

type RequestID = String;
type WorkloadID = Arc<str>;

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
        let workload_id = self.workload_id.clone();

        let mut registrations = plugin.registrations.write().await;
        if registrations.contains_key(&request_id) {
            return Ok(Err(RegisterError::DuplicateKey(request_id)));
        }

        registrations.insert(
            request_id.clone(),
            Registration {
                cancel_handle,
                workload_id,
            },
        );
        debug!(request_id, "work registered");
        Ok(Ok(()))
    }

    async fn cancel(&mut self, request_id: String) -> wasmtime::Result<bool> {
        let plugin = self.try_get_plugin::<CancellableJobsPlugin>(CANCELLABLE_JOBS_PLUGIN_ID)?;

        // Get write lock so that you dont cancel it two times after each other
        let mut registrations = plugin.registrations.write().await;
        if let Some(registration) = registrations.remove(&request_id) {
            registration.cancel_handle.store(true, Ordering::Relaxed);
            debug!(request_id, "work cancelled");
            return Ok(true);
        }
        debug!(request_id, "work does not exist");
        Ok(false)
    }

    async fn complete(&mut self, request_id: String) -> wasmtime::Result<()> {
        let plugin = self.try_get_plugin::<CancellableJobsPlugin>(CANCELLABLE_JOBS_PLUGIN_ID)?;

        if plugin
            .registrations
            .write()
            .await
            .remove(&request_id)
            .is_some()
        {
            debug!(request_id, "work completed");
        }
        Ok(())
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
            .any(|i| i.namespace == "wasmcloud" && i.package == "cancellable-jobs")
        {
            tracing::warn!(
                "CancellableJobs plugin requested for non-wasmcloud:cancellable-jobs interface(s): {:?}",
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
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let mut registrations = self.registrations.write().await;
        let before = registrations.len();
        registrations.retain(|_, reg| reg.workload_id.as_ref() != workload_id);
        let removed = before - registrations.len();
        if removed > 0 {
            debug!(
                workload_id,
                count = removed,
                "removed workload registrations on unbind"
            );
        }
        Ok(())
    }
}
