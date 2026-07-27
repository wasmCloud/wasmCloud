//! A warm instance that serves several calls at once.
//!
//! A pooled instance is otherwise checked out for one call at a time: the
//! caller takes it from the pool, owns its store for the call, and returns it
//! afterwards. A call the warm set cannot take is still served — from a store
//! of its own — so what reuse loses there is not throughput but everything the
//! warm instance had already built: its connection pool, its authenticated
//! session, whatever it set up on first use. A guest that spends its call
//! awaiting I/O is exactly the guest with that kind of state, so a pool has to
//! be sized to peak *concurrency* to keep any of it, rather than to peak work.
//!
//! A driver removes that. Both an inbound HTTP request and a call from another
//! component in the workload are [`InstanceJob`]s, so a component reached both
//! ways shares one warm set rather than keeping two. It owns its store for good
//! and runs one long-lived
//! [`wasmtime::Store::run_concurrent`], taking calls off a channel and
//! [`Accessor::spawn`]ing each as a concurrent task on the same instance. That
//! is what the host already does for a service (see
//! [`crate::host::trigger_service`]) — this is the same driver, one per warm
//! instance rather than one per workload, with admission control in front.
//!
//! Two limits bound it:
//!
//!  * `max_concurrency` caps the calls one instance has in flight. It defaults
//!    to one, so a component that only asked for `pool_size` behaves exactly as
//!    it did before. Raising it is safe only for a guest that *yields* while it
//!    waits; a guest driving its own executor (anything calling `block_on`)
//!    would have a second call try to enter that executor from inside itself.
//!  * `max_invocations` stops the driver admitting once it has served that
//!    many, after which it drains and its store drops.
//!
//! The cost of sharing an instance is that a guest trap takes the whole store
//! with it, so every call in flight on that instance fails rather than just
//! one. That is bounded by `max_concurrency`, and by `1/pool_size` of the pool.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wasmtime::component::{
    Accessor, AccessorTask, ComponentExportIndex, Instance, InstancePre, Val,
};
use wasmtime::error::Context as _;
use wasmtime_wasi_http::p3::bindings::Service;

use crate::engine::ctx::SharedCtx;
use crate::host::http::ServiceHttpJob;
use crate::host::trigger_service::HttpTask;

/// A plain-value call from another component in the workload, routed to this
/// one's instance. Handle-free by construction (that is what puts it on this
/// path), so its arguments and results cross a channel as plain data.
pub(crate) struct LinkedJob {
    pub(crate) func_idx: ComponentExportIndex,
    pub(crate) params: Vec<Val>,
    pub(crate) results_len: usize,
    pub(crate) import_name: Arc<str>,
    pub(crate) export_name: Arc<str>,
    pub(crate) reply: tokio::sync::oneshot::Sender<wasmtime::Result<Vec<Val>>>,
}

/// Work an instance can be given. Both shapes run as concurrent tasks on the
/// same instance, so a component reached both ways shares one warm set rather
/// than keeping two.
pub(crate) enum InstanceJob {
    /// An inbound HTTP request (`wasi:http/handler@0.3`). Boxed to keep the
    /// variants a similar size; a declined job carries the whole request back.
    Http(Box<ServiceHttpJob>),
    /// A call from another component in the workload.
    Linked(Box<LinkedJob>),
}

/// Serves one linked call on a shared instance. Uses `call_concurrent`, so
/// calls interleave rather than taking the store in turn.
struct LinkedTask {
    instance: Instance,
    job: Box<LinkedJob>,
    /// Frees this call's slot when the task ends, however it ends.
    _in_flight: InFlightGuard,
}

impl AccessorTask<SharedCtx> for LinkedTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let LinkedJob {
            func_idx,
            params,
            results_len,
            import_name,
            export_name,
            reply,
        } = *self.job;
        let instance = self.instance;

        let func = accessor.with(|mut access| {
            instance
                .get_func(&mut access, func_idx)
                .with_context(|| format!("function not found for {import_name}.{export_name}"))
        });
        let func = match func {
            Ok(func) => func,
            Err(e) => {
                let _ = reply.send(Err(e));
                return Ok(());
            }
        };

        let mut results = vec![Val::Bool(false); results_len];
        let call_timeout = crate::timeouts::ephemeral_call();
        let outcome = match tokio::time::timeout(
            call_timeout,
            func.call_concurrent(accessor, &params, &mut results),
        )
        .await
        {
            Ok(Ok(())) => Ok(results),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(wasmtime::format_err!(
                "function call timed out after {call_timeout:?}: {e}"
            )),
        };
        // The caller may have gone; its call still ran to completion, because
        // a guest subtask cannot be cancelled from the host.
        let _ = reply.send(outcome);
        Ok(())
    }
}

/// Releases an instance's in-flight slot when the call it admitted ends,
/// however it ends. Held by the task, so a cancelled or trapped call frees its
/// slot just as a completed one does.
pub(crate) struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One warm instance, plus the channel its calls arrive on.
pub(crate) struct InstanceDriver {
    tx: tokio::sync::mpsc::Sender<(InstanceJob, InFlightGuard)>,
    in_flight: Arc<AtomicUsize>,
    /// Calls admitted so far, against `max_invocations`.
    admitted: AtomicUsize,
    /// Set once this instance has admitted its last call; it then drains and
    /// the pool reaps it.
    retired: AtomicBool,
    max_concurrency: usize,
    max_invocations: Option<usize>,
}

impl InstanceDriver {
    /// Build a store's driver and start it. The returned handle is live
    /// immediately; the driver instantiates on its own task, and a call sent
    /// before that finishes simply waits in the channel.
    pub(crate) fn spawn(
        mut store: wasmtime::Store<SharedCtx>,
        pre: InstancePre<SharedCtx>,
        max_concurrency: usize,
        max_invocations: Option<usize>,
    ) -> Self {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<(InstanceJob, InFlightGuard)>(max_concurrency.max(1));

        tokio::spawn(async move {
            let instance = match pre.instantiate_async(&mut store).await {
                Ok(instance) => instance,
                Err(e) => {
                    tracing::error!(err = ?e, "failed to instantiate pooled instance");
                    return;
                }
            };
            // Only a component serving HTTP has this export; one reached only
            // by linked calls does not, and that is not an error.
            let service = Service::new(&mut store, &instance).ok().map(Arc::new);

            // One `run_concurrent` for the life of the instance. Each call is
            // spawned onto it, so calls overlap instead of taking the store in
            // turn. It returns when the channel closes (the pool dropped the
            // handle) or the guest traps.
            let outcome = store
                .run_concurrent(async |accessor| {
                    while let Some((job, guard)) = rx.recv().await {
                        let spawned = match job {
                            InstanceJob::Http(job) => {
                                let (req, resp_tx) = *job;
                                let Some(service) = service.as_ref().map(Arc::clone) else {
                                    tracing::error!("pooled instance is missing wasi:http/handler");
                                    continue;
                                };
                                accessor.spawn(HttpTask {
                                    service,
                                    req,
                                    resp_tx,
                                    in_flight: Some(guard),
                                })
                            }
                            InstanceJob::Linked(job) => accessor.spawn(LinkedTask {
                                instance,
                                job,
                                _in_flight: guard,
                            }),
                        };
                        if let Err(e) = spawned {
                            tracing::error!(err = %e, "failed to spawn pooled invocation task");
                        }
                    }
                })
                .await;
            if let Err(e) = outcome {
                // The store is poisoned: every call in flight on this instance
                // died with it. The pool reaps the handle and the next call
                // starts a fresh instance.
                tracing::error!(err = ?e, "pooled instance faulted; its in-flight calls failed");
            }
        });

        Self {
            tx,
            in_flight: Arc::new(AtomicUsize::new(0)),
            admitted: AtomicUsize::new(0),
            retired: AtomicBool::new(false),
            max_concurrency,
            max_invocations,
        }
    }

    /// Calls in flight on this instance right now.
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Whether this instance has served its last call and is only draining.
    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Whether the driver's task has gone (the guest trapped, or it failed to
    /// instantiate), in which case the handle is dead and should be reaped.
    pub(crate) fn is_gone(&self) -> bool {
        self.tx.is_closed()
    }

    /// Claim a slot on this instance, or `None` when it is full, retired or
    /// gone. A plain compare-and-swap rather than a semaphore: admission has to
    /// be non-blocking so a saturated instance falls through to the next one
    /// instead of making the caller wait.
    fn try_admit(&self) -> Option<InFlightGuard> {
        if self.is_retired() || self.is_gone() {
            return None;
        }
        let claimed = self
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.max_concurrency).then_some(n + 1)
            })
            .is_ok();
        if !claimed {
            return None;
        }
        let guard = InFlightGuard(Arc::clone(&self.in_flight));

        // Count the call against this instance's budget. The instance stops
        // admitting at the limit and drains what it already took, rather than
        // being dropped mid-call the way a checked-out store could be.
        if let Some(limit) = self.max_invocations
            && self.admitted.fetch_add(1, Ordering::SeqCst) + 1 >= limit
        {
            self.retired.store(true, Ordering::SeqCst);
        }
        Some(guard)
    }

    /// Hand a call to this instance. Returns the job again if the instance
    /// could not take it, so the caller can try elsewhere. Boxed because the
    /// refusal carries the whole request back.
    pub(crate) fn try_send(&self, job: InstanceJob) -> Result<(), InstanceJob> {
        let Some(guard) = self.try_admit() else {
            return Err(job);
        };
        // Capacity equals `max_concurrency` and admission already claimed a
        // slot, so this only fails if the driver task has gone.
        self.tx.try_send((job, guard)).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full((job, _))
            | tokio::sync::mpsc::error::TrySendError::Closed((job, _)) => job,
        })
    }
}
