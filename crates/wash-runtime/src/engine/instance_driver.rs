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
//! A driver removes that. An inbound HTTP request, an inbound message, a call
//! from another component in the workload and a plugin's delivery are all
//! [`InstanceJob`]s, so a component reached several ways shares one warm set
//! rather than keeping one per trigger. It owns its store for good and runs one
//! long-lived [`wasmtime::Store::run_concurrent`], taking calls off a channel
//! and [`Accessor::spawn`]ing each as a concurrent task on the same instance.
//! That is what the host already does for a service (see
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
//!    many, after which it drains and its store drops. A call that times out
//!    or fails in the host mid-call retires the instance the same way: the
//!    guest work cannot be cancelled from the host, so draining and dropping
//!    the store is what ends it. So does the pool's idle sweep, through
//!    [`InstanceDriver::retire`], when the component's traffic no longer
//!    needs this many instances.
//!
//! A retired instance ends its own run loop as soon as its last call finishes,
//! rather than waiting for the pool to notice. That matters for the timed-out
//! call: dropping the store is what ends the guest work it left running, so it
//! must not wait on traffic that may never come.
//!
//! The cost of sharing an instance is that a guest trap takes the whole store
//! with it, so every call in flight on that instance fails rather than just
//! one. That is bounded by `max_concurrency`, and by `1/pool_size` of the pool.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wasmtime::component::{Accessor, AccessorTask, ComponentExportIndex, Instance, Val};
use wasmtime::error::Context as _;
use wasmtime_wasi_http::p3::bindings::Service;

use crate::engine::ctx::SharedCtx;
use crate::engine::instance_pool::ComponentInstance;
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
    /// The abandonment flag of the dispatched call enforcing this job's
    /// deadline (see [`crate::engine::abandon`]).
    pub(crate) abandoned: Arc<crate::engine::abandon::AbandonFlag>,
}

/// Work an instance can be given. Every shape runs as a concurrent task on the
/// same instance, so a component reached several ways shares one warm set
/// rather than keeping one per trigger.
pub(crate) enum InstanceJob {
    /// An inbound HTTP request (`wasi:http/handler@0.3`). Boxed to keep the
    /// variants a similar size; a declined job carries the whole request back.
    Http(Box<ServiceHttpJob>),
    /// A call from another component in the workload.
    Linked(Box<LinkedJob>),
    /// An inbound message (`wasmcloud:messaging/handler@0.3.0`), delivered by
    /// whichever messaging backend the workload bound.
    ///
    /// Only the async `@0.3.0` handler reaches here. Its call takes an
    /// [`Accessor`], so deliveries overlap on one instance up to
    /// `max_concurrency`; the sync `@0.2.0` export holds `&mut Store` for the
    /// length of its call and keeps its per-message store.
    Messaging(Box<crate::host::trigger_service::MessagingJob>),
    /// A call a host plugin supplies, made on a pooled instance.
    ///
    /// The engine routes it like any other job and never looks inside: the
    /// plugin keeps its own payload and makes its own typed call. That is what
    /// lets a delivery carry, say, a NATS message's bytes rather than the one
    /// 48-byte [`Val`] per byte a store-independent lowering would cost.
    Plugin(Box<dyn PluginJob>),
}

/// A call a plugin hands to the pool, run on whichever instance is free.
///
/// Implemented by the plugin so the engine needs none of its types. The
/// plugin's own bindgen call takes an [`Accessor`] rather than a `&mut Store`,
/// so it runs inside the driver's long-lived `run_concurrent` exactly as a
/// linked call does — several at a time on one instance, up to
/// `max_concurrency`.
pub(crate) trait PluginJob: Send + 'static {
    /// Names this job in a driver log line.
    fn describe(&self) -> &str;

    /// Runs the call. Owns replying to whoever is waiting for it, and may
    /// retire the instance through `slot` when it ends leaving guest state
    /// indeterminate — the same contract [`LinkedTask`] follows.
    fn run<'a>(
        self: Box<Self>,
        accessor: &'a Accessor<SharedCtx>,
        instance: Instance,
        slot: Option<PoolSlot>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Measures the guest execution one pooled call adds, and records it when
/// dropped.
///
/// Every pooled call is metered, not just the ones a plugin remembered to wrap:
/// `guest.execution.time` is the only signal an operator has for what a warm
/// instance is spending, and a histogram whose population depends on which
/// call site opted in cannot be read as a distribution.
///
/// Recording on drop is what makes it cover a call that ends by trapping or
/// timing out — the two an operator most wants in the histogram, and the two an
/// early return would otherwise skip.
///
/// Started once the export has resolved, not before: a call that never reached
/// guest code has no execution to report, and recording it anyway would put a
/// 0ms observation in the same bucket as a genuinely fast call.
///
/// A plugin serving a call from a store of its own — because the component
/// declared no pool, or because every instance was busy — starts one of these
/// too, so a component's measurements do not depend on whether it opted into
/// pooling.
pub(crate) struct ExecutionSample {
    executed: Arc<crate::engine::abandon::GuestExecution>,
    before: u64,
    attributes: Vec<opentelemetry::KeyValue>,
}

impl ExecutionSample {
    pub(crate) fn start(
        accessor: &Accessor<SharedCtx>,
        attributes: Vec<opentelemetry::KeyValue>,
    ) -> Self {
        let executed = accessor.with(|mut access| Arc::clone(&access.get().executed));
        let before = executed.millis();
        Self {
            executed,
            before,
            attributes,
        }
    }
}

impl Drop for ExecutionSample {
    fn drop(&mut self) {
        if let Some(meter) = crate::observability::execution_time_meter() {
            meter.record(
                &self.attributes,
                self.executed.millis().saturating_sub(self.before),
            );
        }
    }
}

/// Drives one [`PluginJob`] as an ordinary pooled task.
struct PluginTask {
    instance: Instance,
    job: Box<dyn PluginJob>,
    slot: PoolSlot,
}

impl AccessorTask<SharedCtx> for PluginTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        self.job.run(accessor, self.instance, Some(self.slot)).await;
        Ok(())
    }
}

/// What an instance's driver handle and the calls running on it share: how many
/// calls are in flight, whether the instance still admits any, and the signal
/// that ends its run loop once a retired instance has drained.
struct DriverState {
    in_flight: AtomicUsize,
    /// Set once this instance must take no more calls — its invocation budget
    /// is spent, or a call ended leaving guest state indeterminate.
    retired: AtomicBool,
    /// Signalled when a retired instance's last call finishes, so its run loop
    /// stops there and then. Dropping the store is what ends guest work a
    /// timed-out call left running, and waiting for the pool's next dispatch
    /// would leave that running for as long as traffic stayed away.
    drained: tokio::sync::Notify,
}

impl DriverState {
    /// Stop admitting, and end the run loop if there is nothing left to drain.
    fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        // The last call may already have finished, in which case no guard is
        // left to signal on the way out.
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            self.drained.notify_one();
        }
    }
}

/// A pooled call's tether to its instance: holds the call's in-flight slot for
/// as long as the task lives, and can retire the instance when the call ends
/// in a way that leaves guest state indeterminate.
pub(crate) struct PoolSlot {
    state: Arc<DriverState>,
    /// Frees this call's slot when the slot is dropped, however the task ends.
    _in_flight: InFlightGuard,
}

impl PoolSlot {
    /// Stop this instance admitting: it drains what it took, ends its run loop,
    /// and its store's teardown ends any guest work still running on it.
    ///
    /// Only as far as the last call returning, though: every path to `drained`
    /// runs through a call's task ending, so a guest that never yields holds the
    /// store open regardless. That one is ended by its abandoned call instead
    /// (see [`crate::engine::abandon`]).
    ///
    /// TODO: retirement is a stand-in for cancelling the one bad call. The
    /// host cannot cancel a guest `call_concurrent` subtask
    /// (bytecodealliance/wasmtime#11833), so ending a wedged call's work means
    /// condemning the whole instance and every warm state it held. Once that
    /// API exists, a timed-out call should cancel just its own task and leave
    /// the instance serving.
    pub(crate) fn retire_instance(&self) {
        self.state.retire();
    }
}

/// Serves one linked call on a shared instance. Uses `call_concurrent`, so
/// calls interleave rather than taking the store in turn.
struct LinkedTask {
    instance: Instance,
    job: Box<LinkedJob>,
    slot: PoolSlot,
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
            abandoned,
        } = *self.job;
        let instance = self.instance;

        // The epoch deadline measures this call's own execution, so re-arm it
        // here. `watch_until_abandoned` below owns the registration.
        let calls = accessor.with(|mut access| {
            crate::engine::abandon::rearm_for_call(&mut access);
            Arc::clone(&access.get().abandoned)
        });
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
        // This bound ends the caller's wait. Keeping a slow guest out of the
        // epoch callback's reach is `watch_until_abandoned`'s job.
        let outcome = match tokio::time::timeout(
            call_timeout,
            crate::engine::abandon::watch_until_abandoned(
                &calls,
                abandoned,
                func.call_concurrent(accessor, &params, &mut results),
            ),
        )
        .await
        {
            Ok(Ok(())) => Ok(results),
            // A host error mid-call leaves the guest in an indeterminate
            // state; a trap will also fault the whole driver, but retiring
            // here covers the errors that do not.
            Ok(Err(e)) => {
                tracing::warn!(
                    err = ?e,
                    %import_name,
                    %export_name,
                    "pooled call failed in the host; retiring the instance"
                );
                self.slot.retire_instance();
                Err(e)
            }
            // A guest subtask cannot be cancelled from the host, so the timed
            // out work is still running on this store. Retiring the instance
            // is what ends it: the driver stops admitting, drains, ends its
            // run loop, and the store's teardown takes the stalled work with
            // it.
            Err(e) => {
                tracing::warn!(
                    %import_name,
                    %export_name,
                    timeout = ?call_timeout,
                    "pooled call timed out; retiring the instance to end the stalled guest work"
                );
                self.slot.retire_instance();
                Err(wasmtime::format_err!(
                    "function call timed out after {call_timeout:?}: {e}"
                ))
            }
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
pub(crate) struct InFlightGuard(Arc<DriverState>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        // `fetch_sub` returns the previous count, so `1` means this was the
        // last call in flight. A retired instance is finished at that point.
        if self.0.in_flight.fetch_sub(1, Ordering::SeqCst) == 1
            && self.0.retired.load(Ordering::SeqCst)
        {
            self.0.drained.notify_one();
        }
    }
}

/// One warm instance, plus the channel its calls arrive on.
pub(crate) struct InstanceDriver {
    tx: tokio::sync::mpsc::Sender<(InstanceJob, InFlightGuard)>,
    state: Arc<DriverState>,
    /// Calls admitted so far, against `max_invocations`.
    admitted: AtomicUsize,
    max_concurrency: usize,
    max_invocations: Option<usize>,
    /// The triggered work this instance can take, so admission never gives it
    /// a job it could only fail.
    accepts: Accepts,
}

/// The typed handler views bound over one instance, built once when its driver
/// starts and reused by every call on it.
///
/// Each is both the probe and the binding: `Service::new` and
/// `AsyncMessaging::new` type-check their export against the live instance and
/// hand back the view. A component reached only by linked calls has neither,
/// which is not an error — it just cannot be given triggered work.
///
/// A set rather than a flag per trigger: the next host-invoked export is a
/// field and a match arm, not a third bool and a third special case in
/// admission.
struct BoundExports {
    http: Option<Arc<Service>>,
    messaging: Option<Arc<crate::host::trigger_service::AsyncMessaging>>,
}

/// Which job kinds an instance will accept — the presence half of
/// [`BoundExports`], split off so admission can answer without the views (and
/// so a test can build one without a store).
#[derive(Clone, Copy)]
struct Accepts {
    http: bool,
    messaging: bool,
}

impl BoundExports {
    fn bind(store: &mut wasmtime::Store<SharedCtx>, instance: &Instance) -> Self {
        Self {
            http: Service::new(&mut *store, instance).ok().map(Arc::new),
            messaging: crate::host::trigger_service::AsyncMessaging::new(&mut *store, instance)
                .ok()
                .map(Arc::new),
        }
    }

    fn accepts(&self) -> Accepts {
        Accepts {
            http: self.http.is_some(),
            messaging: self.messaging.is_some(),
        }
    }
}

impl Accepts {
    /// Whether this instance can be given `job` at all.
    ///
    /// A linked call names the export index it resolved against this very
    /// component, and a plugin job binds its own view when it runs, so neither
    /// is gated here.
    fn takes(self, job: &InstanceJob) -> bool {
        match job {
            InstanceJob::Http(_) => self.http,
            InstanceJob::Messaging(_) => self.messaging,
            InstanceJob::Linked(_) | InstanceJob::Plugin(_) => true,
        }
    }
}

impl InstanceDriver {
    /// Build an instantiated store's driver and start it. The caller
    /// instantiates, so a component that fails to do so reports that failure
    /// where it can still be returned to whoever asked for the call.
    pub(crate) fn spawn(
        instance: ComponentInstance,
        max_concurrency: usize,
        max_invocations: Option<usize>,
    ) -> Self {
        let ComponentInstance {
            mut store,
            instance,
        } = instance;
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<(InstanceJob, InFlightGuard)>(max_concurrency.max(1));
        let state = Arc::new(DriverState {
            in_flight: AtomicUsize::new(0),
            retired: AtomicBool::new(false),
            drained: tokio::sync::Notify::new(),
        });
        let task_state = Arc::clone(&state);

        // Built before the run loop so admission knows what this instance can
        // take at all, rather than accepting work it could only drop. The views
        // move into the run loop; admission keeps only their presence.
        let bound = BoundExports::bind(&mut store, &instance);
        let accepts = bound.accepts();

        tokio::spawn(async move {
            // One `run_concurrent` for the life of the instance. Each call is
            // spawned onto it, so calls overlap instead of taking the store in
            // turn. It returns when the channel closes (the pool dropped the
            // handle), when a retired instance has drained, or when the guest
            // traps.
            let outcome = store
                .run_concurrent(async |accessor| {
                    loop {
                        let (job, guard) = tokio::select! {
                            received = rx.recv() => match received {
                                Some(received) => received,
                                None => break,
                            },
                            // Retired and drained: stopping here drops the
                            // store, which is what ends guest work a
                            // timed-out call left running.
                            _ = task_state.drained.notified() => break,
                        };
                        let spawned = match job {
                            InstanceJob::Http(job) => {
                                let ServiceHttpJob {
                                    req,
                                    resp_tx,
                                    abandoned,
                                } = *job;
                                let Some(service) = bound.http.as_ref().map(Arc::clone) else {
                                    // Admission declines HTTP for an instance
                                    // without the export, so this is not
                                    // normally reachable; answer the request
                                    // rather than dropping it if it ever is.
                                    let _ = resp_tx.send(Err(anyhow::anyhow!(
                                        "pooled instance does not export wasi:http/handler"
                                    )));
                                    continue;
                                };
                                accessor.spawn(HttpTask {
                                    service,
                                    req,
                                    resp_tx,
                                    abandoned,
                                    pool_slot: Some(PoolSlot {
                                        state: Arc::clone(&task_state),
                                        _in_flight: guard,
                                    }),
                                })
                            }
                            InstanceJob::Messaging(job) => {
                                let crate::host::trigger_service::MessagingJob {
                                    msg,
                                    result_tx,
                                    abandoned,
                                    attributes,
                                } = *job;
                                let Some(handler) = bound.messaging.as_ref().map(Arc::clone) else {
                                    // As with HTTP: admission declines this for
                                    // an instance without the export, so report
                                    // it rather than dropping the delivery if
                                    // it is ever reached.
                                    let _ =
                                        result_tx.send(Err("pooled instance does not export the \
                                         @0.3.0 messaging handler"
                                            .into()));
                                    continue;
                                };
                                accessor.spawn(crate::host::trigger_service::MessagingTask {
                                    handler,
                                    msg,
                                    result_tx,
                                    abandoned,
                                    attributes,
                                    pool_slot: Some(PoolSlot {
                                        state: Arc::clone(&task_state),
                                        _in_flight: guard,
                                    }),
                                })
                            }
                            InstanceJob::Linked(job) => accessor.spawn(LinkedTask {
                                instance,
                                job,
                                slot: PoolSlot {
                                    state: Arc::clone(&task_state),
                                    _in_flight: guard,
                                },
                            }),
                            InstanceJob::Plugin(job) => accessor.spawn(PluginTask {
                                instance,
                                job,
                                slot: PoolSlot {
                                    state: Arc::clone(&task_state),
                                    _in_flight: guard,
                                },
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
            state,
            admitted: AtomicUsize::new(0),
            max_concurrency,
            max_invocations,
            accepts,
        }
    }

    /// Calls in flight on this instance right now.
    pub(crate) fn in_flight(&self) -> usize {
        self.state.in_flight.load(Ordering::SeqCst)
    }

    /// Whether this instance has served its last call and is only draining.
    pub(crate) fn is_retired(&self) -> bool {
        self.state.retired.load(Ordering::SeqCst)
    }

    /// Stop this instance admitting calls, from outside the calls running on
    /// it: it drains what it already took, ends its run loop and drops its
    /// store. What the pool's idle sweep uses to give back an instance the
    /// component's traffic stopped needing (see
    /// [`crate::engine::instance_pool::InstancePool::sweep`]).
    pub(crate) fn retire(&self) {
        self.state.retire();
    }

    /// Whether the driver's task has gone (the guest trapped, or the instance
    /// retired and drained), in which case the handle is dead and should be
    /// reaped.
    pub(crate) fn is_gone(&self) -> bool {
        self.tx.is_closed()
    }

    /// Claim a slot on this instance, or `None` when it is full, retired or
    /// gone. A plain compare-and-swap rather than a semaphore: admission has to
    /// be non-blocking so a saturated instance falls through to the next one
    /// instead of making the caller wait.
    pub(crate) fn try_admit(&self) -> Option<InFlightGuard> {
        if self.is_retired() || self.is_gone() {
            return None;
        }
        let claimed = self
            .state
            .in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.max_concurrency).then_some(n + 1)
            })
            .is_ok();
        if !claimed {
            return None;
        }
        let guard = InFlightGuard(Arc::clone(&self.state));

        // Count the call against this instance's budget. Admission past the
        // limit is refused, not just noted: racing callers can all get here
        // before any of them marks the instance retired, and `fetch_update`
        // is what keeps the budget exact under that race. The instance then
        // drains what it admitted rather than being dropped mid-call the way
        // a checked-out store could be.
        if let Some(limit) = self.max_invocations {
            match self
                .admitted
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    (n < limit).then_some(n + 1)
                }) {
                Ok(previous) => {
                    if previous + 1 >= limit {
                        self.state.retire();
                    }
                }
                // Budget already spent by racing admissions. Dropping the
                // guard frees the slot this call claimed.
                Err(_) => {
                    self.state.retire();
                    return None;
                }
            }
        }
        Some(guard)
    }

    /// A driver with no store behind it, for exercising admission alone. The
    /// receiver stands in for the run loop: kept alive so the channel is open,
    /// never drained.
    #[cfg(test)]
    pub(crate) fn stub(
        max_concurrency: usize,
        max_invocations: Option<usize>,
    ) -> (
        Self,
        tokio::sync::mpsc::Receiver<(InstanceJob, InFlightGuard)>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(max_concurrency.max(1));
        (
            Self {
                tx,
                state: Arc::new(DriverState {
                    in_flight: AtomicUsize::new(0),
                    retired: AtomicBool::new(false),
                    drained: tokio::sync::Notify::new(),
                }),
                admitted: AtomicUsize::new(0),
                max_concurrency,
                max_invocations,
                accepts: Accepts {
                    http: true,
                    messaging: true,
                },
            },
            rx,
        )
    }

    /// Hand a call to this instance. Returns the job again if the instance
    /// could not take it, so the caller can try elsewhere. Boxed because the
    /// refusal carries the whole request back.
    pub(crate) fn try_send(&self, job: InstanceJob) -> Result<(), InstanceJob> {
        // An instance without the export a job needs can only fail it.
        // Declining sends the job to a store of its own, where the binding is
        // built per call and its error reaches whoever is waiting.
        if !self.accepts.takes(&job) {
            return Err(job);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The invocation budget is exact: once `max_invocations` calls have been
    /// admitted the instance refuses more — even though the refused caller had
    /// already claimed (and must release) a concurrency slot — and stays
    /// refused after every in-flight call completes.
    #[test]
    fn invocation_budget_is_exact() {
        let (driver, _rx) = InstanceDriver::stub(8, Some(2));

        let first = driver.try_admit().expect("first call is within budget");
        let second = driver.try_admit().expect("second call spends the budget");
        assert!(driver.is_retired(), "spending the budget retires");
        assert!(
            driver.try_admit().is_none(),
            "a third admission must be refused, not merely noted"
        );
        assert_eq!(
            driver.in_flight(),
            2,
            "the refused admission must have released its claimed slot"
        );

        drop(first);
        drop(second);
        assert!(
            driver.try_admit().is_none(),
            "a drained retired instance still admits nothing"
        );
    }

    /// The budget refuses over-admission even when callers race: `fetch_update`
    /// consumes the budget atomically, so exactly `max_invocations` admissions
    /// can ever succeed no matter how the threads interleave. (The sequential
    /// test above cannot distinguish this from bumping a counter after the
    /// fact; this one can.)
    #[test]
    fn invocation_budget_holds_under_racing_admissions() {
        let (driver, _rx) = InstanceDriver::stub(64, Some(16));
        let driver = Arc::new(driver);

        let admitted: usize = std::thread::scope(|scope| {
            (0..8)
                .map(|_| {
                    let driver = Arc::clone(&driver);
                    scope.spawn(move || {
                        // Hold the guards so slots stay claimed: the budget,
                        // not the concurrency cap, must be what refuses.
                        let mut guards = Vec::new();
                        while let Some(guard) = driver.try_admit() {
                            guards.push(guard);
                        }
                        guards.len()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("admission thread panicked"))
                .sum()
        });

        assert_eq!(admitted, 16, "the budget must be exact under contention");
    }

    /// `max_concurrency` caps in-flight calls, and a completed call frees its
    /// slot for the next.
    #[test]
    fn concurrency_slots_are_reclaimed() {
        let (driver, _rx) = InstanceDriver::stub(2, None);

        let a = driver.try_admit().expect("first slot");
        let _b = driver.try_admit().expect("second slot");
        assert!(driver.try_admit().is_none(), "at capacity");

        drop(a);
        assert!(
            driver.try_admit().is_some(),
            "a finished call's slot serves the next"
        );
    }

    /// A retired instance ends its run loop as soon as its last call finishes,
    /// not whenever the pool next happens to look at it. Dropping the store is
    /// what ends guest work a timed-out call left running, so it cannot wait
    /// on traffic that may never arrive.
    #[tokio::test]
    async fn a_retired_instance_signals_its_run_loop_once_drained() {
        let (driver, _rx) = InstanceDriver::stub(4, Some(1));
        let guard = driver.try_admit().expect("first call is within budget");
        assert!(driver.is_retired(), "spending the budget retires");

        let state = Arc::clone(&driver.state);
        let poll = std::time::Duration::from_millis(50);
        assert!(
            tokio::time::timeout(poll, state.drained.notified())
                .await
                .is_err(),
            "a retired instance with a call still running has not drained"
        );

        drop(guard);
        tokio::time::timeout(poll, state.drained.notified())
            .await
            .expect("the last call finishing must end the run loop");
    }

    /// Retiring an instance that is already idle still ends its run loop:
    /// there is no in-flight guard left to signal on the way out.
    #[tokio::test]
    async fn retiring_an_idle_instance_signals_its_run_loop() {
        let (driver, _rx) = InstanceDriver::stub(4, None);
        driver.state.retire();

        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            driver.state.drained.notified(),
        )
        .await
        .expect("an idle instance is drained the moment it retires");
    }
}
