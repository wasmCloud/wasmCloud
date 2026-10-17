//! Attached workloads outlive a dropped RPC future. Keep their IDs until removal
//! is confirmed, including starts whose reply was lost.

use crate::api::{HostClient, WorkloadState};
use anyhow::{Result, bail};
use futures::{StreamExt as _, stream};
use std::{collections::BTreeMap, future::Future, sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct Journal(Mutex<BTreeMap<String, bool>>);

impl Journal {
    pub fn submitted(&self, id: &str) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.into())
            .or_insert(false);
    }
    pub fn acknowledged(&self, id: &str) {
        if let Some(ack) = self.0.lock().unwrap_or_else(|p| p.into_inner()).get_mut(id) {
            *ack = true;
        }
    }
    fn entries(&self) -> BTreeMap<String, bool> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

pub async fn cancellable<T>(
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("interrupted"),
        result = work => result,
    }
}

pub trait Control {
    async fn stop(&self, id: &str) -> Result<WorkloadState>;
    async fn status(&self, id: &str) -> Result<WorkloadState>;
}

impl Control for HostClient {
    async fn stop(&self, id: &str) -> Result<WorkloadState> {
        Ok(self
            .stop(id, Duration::from_secs(5))
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
            .workload_state)
    }
    async fn status(&self, id: &str) -> Result<WorkloadState> {
        Ok(self
            .status(id, Duration::from_secs(5))
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
            .workload_state)
    }
}

pub async fn with_cleanup<T>(
    cancel: &CancellationToken,
    client: &impl Control,
    journal: &Journal,
    attached: bool,
    concurrency: usize,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let result = cancellable(cancel, work).await;
    if attached {
        // Cleanup deliberately ignores cancellation and has its own finite deadline.
        let residual = cleanup(
            client,
            journal.entries(),
            concurrency,
            Duration::from_secs(60),
        )
        .await;
        if !residual.is_empty() {
            bail!(
                "cleanup could not confirm removal of workload IDs: {}; run result: {}",
                residual.join(", "),
                result
                    .as_ref()
                    .err()
                    .map_or_else(|| "completed".into(), |e| format!("{e:#}"))
            );
        }
    }
    result
}

async fn cleanup(
    client: &impl Control,
    entries: BTreeMap<String, bool>,
    concurrency: usize,
    budget: Duration,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + budget;
    stream::iter(entries)
        .map(|(id, acknowledged)| async move {
            let removed = tokio::time::timeout_at(deadline, async {
                let mut seen = acknowledged;
                loop {
                    if let Ok(state) = client.stop(&id).await {
                        seen |= matches!(
                            state,
                            WorkloadState::Starting
                                | WorkloadState::Running
                                | WorkloadState::Stopping
                                | WorkloadState::Completed
                        );
                    }
                    match client.status(&id).await {
                        Ok(WorkloadState::NotFound | WorkloadState::Completed) if seen => return,
                        Ok(
                            WorkloadState::Starting
                            | WorkloadState::Running
                            | WorkloadState::Stopping
                            | WorkloadState::Error,
                        ) => seen = true,
                        _ => {}
                    }
                    // An unanswered start followed only by NotFound is ambiguous: it
                    // may still be queued at the host. Report it rather than claim cleanup.
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
            .await
            .is_ok();
            (!removed).then_some(id)
        })
        .buffer_unordered(concurrency.max(1))
        .filter_map(|id| async { id })
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Host {
        running: AtomicBool,
        fail_stops: AtomicUsize,
    }
    impl Control for Host {
        async fn stop(&self, _: &str) -> Result<WorkloadState> {
            let failures = self.fail_stops.load(Ordering::SeqCst);
            if failures > 0
                && self
                    .fail_stops
                    .compare_exchange(failures, failures - 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                bail!("transient stop error");
            }
            Ok(if self.running.swap(false, Ordering::SeqCst) {
                WorkloadState::Completed
            } else {
                WorkloadState::NotFound
            })
        }
        async fn status(&self, _: &str) -> Result<WorkloadState> {
            Ok(if self.running.load(Ordering::SeqCst) {
                WorkloadState::Running
            } else {
                WorkloadState::NotFound
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn interruption_cleans_an_unanswered_start_and_retries_stop() {
        let host = Host {
            running: AtomicBool::new(false),
            fail_stops: AtomicUsize::new(1),
        };
        let journal = Journal::default();
        let cancel = CancellationToken::new();
        let result: Result<()> = with_cleanup(&cancel, &host, &journal, true, 2, async {
            journal.submitted("lost-reply");
            host.running.store(true, Ordering::SeqCst);
            cancel.cancel();
            std::future::pending().await
        })
        .await;
        assert_eq!(result.unwrap_err().to_string(), "interrupted");
        assert!(!host.running.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn early_errors_also_clean_acknowledged_starts() {
        let host = Host {
            running: AtomicBool::new(true),
            fail_stops: AtomicUsize::new(0),
        };
        let journal = Journal::default();
        journal.submitted("started");
        journal.acknowledged("started");
        let result: Result<()> =
            with_cleanup(&CancellationToken::new(), &host, &journal, true, 1, async {
                bail!("measurement failed")
            })
            .await;
        assert_eq!(result.unwrap_err().to_string(), "measurement failed");
        assert!(!host.running.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn a_start_that_finishes_during_cleanup_is_removed() {
        let host = std::sync::Arc::new(Host {
            running: AtomicBool::new(false),
            fail_stops: AtomicUsize::new(0),
        });
        let delayed = host.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            delayed.running.store(true, Ordering::SeqCst);
        });
        let remaining = cleanup(
            host.as_ref(),
            [("late".into(), false)].into(),
            1,
            Duration::from_secs(5),
        )
        .await;
        assert!(remaining.is_empty());
        assert!(!host.running.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn uncertain_and_failed_removals_report_residual_ids() {
        let host = Host {
            running: AtomicBool::new(false),
            fail_stops: AtomicUsize::new(0),
        };
        assert_eq!(
            cleanup(
                &host,
                [("unanswered".into(), false)].into(),
                1,
                Duration::from_secs(1)
            )
            .await,
            ["unanswered"]
        );
        host.running.store(true, Ordering::SeqCst);
        host.fail_stops.store(usize::MAX, Ordering::SeqCst);
        assert_eq!(
            cleanup(
                &host,
                [("stuck".into(), true)].into(),
                1,
                Duration::from_secs(1)
            )
            .await,
            ["stuck"]
        );
    }
}
