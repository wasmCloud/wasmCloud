//! Per-store registry of cancellable in-flight jobs.
//!
//! Each spawned [`crate::host::trigger_service`] task that serves one invocation is a
//! *job*. Cancellation is **cooperative**: `request-cancel` marks the job and the
//! running guest observes it and unwinds itself — explicitly (polling
//! `is-cancelled`) or implicitly (a streaming guest's next write returns
//! `dropped`). wasmtime cannot hard-cancel a guest `call_concurrent` subtask short
//! of dropping the whole store, so a cooperative mark is the only per-invocation
//! lever that spares the store's other tenants.
//!
//! The registry is TriggerService-scoped (one per plugin incarnation) and `Arc<Mutex>`
//! backed rather than part of `SharedCtx`, so a [`JobGuard`] can clean up when its
//! task's future drops without async store access. A registry belongs to one
//! store, so cross-store isolation is structural; within a store, a job may be
//! cancelled only by a requester from its owner's workload. Job ids are
//! host-minted and opaque, so a guest cannot forge one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wasmtime::component::GuestTaskId;

use crate::engine::ctx::CallerIdentity;

/// Host-minted, opaque identifier for one cancellable job. Unique within a
/// registry (one store incarnation); never reused within that lifetime.
pub type JobId = u64;

struct JobEntry {
    /// The identity that started the job; only a same-workload requester may
    /// cancel it.
    owner: CallerIdentity,
    /// Set by [`JobRegistry::request_cancel`]; the running guest observes it (via
    /// `is-cancelled`) and unwinds cooperatively.
    cancelled: bool,
}

#[derive(Default)]
struct Inner {
    next: JobId,
    jobs: BTreeMap<JobId, JobEntry>,
    /// Maps a running guest task to its job and the caller that started it, so a
    /// host import can resolve either from the current async call stack.
    by_task: BTreeMap<GuestTaskId, (JobId, CallerIdentity)>,
}

/// Registry of the cancellable jobs on one store incarnation. See the module
/// docs for the isolation and cancellation model.
pub struct JobRegistry(Mutex<Inner>);

impl JobRegistry {
    /// A fresh, empty registry behind an `Arc` for sharing between the serve loop
    /// and the host cancel/identity imports. Job ids start at 1 so `0` is a
    /// reserved "no current job" sentinel for `current-job`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Inner {
            next: 1,
            ..Default::default()
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Admit a job for `owner`, returning its id. Called in the serve loop as the
    /// task is spawned; a [`JobGuard`] built with the id retires it on drop.
    pub fn admit(&self, owner: CallerIdentity) -> JobId {
        let mut inner = self.lock();
        let job = inner.next;
        inner.next = inner.next.wrapping_add(1);
        inner.jobs.insert(
            job,
            JobEntry {
                owner,
                cancelled: false,
            },
        );
        job
    }

    /// Bind the guest task a job runs under, recording the caller so the identity
    /// and cancel imports can resolve the job/caller from the async call stack.
    /// Called through [`JobGuard::set_task`] so the binding is cleaned on drop.
    fn bind_task(&self, task: GuestTaskId, job: JobId, caller: CallerIdentity) {
        self.lock().by_task.insert(task, (job, caller));
    }

    /// The job currently running under `task`, for `current-job`.
    pub fn job_for_task(&self, task: GuestTaskId) -> Option<JobId> {
        self.lock().by_task.get(&task).map(|(job, _)| *job)
    }

    /// The caller that started the job running under `task`, for the host
    /// identity import.
    pub fn caller_for_task(&self, task: GuestTaskId) -> Option<CallerIdentity> {
        self.lock()
            .by_task
            .get(&task)
            .map(|(_, caller)| caller.clone())
    }

    /// Ask `job` to cancel on behalf of `requester`; returns whether the request
    /// was accepted (not whether the guest has stopped). Accepted only when the
    /// requester shares the job owner's workload — the cross-tenant guard. The
    /// guest observes the mark and unwinds itself.
    pub fn request_cancel(&self, job: JobId, requester: &CallerIdentity) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.jobs.get_mut(&job) else {
            return false;
        };
        if entry.owner.workload_id != requester.workload_id {
            return false;
        }
        entry.cancelled = true;
        true
    }

    /// Whether `job` has been asked to cancel, for the guest's cooperative poll.
    pub fn is_cancelled(&self, job: JobId) -> bool {
        self.lock()
            .jobs
            .get(&job)
            .is_some_and(|entry| entry.cancelled)
    }

    /// Remove a job and its task binding. Called by [`JobGuard`] on drop, so it
    /// runs whether the task completes normally or its future is dropped (e.g. on
    /// store teardown), with no store access required.
    ///
    /// The task binding is removed only if it still points at *this* job: a guest
    /// task id is reused once its task ends, so by the time this job retires a
    /// newer job may already have rebound the same task id. Job ids are unique and
    /// never reused within an incarnation, so this test cannot false-match.
    fn retire(&self, job: JobId, task: Option<GuestTaskId>) {
        let mut inner = self.lock();
        inner.jobs.remove(&job);
        if let Some(task) = task
            && inner
                .by_task
                .get(&task)
                .is_some_and(|(bound, _)| *bound == job)
        {
            inner.by_task.remove(&task);
        }
    }

    /// Number of live jobs. Test-only leak assertion.
    #[cfg(test)]
    pub fn live_jobs(&self) -> usize {
        self.lock().jobs.len()
    }

    /// Number of live task bindings. Test-only leak assertion.
    #[cfg(test)]
    pub fn live_task_bindings(&self) -> usize {
        self.lock().by_task.len()
    }
}

/// RAII cleanup for one job: retires the job (and its task binding) from the
/// registry on drop. Because a dropped task future skips any cleanup written as a
/// trailing statement after an `.await`, the retire lives here instead, running
/// on normal completion and on drop alike without needing store access.
pub struct JobGuard {
    registry: Arc<JobRegistry>,
    job: JobId,
    task: Option<GuestTaskId>,
}

impl JobGuard {
    /// Guard an admitted `job`. The task id is attached later via
    /// [`Self::set_task`] once the guest call starts.
    pub fn new(registry: Arc<JobRegistry>, job: JobId) -> Self {
        Self {
            registry,
            job,
            task: None,
        }
    }

    /// Record the guest task the job runs under once the call has started,
    /// binding it (and the `caller`) in the registry so `current-job`,
    /// `is-cancelled`, and the identity import resolve, and so drop cleans the
    /// binding.
    pub fn set_task(&mut self, task: GuestTaskId, caller: CallerIdentity) {
        self.task = Some(task);
        self.registry.bind_task(task, self.job, caller);
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.registry.retire(self.job, self.task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(workload: &str, component: &str) -> CallerIdentity {
        CallerIdentity {
            workload_id: Arc::from(workload),
            component_id: Arc::from(component),
        }
    }

    #[test]
    fn cancel_is_authorized_by_workload() {
        let reg = JobRegistry::new();
        let job = reg.admit(caller("wl-a", "comp-a"));
        assert!(!reg.is_cancelled(job));

        // A different workload cannot cancel — the cross-tenant guard.
        assert!(!reg.request_cancel(job, &caller("wl-b", "comp-b")));
        assert!(
            !reg.is_cancelled(job),
            "an unauthorized cancel has no effect"
        );

        // The owning workload can — a different component within it still counts,
        // since authorization is by workload, not component.
        assert!(reg.request_cancel(job, &caller("wl-a", "comp-other")));
        assert!(reg.is_cancelled(job), "an authorized cancel marks the job");
    }

    #[test]
    fn cancel_unknown_job_is_false() {
        let reg = JobRegistry::new();
        assert!(!reg.request_cancel(999, &caller("wl-a", "comp-a")));
        assert!(!reg.is_cancelled(999));
    }

    #[test]
    fn guard_retires_job_on_drop() {
        let reg = JobRegistry::new();
        let job = reg.admit(caller("wl-a", "comp-a"));
        assert_eq!(reg.live_jobs(), 1);
        {
            let _guard = JobGuard::new(Arc::clone(&reg), job);
            assert_eq!(reg.live_jobs(), 1, "job stays live while its guard is held");
        }
        // Drop ran retire. The task-bound path (`set_task`) needs a real
        // `GuestTaskId` and is exercised by the integration tests.
        assert_eq!(reg.live_jobs(), 0, "guard drop retires the job");
    }
}
