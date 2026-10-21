//! The host's footprint, read the way its own memory guard reads it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use wash_runtime::engine::process_memory::pid_footprint;

/// The attached host's footprint now, or `None` without `--host-pid` or once
/// the process is gone.
pub fn footprint(pid: Option<u32>) -> Option<u64> {
    pid_footprint(pid?)
}

/// Samples the footprint in the background between the bench's own
/// measurements, so a step of starts halts as soon as the host reaches the
/// limit instead of at the end of the step.
pub struct Watch {
    pub over: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    unavailable: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Watch {
    /// `require_measurement`: a lost reading also halts, because nothing
    /// else bounds an uncapped run.
    pub fn spawn(
        read: impl Fn() -> Option<u64> + Send + 'static,
        limit: u64,
        halt: Arc<AtomicBool>,
        require_measurement: bool,
    ) -> Self {
        let over = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let unavailable = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let (over, peak, unavailable) = (over.clone(), peak.clone(), unavailable.clone());
            async move {
                let mut tick = tokio::time::interval(Duration::from_millis(100));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    if let Some(footprint) = read() {
                        peak.fetch_max(footprint, Ordering::Relaxed);
                        if footprint >= limit {
                            over.store(true, Ordering::Relaxed);
                            halt.store(true, Ordering::Relaxed);
                        }
                    } else if require_measurement {
                        unavailable.store(true, Ordering::Relaxed);
                        halt.store(true, Ordering::Relaxed);
                    }
                }
            }
        });
        Self {
            over,
            peak,
            unavailable,
            task,
        }
    }

    pub fn check_available(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.unavailable.load(Ordering::Relaxed),
            "host footprint became unavailable during an uncapped run"
        );
        Ok(())
    }

    pub fn peak(&self) -> Option<u64> {
        Some(self.peak.load(Ordering::Relaxed)).filter(|p| *p > 0)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_limit_and_lost_required_readings_halt_a_batch() {
        let halted = |read: fn() -> Option<u64>, required| async move {
            let halt = Arc::new(AtomicBool::new(false));
            let watch = Watch::spawn(read, 1024, halt.clone(), required);
            // Paused time: several sampling intervals pass at once.
            tokio::time::sleep(Duration::from_millis(350)).await;
            (halt.load(Ordering::Relaxed), watch)
        };
        let (halt, watch) = halted(|| None, false).await;
        assert!(!halt && watch.check_available().is_ok());
        let (halt, watch) = halted(|| None, true).await;
        assert!(halt && watch.check_available().is_err());
        let (halt, watch) = halted(|| Some(2048), true).await;
        assert!(halt && watch.over.load(Ordering::Relaxed));
        assert_eq!(watch.peak(), Some(2048));
    }

    #[test]
    fn reads_a_live_process_and_nothing_without_one() {
        assert!(footprint(Some(std::process::id())).is_some_and(|b| b > 0));
        assert_eq!(footprint(None), None);
    }
}
