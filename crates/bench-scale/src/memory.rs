//! The host process's memory footprint, read from outside it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

/// Where footprint numbers come from; the report says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySource {
    /// Linux `/proc/<pid>/status`: `VmRSS + VmSwap`, so swapping out does not
    /// hide memory from the limit.
    ProcStatus,
    /// sysinfo's resident size, elsewhere. Pages the OS compressed or swapped
    /// do not show up in it.
    Sysinfo,
    /// No process to read: attached to a host without `--host-pid`.
    Unknown,
}

impl MemorySource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::ProcStatus => "VmRSS + VmSwap from /proc/<pid>/status",
            Self::Sysinfo => "resident size from sysinfo (excludes swapped/compressed pages)",
            Self::Unknown => "unknown (attached without --host-pid)",
        }
    }
}

/// `VmRSS + VmSwap` in bytes out of a `/proc/<pid>/status` body.
pub fn parse_proc_status(status: &str) -> Option<u64> {
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    };
    let rss = field("VmRSS:")?;
    Some(rss + field("VmSwap:").unwrap_or(0))
}

pub struct Sampler {
    pid: Option<u32>,
    source: MemorySource,
    system: std::sync::Mutex<sysinfo::System>,
}

impl Sampler {
    pub fn new(pid: Option<u32>) -> Self {
        let source = match pid {
            None => MemorySource::Unknown,
            Some(_) if cfg!(target_os = "linux") => MemorySource::ProcStatus,
            Some(_) => MemorySource::Sysinfo,
        };
        Self {
            pid,
            source,
            system: std::sync::Mutex::new(sysinfo::System::new()),
        }
    }

    pub fn source(&self) -> MemorySource {
        self.source
    }

    /// The footprint now, or `None` if unknown or the process is gone.
    pub fn footprint(&self) -> Option<u64> {
        let pid = self.pid?;
        match self.source {
            MemorySource::ProcStatus => {
                let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
                parse_proc_status(&status)
            }
            MemorySource::Sysinfo => {
                use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};
                let mut system = self.system.lock().ok()?;
                let pid = Pid::from_u32(pid);
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&[pid]),
                    true,
                    ProcessRefreshKind::new().with_memory(),
                );
                system.process(pid).map(sysinfo::Process::memory)
            }
            MemorySource::Unknown => None,
        }
    }
}

/// Samples the footprint in the background between the bench's own
/// measurements, so a step of starts halts as soon as the host reaches the
/// limit instead of at the end of the step.
pub struct Watch {
    pub over: Arc<AtomicBool>,
    pub peak: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl Watch {
    pub fn spawn(sampler: Arc<Sampler>, limit: u64, halt: Arc<AtomicBool>) -> Self {
        let over = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let task = tokio::spawn({
            let over = Arc::clone(&over);
            let peak = Arc::clone(&peak);
            async move {
                let mut tick = tokio::time::interval(Duration::from_millis(100));
                loop {
                    tick.tick().await;
                    if let Some(footprint) = sampler.footprint() {
                        peak.fetch_max(footprint, Ordering::Relaxed);
                        if footprint >= limit {
                            over.store(true, Ordering::Relaxed);
                            halt.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        Self { over, peak, task }
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

    #[test]
    fn reads_rss_and_swap() {
        let status = "Name:\twash\nVmPeak:\t 999999 kB\nVmRSS:\t   2048 kB\nRssAnon:\t 1000 kB\nVmSwap:\t     16 kB\n";
        assert_eq!(parse_proc_status(status), Some((2048 + 16) * 1024));
        assert_eq!(parse_proc_status("VmRSS:\t 4 kB\n"), Some(4096));
        assert_eq!(parse_proc_status("Name:\tzombie\n"), None);
    }

    #[test]
    fn reads_this_process() {
        let sampler = Sampler::new(Some(std::process::id()));
        assert!(sampler.footprint().is_some_and(|b| b > 0));
        assert_eq!(Sampler::new(None).footprint(), None);
    }
}
