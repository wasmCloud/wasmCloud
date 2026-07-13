//! TriggerService: a long-lived service instance that co-drives `wasi:cli/run`
//! alongside one or more host-invoked handler exports on a single instance.
//!
//! A wasmCloud service is a long-lived `wasi:cli/run` component. When that same
//! component also exports a host-invoked handler (today `wasi:http/handler@0.3`),
//! the TriggerService runs BOTH on one instance under a single [`Store::run_concurrent`]:
//!
//! - the `cli/run` export drives the service's own long-running work (e.g. a
//!   connection pooler listening on a loopback socket), and
//! - each host-invoked export is served as concurrent per-invocation tasks on
//!   the same instance, so the long-running work and the handlers share the
//!   instance's in-memory state.
//!
//! Each host-invoked export is an [`Ingress`]: the host-side plugin (the HTTP
//! server, ...) pushes invocations into the ingress's channel and the TriggerService
//! serves them via [`Accessor::spawn`]. Adding another host-invoked interface
//! (e.g. a messaging handler) is a new [`Ingress`] variant plus a serve arm —
//! the `cli/run` driving and the single-instance `run_concurrent` are reused.
//! Each ingress kind lives in its own submodule ([`http`], [`messaging`],
//! [`capability`]); this module holds the shared [`Ingress`] enum, the
//! `prepare`/`serve` dispatch, and the [`run_trigger_driver`] loop.
//!
//! The [`Ingress::Capability`] variant generalizes this to *host component
//! plugins*: instead of HTTP requests or messages, the host pushes cross-store
//! capability calls (a workload importing `acme:kv/store`, `wasi:keyvalue`, ...)
//! as [`CapabilityJob`]s, and the TriggerService serves each on the same long-lived
//! instance — the persistent, concurrent-driven store a host component plugin is
//! built on. Such a plugin need not export `wasi:cli/run`; the TriggerService treats it
//! as optional and simply omits the co-driven run loop when it is absent.

#[cfg(feature = "host-component-plugins")]
use std::collections::HashMap;
use std::sync::Arc;

use wasmtime::Store;
use wasmtime::component::{Accessor, AccessorTask, ComponentExportIndex, Instance, InstancePre};
use wasmtime::error::Context as _;
use wasmtime_wasi::p3::bindings::Command;
use wasmtime_wasi_http::p3::bindings::Service;

use crate::engine::ctx::SharedCtx;
use crate::host::http::ServiceHttpJob;
#[cfg(feature = "host-component-plugins")]
use crate::host::job_registry::{JobGuard, JobRegistry};

#[cfg(feature = "host-component-plugins")]
mod capability;
mod http;
mod messaging;

#[cfg(feature = "host-component-plugins")]
pub use capability::{CapabilityCall, CapabilityFunc, CapabilityJob};
pub use messaging::{BrokerMessage, MessagingJob};

#[cfg(feature = "host-component-plugins")]
use capability::{
    CapabilityTask, InFlightGuard, MAX_INFLIGHT_CAPABILITY_CALLS, drain_plugin_resources,
    flush_pending_resource_drops,
};
use http::HttpTask;
use messaging::{HANDLE_MESSAGE, MESSAGING_HANDLER, MessagingTask};

/// A host-invoked handler export the TriggerService serves, carrying the receiver end
/// of its delivery channel. The paired sender is handed to the host-side ingress
/// (the HTTP server, the messaging subscriber, ...) so it can deliver
/// invocations to this live instance.
#[non_exhaustive]
pub enum Ingress {
    /// `wasi:http/handler@0.3` — the HTTP server delivers requests here.
    Http(tokio::sync::mpsc::Receiver<ServiceHttpJob>),
    /// `wasmcloud:messaging/handler@0.2.0` — the messaging subscriber delivers
    /// received messages here.
    Messaging(tokio::sync::mpsc::Receiver<MessagingJob>),
    /// Cross-store capability calls for a host component plugin. `funcs` lists
    /// every exported function to resolve up front; `rx` delivers the calls;
    /// `registry` tracks each served call as a cancellable job.
    #[cfg(feature = "host-component-plugins")]
    Capability {
        funcs: Vec<CapabilityFunc>,
        rx: tokio::sync::mpsc::Receiver<CapabilityJob>,
        registry: Arc<JobRegistry>,
    },
}

impl Ingress {
    /// Build this ingress's binding view over the shared instance. Done before
    /// `run_concurrent` (which needs `&mut store`), mirroring the `cli` view.
    fn prepare(
        self,
        store: &mut Store<SharedCtx>,
        instance: &Instance,
    ) -> anyhow::Result<PreparedIngress> {
        match self {
            Ingress::Http(rx) => {
                let service = Service::new(store, instance)
                    .map_err(|e| e.context("service is missing wasi:http/handler export"))?;
                Ok(PreparedIngress::Http {
                    service: Arc::new(service),
                    rx,
                })
            }
            Ingress::Messaging(rx) => {
                // Look up the p2 `handle-message` export up front; it's invoked
                // dynamically (there is no accessor-driven p3 messaging binding).
                let iface = instance
                    .get_export(&mut *store, None, MESSAGING_HANDLER)
                    .with_context(|| format!("service is missing {MESSAGING_HANDLER} export"))?
                    .1;
                let func_idx = instance
                    .get_export(&mut *store, Some(&iface), HANDLE_MESSAGE)
                    .with_context(|| format!("{MESSAGING_HANDLER} is missing {HANDLE_MESSAGE}"))?
                    .1;
                Ok(PreparedIngress::Messaging {
                    instance: *instance,
                    func_idx,
                    rx,
                })
            }
            #[cfg(feature = "host-component-plugins")]
            Ingress::Capability {
                funcs,
                rx,
                registry,
            } => {
                // Resolve every exported capability function to a call index up
                // front (mirroring the messaging arm), so serving a call is a
                // map lookup rather than a per-call export search.
                // Nested by interface so a call resolves its index with borrowed
                // `&str` lookups (`Arc<str>: Borrow<str>`) — no per-call key clone.
                let mut func_map: HashMap<Arc<str>, HashMap<Arc<str>, ComponentExportIndex>> =
                    HashMap::new();
                for CapabilityFunc { interface, func } in funcs {
                    let iface = instance
                        .get_export(&mut *store, None, &interface)
                        .with_context(|| format!("plugin is missing {interface} export"))?
                        .1;
                    let func_idx = instance
                        .get_export(&mut *store, Some(&iface), &func)
                        .with_context(|| format!("{interface} is missing {func}"))?
                        .1;
                    func_map
                        .entry(interface)
                        .or_default()
                        .insert(func, func_idx);
                }
                Ok(PreparedIngress::Capability {
                    instance: *instance,
                    func_map,
                    rx,
                    registry,
                    in_flight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                })
            }
        }
    }
}

/// An [`Ingress`] with its binding view built, ready to serve invocations under
/// `run_concurrent`.
enum PreparedIngress {
    Http {
        service: Arc<Service>,
        rx: tokio::sync::mpsc::Receiver<ServiceHttpJob>,
    },
    Messaging {
        instance: Instance,
        func_idx: ComponentExportIndex,
        rx: tokio::sync::mpsc::Receiver<MessagingJob>,
    },
    #[cfg(feature = "host-component-plugins")]
    Capability {
        instance: Instance,
        func_map: HashMap<Arc<str>, HashMap<Arc<str>, ComponentExportIndex>>,
        rx: tokio::sync::mpsc::Receiver<CapabilityJob>,
        registry: Arc<JobRegistry>,
        in_flight: Arc<std::sync::atomic::AtomicUsize>,
    },
}

/// Why a [`PreparedIngress::serve`] loop returned: the channel closed (the
/// workload is stopping) or a proxied resource must be dropped (which needs
/// top-level store access, so the driver steps out of `run_concurrent`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ServeOutcome {
    Shutdown,
    #[cfg(feature = "host-component-plugins")]
    FlushDrops,
}

impl PreparedIngress {
    /// Serve inbound invocations, spawning one concurrent task per invocation on
    /// the shared instance, until the delivery channel closes ([`Shutdown`]) or a
    /// resource drop needs flushing ([`FlushDrops`]). Takes `&mut self` so the
    /// driver can resume the same channel after stepping out to flush drops.
    ///
    /// [`Shutdown`]: ServeOutcome::Shutdown
    /// [`FlushDrops`]: ServeOutcome::FlushDrops
    async fn serve(&mut self, accessor: &Accessor<SharedCtx>) -> ServeOutcome {
        match self {
            PreparedIngress::Http { service, rx } => {
                while let Some((req, resp_tx)) = rx.recv().await {
                    accessor.spawn(HttpTask {
                        service: Arc::clone(service),
                        req,
                        resp_tx,
                    });
                }
                ServeOutcome::Shutdown
            }
            PreparedIngress::Messaging {
                instance,
                func_idx,
                rx,
            } => {
                while let Some((msg, result_tx)) = rx.recv().await {
                    accessor.spawn(MessagingTask {
                        instance: *instance,
                        func_idx: *func_idx,
                        msg,
                        result_tx,
                    });
                }
                ServeOutcome::Shutdown
            }
            #[cfg(feature = "host-component-plugins")]
            PreparedIngress::Capability {
                instance,
                func_map,
                rx,
                registry,
                in_flight,
            } => {
                use std::sync::atomic::Ordering;
                while let Some(job) = rx.recv().await {
                    match job {
                        CapabilityJob::DropResource { proxy_id, reply } => {
                            // Stage the real resource and step out of
                            // `run_concurrent` so the driver can free it with the
                            // top-level store access `resource_drop_async` needs.
                            accessor.with(|mut access| {
                                if let Some(registry) = access.data_mut().resource_registry.as_mut()
                                {
                                    registry.stage_drop(proxy_id);
                                }
                            });
                            let _ = reply.send(Ok(()));
                            return ServeOutcome::FlushDrops;
                        }
                        CapabilityJob::Call(call) => {
                            // Admit the call only under the in-flight ceiling. A
                            // non-blocking reservation (fetch_add + compare): the
                            // serve loop NEVER blocks, so a re-entrant call is
                            // always admitted while the store is under the ceiling
                            // — no bounded-pool deadlock. A runaway self-recursion
                            // consumes a slot per hop and is rejected at the cap.
                            if in_flight.fetch_add(1, Ordering::SeqCst)
                                >= MAX_INFLIGHT_CAPABILITY_CALLS
                            {
                                in_flight.fetch_sub(1, Ordering::SeqCst);
                                let _ = call.reply.send(Err(wasmtime::format_err!(
                                    "host component plugin is at its in-flight capability-call \
                                     ceiling ({MAX_INFLIGHT_CAPABILITY_CALLS}); a call cycle or \
                                     overload is likely"
                                )));
                                continue;
                            }
                            let guard = InFlightGuard::new(Arc::clone(in_flight));
                            let Some(func_idx) = func_map
                                .get(&*call.interface)
                                .and_then(|fns| fns.get(&*call.func))
                                .copied()
                            else {
                                let _ = call.reply.send(Err(wasmtime::format_err!(
                                    "plugin does not export {}/{}",
                                    call.interface,
                                    call.func
                                )));
                                continue;
                            };
                            // Register the call as a cancellable job. The
                            // `JobGuard` moves into the task and retires the job on
                            // completion or drop. The `Accessor::spawn` handle is
                            // not retained: wasmtime cannot hard-cancel the guest
                            // subtask, so cancellation is cooperative —
                            // `request-cancel` marks the job and the guest unwinds
                            // itself.
                            let job = registry.admit(call.caller.clone());
                            let job_guard = JobGuard::new(Arc::clone(registry), job);
                            accessor.spawn(CapabilityTask {
                                instance: *instance,
                                func_idx,
                                call,
                                in_flight: guard,
                                job_guard,
                            });
                        }
                    }
                }
                ServeOutcome::Shutdown
            }
        }
    }
}

/// A running service instance co-driving `cli/run` and its host-invoked handler
/// exports on one instance.
pub struct TriggerService {
    /// The driver task: instantiates once and runs cli/run + every ingress
    /// concurrently.
    pub driver: tokio::task::JoinHandle<()>,
}

impl TriggerService {
    /// Instantiate the service once and start driving its `cli/run` export plus
    /// every `ingress` on the same instance under one `run_concurrent`.
    pub fn spawn(
        mut store: Store<SharedCtx>,
        pre: InstancePre<SharedCtx>,
        ingresses: Vec<Ingress>,
    ) -> anyhow::Result<Self> {
        let driver = tokio::spawn(async move {
            if let Err(e) = run_trigger_driver(&mut store, &pre, ingresses).await {
                tracing::error!(err = %e, "trigger service driver faulted");
            }
        });

        Ok(TriggerService { driver })
    }
}

/// Drive a trigger service on `store` to completion: instantiate `pre`, then
/// co-drive `cli/run` (optional) with every ingress under `run_concurrent`,
/// serving until each ingress channel closes (clean exit, `Ok`) or the store
/// faults (`Err`, e.g. a guest trap). Reusable across restarts — a supervisor
/// re-instantiates into the same store and re-runs this.
pub(crate) async fn run_trigger_driver(
    store: &mut Store<SharedCtx>,
    pre: &InstancePre<SharedCtx>,
    ingresses: Vec<Ingress>,
) -> anyhow::Result<()> {
    let instance = pre
        .instantiate_async(&mut *store)
        .await
        .context("failed to instantiate trigger service")?;
    // `cli/run` is optional: a service exports it (its long-running work), but a
    // pure capability host component plugin need not — in which case there is
    // simply no run loop to co-drive.
    let mut command = match Command::new(&mut *store, &instance) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!(err = %e, "no wasi:cli/run export to co-drive");
            None
        }
    };
    // Build each ingress's binding view before entering run_concurrent.
    let mut prepared = Vec::with_capacity(ingresses.len());
    for ingress in ingresses {
        prepared.push(anyhow::Context::context(
            ingress.prepare(&mut *store, &instance),
            "failed to prepare trigger service ingress",
        )?);
    }

    // Drive serving in a loop. A proxied-resource drop needs top-level store
    // access (`resource_drop_async`), which is unavailable inside
    // `run_concurrent`; when one is pending, a serve loop returns `FlushDrops`,
    // we step out here to free it, and re-enter. In-flight tasks are preserved
    // across `run_concurrent` calls (the concurrent state lives on the store), so
    // re-entering resumes them. Without host component plugins there are no
    // proxied resources, so serving runs to channel-close in a single pass.
    let mut serve_again = true;
    while serve_again {
        let outcomes = store
            .run_concurrent(async |accessor| {
                // Spawn the cli/run co-driver once (first entry only).
                if let Some(command) = command.take() {
                    accessor.spawn(RunTask { command });
                }
                // `join_all` steps out only once EVERY ingress serve returns, so a
                // `FlushDrops` is prompt only when the Capability ingress is served
                // alone. That holds today: a plugin store carries exactly one
                // Capability ingress and no Http/Messaging ingress, and service
                // stores (which carry those) have no resource registry and never
                // flush.
                futures::future::join_all(prepared.iter_mut().map(|p| p.serve(accessor))).await
            })
            .await
            .context("trigger service driver exited")?;
        // Every channel closed → clean stop, unless an ingress asked to flush a
        // proxied-resource drop (which needs top-level store access), in which
        // case free it and re-serve to resume the preserved in-flight tasks.
        #[cfg(feature = "host-component-plugins")]
        {
            flush_pending_resource_drops(&mut *store).await;
            serve_again = outcomes.contains(&ServeOutcome::FlushDrops);
        }
        #[cfg(not(feature = "host-component-plugins"))]
        {
            let _ = outcomes;
            serve_again = false;
        }
    }
    // Teardown: drop every resource the plugin still owns.
    #[cfg(feature = "host-component-plugins")]
    drain_plugin_resources(&mut *store).await;
    Ok(())
}

/// Drives the service's `wasi:cli/run` export (its long-running work).
struct RunTask {
    command: Command,
}

impl AccessorTask<SharedCtx> for RunTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        match self.command.wasi_cli_run().call_run(accessor).await {
            Ok(Ok(())) => tracing::info!("service cli/run exited successfully"),
            Ok(Err(())) => tracing::error!("service cli/run exited with error"),
            Err(e) => tracing::error!(err = %e, "service cli/run trapped"),
        }
        Ok(())
    }
}
