//! Teardown owned by a workload reservation, even after its start is cancelled.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use tokio::sync::RwLock;
use tokio_util::task::TaskTracker;

use super::{Host, HostWorkload, Reservation};
use crate::engine::workload::StartCleanup;

type Workloads = Arc<RwLock<HashMap<String, HostWorkload>>>;
pub(super) type CancelledStarts = Arc<Mutex<HashMap<String, Arc<CancelledStart>>>>;

pub(super) struct CancelledStart {
    reservation: Reservation,
    cleanup: StartCleanup,
    serial: tokio::sync::Mutex<()>,
    #[cfg(feature = "washlet")]
    _control: Option<Arc<super::HostControlLease>>,
}

impl CancelledStart {
    pub(super) async fn recover(
        &self,
        workload_id: &str,
        workloads: &Workloads,
        cancelled: &CancelledStarts,
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
                Some(HostWorkload::Starting(held) | HostWorkload::Stopping(held))
                    if *held == self.reservation =>
                {
                    workloads.insert(workload_id.into(), HostWorkload::Stopping(self.reservation));
                }
                None => return Ok(()),
                _ => anyhow::bail!("cancelled start no longer owns its workload reservation"),
            }
        }
        tokio::time::timeout(
            crate::timeouts::plugin_stop(),
            self.cleanup.release(workload_id),
        )
        .await
        .context("cancelled workload cleanup timed out")??;
        let mut workloads = workloads.write().await;
        if matches!(workloads.get(workload_id), Some(HostWorkload::Stopping(held)) if *held == self.reservation)
        {
            // Remove recovery bookkeeping while the ID is still held.
            let mut cancelled = cancelled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cancelled
                .get(workload_id)
                .is_some_and(|entry| entry.reservation == self.reservation)
            {
                cancelled.remove(workload_id);
            }
            workloads.remove(workload_id);
        }
        Ok(())
    }
}

pub(crate) struct StartGuard {
    workload_id: String,
    reservation: Reservation,
    workloads: Workloads,
    cancelled: CancelledStarts,
    tasks: TaskTracker,
    cleanup: StartCleanup,
    armed: bool,
    #[cfg(feature = "washlet")]
    control: Option<Arc<super::HostControlLease>>,
}

impl StartGuard {
    pub(super) fn new(host: &Host, workload_id: &str, reservation: Reservation) -> Self {
        Self {
            workload_id: workload_id.into(),
            reservation,
            workloads: Arc::clone(&host.workloads),
            cancelled: Arc::clone(&host.cancelled_starts),
            tasks: host.start_cleanup.clone(),
            cleanup: StartCleanup::default(),
            armed: true,
            #[cfg(feature = "washlet")]
            control: host
                .control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .upgrade(),
        }
    }

    pub(crate) fn cleanup(&self) -> StartCleanup {
        self.cleanup.clone()
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let workload_id = self.workload_id.clone();
        let workloads = Arc::clone(&self.workloads);
        let cancelled = Arc::clone(&self.cancelled);
        let recovery = Arc::new(CancelledStart {
            reservation: self.reservation,
            cleanup: self.cleanup.clone(),
            serial: tokio::sync::Mutex::new(()),
            #[cfg(feature = "washlet")]
            _control: self.control.take(),
        });
        cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(workload_id.clone(), Arc::clone(&recovery));
        // Track cleanup before the cancelled command can be joined.
        self.tasks.spawn(async move {
            if let Err(error) = recovery.recover(&workload_id, &workloads, &cancelled).await {
                tracing::error!(workload_id, %error, "cancelled start requires a workload stop to retry cleanup");
            }
        });
    }
}
