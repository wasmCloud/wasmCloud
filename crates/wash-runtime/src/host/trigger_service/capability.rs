//! The [`Ingress::Capability`] path: cross-store capability calls a host
//! component plugin serves on its long-lived instance.
//!
//! [`Ingress::Capability`]: super::Ingress::Capability

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use wasmtime::AsContextMut;
use wasmtime::Store;
use wasmtime::component::types::Type;
use wasmtime::component::{Accessor, AccessorTask, ComponentExportIndex, Instance, Val};
use wasmtime::error::Context as _;

use crate::engine::ctx::{CallerIdentity, SharedCtx};
use crate::engine::store::relocate::{self, Relocated};
use crate::host::job_registry::JobGuard;

/// One exported capability function the TriggerService should be ready to serve,
/// identified by its interface and function name (e.g. `acme:kv/store@0.1.0` /
/// `get`). Resolved to a [`ComponentExportIndex`] once the instance exists.
#[derive(Clone)]
pub struct CapabilityFunc {
    pub interface: Arc<str>,
    pub func: Arc<str>,
}

/// Work routed to a host component plugin's long-lived instance from a caller
/// store across the bridge.
#[non_exhaustive]
pub enum CapabilityJob {
    /// A capability function call (including resource methods, constructors, and
    /// statics — they are ordinary interface functions).
    Call(CapabilityCall),
    /// Drop a proxied resource: the caller dropped its proxy, so the real
    /// resource in the plugin store is taken from the registry and destroyed.
    DropResource {
        proxy_id: u64,
        reply: tokio::sync::oneshot::Sender<wasmtime::Result<()>>,
    },
}

/// A single cross-store capability call. The host-side shim (installed on a
/// workload's linker) extracts the call's arguments in the caller store into
/// store-agnostic [`Relocated`] values, sends them here, and awaits the
/// [`Relocated`] results on `reply`. Handle-free values are copied;
/// `stream<T>`/`future<T>`/`resource` handles are relocated across the boundary
/// via [`relocate`].
pub struct CapabilityCall {
    /// Interface of the called function (matches a [`CapabilityFunc::interface`]).
    pub interface: Arc<str>,
    /// Function name within that interface (matches a [`CapabilityFunc::func`]).
    pub func: Arc<str>,
    /// Identity of the calling workload, set on the plugin store while the call
    /// runs so the plugin can partition state per caller.
    pub caller: CallerIdentity,
    /// Call arguments, extracted in the caller store, to be injected into the
    /// plugin store.
    pub args: Vec<Relocated>,
    /// Result types of the called function, used to extract the results in the
    /// plugin store before relocating them back to the caller.
    pub result_tys: Arc<[Type]>,
    /// Carries the call's relocated results (or a trap/routing error) back to
    /// the shim.
    pub reply: tokio::sync::oneshot::Sender<wasmtime::Result<Vec<Relocated>>>,
}

/// Hard ceiling on capability-call tasks in flight on one plugin instance at
/// once, counting re-entrant hops (each hop of an `A -> plugin -> ... -> A`
/// chain is a live task suspended awaiting the next). A call that would exceed
/// it is rejected with a trap rather than spawned.
///
/// This is the single concurrency backstop: it bounds both breadth (many
/// concurrent chains) and re-entrant depth (a runaway self-recursion consumes a
/// task per hop and hits the ceiling, surfacing a clear error instead of
/// exhausting memory). It is enforced with a non-blocking atomic counter — never
/// a blocking acquire — so a re-entrant call can never deadlock waiting on a slot
/// held by an ancestor that is itself waiting on the re-entrant call. Unlike a
/// per-store depth counter, an atomic count is exact under concurrent calls.
pub(super) const MAX_INFLIGHT_CAPABILITY_CALLS: usize = 512;

/// Serves one cross-store capability call on the shared plugin instance: looks
/// up the resolved export, invokes it via the dynamic concurrent path
/// (`call_concurrent`, so calls interleave rather than taking the store
/// exclusively), and returns the results — or the trap — to the host-side shim.
///
/// A *routing* or host-side error is reported on that call's `reply` only and
/// leaves the store serving other calls. A *guest trap* (panic/unreachable/OOM),
/// however, poisons the wasm store: `call_concurrent` faults the driver, and the
/// plugin is rebuilt under supervision (see [`crate::plugin::component_host`]).
/// Because the plugin is a host-scoped singleton, that restart drops every
/// tenant's in-memory state — the blast radius of the shared-singleton model.
pub(super) struct CapabilityTask {
    pub(super) instance: Instance,
    pub(super) func_idx: ComponentExportIndex,
    pub(super) call: CapabilityCall,
    /// Releases this call's in-flight slot when the task completes (or is
    /// dropped, e.g. on cancellation).
    pub(super) in_flight: InFlightGuard,
    /// Retires this call's registry job (and its task binding) when the call
    /// completes or the task is otherwise dropped (e.g. store teardown). Held
    /// across the `.await` so the retire runs however the task ends.
    pub(super) job_guard: JobGuard,
}

/// Decrements a plugin store's in-flight capability-call counter on drop, so a
/// slot is reclaimed whether the task completes normally or is cancelled.
pub(super) struct InFlightGuard(Arc<AtomicUsize>);

impl InFlightGuard {
    pub(super) fn new(counter: Arc<AtomicUsize>) -> Self {
        Self(counter)
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Free every resource whose proxy a caller has dropped since the last flush,
/// using the top-level store access `resource_drop_async` requires (unavailable
/// inside `run_concurrent`). Runs each guest resource destructor.
pub(super) async fn flush_pending_resource_drops(store: &mut Store<SharedCtx>) {
    let pending = store
        .data_mut()
        .resource_registry
        .as_mut()
        .map(crate::engine::store::resource_bridge::ResourceRegistry::take_pending_drops)
        .unwrap_or_default();
    for real in pending {
        if let Err(e) = real.resource_drop_async(&mut *store).await {
            tracing::warn!(err = %e, "failed to drop a proxied resource");
        }
    }
}

/// Drop every resource the plugin still owns, on store teardown, so nothing
/// leaks when the plugin stops or restarts.
pub(super) async fn drain_plugin_resources(store: &mut Store<SharedCtx>) {
    let all = store
        .data_mut()
        .resource_registry
        .as_mut()
        .map(crate::engine::store::resource_bridge::ResourceRegistry::drain_all)
        .unwrap_or_default();
    for real in all {
        if let Err(e) = real.resource_drop_async(&mut *store).await {
            tracing::warn!(err = %e, "failed to drop a proxied resource on teardown");
        }
    }
}

impl AccessorTask<SharedCtx> for CapabilityTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let CapabilityTask {
            instance,
            func_idx,
            call,
            in_flight: _in_flight,
            mut job_guard,
        } = self;
        let CapabilityCall {
            interface,
            func,
            caller,
            args,
            result_tys,
            reply,
        } = call;

        // Look up the export and inject the relocated arguments — in one discrete
        // sync block, never holding the borrow across the await below.
        let prepared = accessor.with(|mut access| -> wasmtime::Result<_> {
            let func_handle = instance
                .get_func(&mut access, func_idx)
                .with_context(|| format!("capability function {interface}/{func} not found"))?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg in args {
                arg_vals.push(relocate::inject(access.as_context_mut(), arg)?);
            }
            Ok((func_handle, arg_vals))
        });
        let (func_handle, arg_vals) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = reply.send(Err(e));
                return Ok(());
            }
        };

        // Start the call so we can read the guest task it runs under, then bind
        // that task to this job (recording the caller). The identity and cancel
        // imports resolve the caller/job by walking their async call stack back to
        // this root task. The binding lives in the `JobGuard`, so it is retired
        // when the call completes or the task is dropped, without store access.
        let mut results = vec![Val::Bool(false); result_tys.len()];
        let started = accessor.with(|mut access| -> wasmtime::Result<_> {
            let call = func_handle.start_call_concurrent(
                access.as_context_mut(),
                &arg_vals,
                &mut results,
            )?;
            let task_id = call.task();
            Ok((call, task_id))
        });
        let (call, task_id) = match started {
            Ok(started) => started,
            Err(e) => {
                let _ = reply.send(Err(e));
                return Ok(());
            }
        };
        job_guard.set_task(task_id, caller);
        let call_result = func_handle.finish_call_concurrent(accessor, call).await;
        if let Err(e) = call_result {
            let _ = reply.send(Err(
                e.context(format!("capability call {interface}/{func} trapped"))
            ));
            return Ok(());
        }

        // Extract the results in the plugin store. Any result `stream`/`future`
        // pumps keep running under this persistent store's `run_concurrent`, so
        // their drain signals (`dones`) are dropped here rather than awaited.
        let extracted = accessor.with(|mut access| -> wasmtime::Result<Vec<Relocated>> {
            let mut dones = Vec::new();
            let mut out = Vec::with_capacity(results.len());
            for (val, ty) in results.iter().zip(result_tys.iter()) {
                out.push(relocate::extract(
                    access.as_context_mut(),
                    val,
                    ty,
                    &mut dones,
                )?);
            }
            Ok(out)
        });
        let _ = reply.send(extracted);
        Ok(())
    }
}
