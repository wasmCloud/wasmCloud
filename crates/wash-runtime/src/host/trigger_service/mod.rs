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
//! server, the messaging subscriber, ...) pushes invocations into the ingress's
//! channel and the TriggerService serves them via [`Accessor::spawn`]. Adding
//! another host-invoked interface is a new [`Ingress`] variant plus a serve arm
//! — the `cli/run` driving and the single-instance `run_concurrent` are reused.
//! Each ingress kind lives in its own submodule ([`http`], [`messaging`]); this
//! module holds the shared [`Ingress`] enum, the `prepare`/`serve` dispatch,
//! and the [`run_trigger_driver`] loop.

use std::sync::Arc;

use wasmtime::Store;
use wasmtime::component::{Accessor, AccessorTask, ComponentExportIndex, Instance, InstancePre};
use wasmtime::error::Context as _;
use wasmtime_wasi::p3::bindings::Command;
use wasmtime_wasi_http::p3::bindings::Service;

use crate::engine::ctx::SharedCtx;
use crate::host::http::ServiceHttpJob;

mod http;
mod messaging;

pub use messaging::{BrokerMessage, MessagingJob};

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
}

impl PreparedIngress {
    /// Serve inbound invocations, spawning one concurrent task per invocation on
    /// the shared instance, until the delivery channel closes (the workload is
    /// stopping).
    async fn serve(&mut self, accessor: &Accessor<SharedCtx>) {
        match self {
            PreparedIngress::Http { service, rx } => {
                while let Some((req, resp_tx)) = rx.recv().await {
                    accessor.spawn(HttpTask {
                        service: Arc::clone(service),
                        req,
                        resp_tx,
                    });
                }
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
    ) -> Self {
        let driver = tokio::spawn(async move {
            if let Err(e) = run_trigger_driver(&mut store, &pre, ingresses).await {
                tracing::error!(err = %e, "trigger service driver faulted");
            }
        });

        TriggerService { driver }
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
    // handler-only instance need not — in which case there is simply no run loop
    // to co-drive.
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
        prepared.push(
            ingress
                .prepare(&mut *store, &instance)
                .map_err(|e| e.context("failed to prepare trigger service ingress"))?,
        );
    }

    store
        .run_concurrent(async |accessor| {
            // Spawn the cli/run co-driver.
            if let Some(command) = command.take() {
                accessor.spawn(RunTask { command });
            }
            futures::future::join_all(prepared.iter_mut().map(|p| p.serve(accessor))).await;
        })
        .await
        .context("trigger service driver exited")?;
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
