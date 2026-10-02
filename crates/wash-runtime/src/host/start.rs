//! Teardown owned by a workload reservation, even after its start is cancelled.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use tokio::sync::RwLock;
use tokio_util::task::TaskTracker;

use super::{Host, HostWorkload, Reservation};
use crate::engine::workload::WorkloadStartResources;

type Workloads = Arc<RwLock<HashMap<String, HostWorkload>>>;
pub(super) type WorkloadStartRecoveries = Arc<Mutex<HashMap<String, Arc<CancelledWorkloadStart>>>>;

/// Retains a cancelled workload start's reservation and resources until teardown succeeds.
pub(super) struct CancelledWorkloadStart {
    reservation: Reservation,
    cleanup: WorkloadStartResources,
    serial: tokio::sync::Mutex<()>,
    /// The start answered its caller with a failure before its cleanup
    /// finished, so that failure outlives the cleanup as the workload's `Error`.
    retain_failure: bool,
    #[cfg(feature = "washlet")]
    _control: Option<Arc<super::HostControlLease>>,
}

impl CancelledWorkloadStart {
    /// Drop this recovery's bookkeeping, unless a later one replaced it.
    fn forget(&self, workload_id: &str, cancelled: &WorkloadStartRecoveries) {
        let mut cancelled = cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancelled
            .get(workload_id)
            .is_some_and(|entry| entry.reservation == self.reservation)
        {
            cancelled.remove(workload_id);
        }
    }

    pub(super) async fn recover(
        &self,
        workload_id: &str,
        workloads: &Workloads,
        cancelled: &WorkloadStartRecoveries,
    ) -> anyhow::Result<()> {
        let _serial = self.serial.lock().await;
        if cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(workload_id)
            .is_none_or(|entry| entry.reservation != self.reservation)
        {
            return Ok(());
        }
        {
            let mut workloads = workloads.write().await;
            match workloads.get(workload_id) {
                // Left as it is: the reason is published as the workload's
                // `Error` below, the same as when the start's own cleanup
                // succeeds. A cancelled start reported nothing, and drops the id.
                Some(HostWorkload::Failing(held, _))
                    if *held == self.reservation && self.retain_failure => {}
                Some(
                    HostWorkload::Starting(held)
                    | HostWorkload::Stopping(held)
                    | HostWorkload::Failing(held, _),
                ) if *held == self.reservation => {
                    workloads.insert(workload_id.into(), HostWorkload::Stopping(self.reservation));
                }
                // The id is no longer this start's. Every state a start can be
                // left in carries its reservation, so neither arm is expected;
                // but a recovery left recorded would answer every later stop
                // for this id, including one meant for a workload that has
                // claimed it since.
                None => {
                    self.forget(workload_id, cancelled);
                    return Ok(());
                }
                _ => {
                    self.forget(workload_id, cancelled);
                    anyhow::bail!("cancelled start no longer owns its workload reservation");
                }
            }
        }
        tokio::time::timeout(
            crate::timeouts::plugin_stop(),
            self.cleanup.release(workload_id),
        )
        .await
        .context("cancelled workload cleanup timed out")??;
        let mut workloads = workloads.write().await;
        let failure = match workloads.get(workload_id) {
            Some(HostWorkload::Stopping(held)) if *held == self.reservation => None,
            Some(HostWorkload::Failing(held, reason)) if *held == self.reservation => {
                Some(reason.clone())
            }
            _ => return Ok(()),
        };
        // Remove recovery bookkeeping while the ID is still held.
        self.forget(workload_id, cancelled);
        match failure {
            Some(reason) => {
                workloads.insert(workload_id.into(), HostWorkload::Error(reason));
            }
            None => {
                workloads.remove(workload_id);
            }
        }
        Ok(())
    }
}

/// Transfers an unfinished workload start to tracked cleanup when its future is dropped.
pub(crate) struct WorkloadStartGuard {
    workload_id: String,
    reservation: Reservation,
    workloads: Workloads,
    cancelled: WorkloadStartRecoveries,
    tasks: TaskTracker,
    cleanup: WorkloadStartResources,
    armed: bool,
    retain_failure: bool,
    #[cfg(feature = "washlet")]
    control: Option<Arc<super::HostControlLease>>,
}

impl WorkloadStartGuard {
    pub(super) fn new(host: &Host, workload_id: &str, reservation: Reservation) -> Self {
        Self {
            workload_id: workload_id.into(),
            reservation,
            workloads: Arc::clone(&host.workloads),
            cancelled: Arc::clone(&host.workload_start_recoveries),
            tasks: host.workload_start_cleanup_tasks.clone(),
            cleanup: WorkloadStartResources::default(),
            armed: true,
            retain_failure: false,
            #[cfg(feature = "washlet")]
            control: host
                .control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .upgrade(),
        }
    }

    pub(crate) fn cleanup(&self) -> WorkloadStartResources {
        self.cleanup.clone()
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    /// The start is answering its caller with cleanup still owed. A failure
    /// it reports then stays the workload's `Error` once recovery finishes,
    /// rather than being dropped with the id as a cancelled start's is.
    pub(crate) fn retain_failure(&mut self) {
        self.retain_failure = true;
    }
}

impl Drop for WorkloadStartGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let workload_id = self.workload_id.clone();
        let workloads = Arc::clone(&self.workloads);
        let cancelled = Arc::clone(&self.cancelled);
        let recovery = Arc::new(CancelledWorkloadStart {
            reservation: self.reservation,
            cleanup: self.cleanup.clone(),
            serial: tokio::sync::Mutex::new(()),
            retain_failure: self.retain_failure,
            #[cfg(feature = "washlet")]
            _control: self.control.take(),
        });
        cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(workload_id.clone(), Arc::clone(&recovery));
        // Spawning needs a runtime, and panicking here would do so inside a
        // destructor. The recovery is already recorded, so a start dropped off
        // the runtime is still cleaned up by a stop.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(
                workload_id,
                "workload start dropped outside the runtime; a workload stop is required to clean it up"
            );
            return;
        };
        // Track cleanup before the cancelled command can be joined.
        self.tasks.spawn_on(
            async move {
                if let Err(error) = recovery.recover(&workload_id, &workloads, &cancelled).await {
                    tracing::error!(workload_id, %error, "cancelled start requires a workload stop to retry cleanup");
                }
            },
            &runtime,
        );
    }
}
