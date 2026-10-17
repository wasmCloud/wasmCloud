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
    #[cfg(not(target_os = "macos"))]
    Sysinfo,
    #[cfg(target_os = "macos")]
    PhysFootprint,
    /// No process to read: attached to a host without `--host-pid`.
    Unknown,
}

impl MemorySource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::ProcStatus => "VmRSS + VmSwap from /proc/<pid>/status",
            #[cfg(not(target_os = "macos"))]
            Self::Sysinfo => "resident size from sysinfo (excludes swapped/compressed pages)",
            #[cfg(target_os = "macos")]
            Self::PhysFootprint => {
                "phys_footprint from proc_pid_rusage (includes compressed pages)"
            }
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
    #[cfg(not(target_os = "macos"))]
    system: std::sync::Mutex<sysinfo::System>,
}

impl Sampler {
    pub fn new(pid: Option<u32>) -> Self {
        let source = match pid {
            None => MemorySource::Unknown,
            Some(_) if cfg!(target_os = "linux") => MemorySource::ProcStatus,
            #[cfg(target_os = "macos")]
            Some(_) => MemorySource::PhysFootprint,
            #[cfg(not(target_os = "macos"))]
            Some(_) => MemorySource::Sysinfo,
        };
        Self {
            pid,
            source,
            #[cfg(not(target_os = "macos"))]
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
            #[cfg(not(target_os = "macos"))]
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
            #[cfg(target_os = "macos")]
            MemorySource::PhysFootprint => phys_footprint(pid),
            MemorySource::Unknown => None,
        }
    }
}

/// Same macOS accounting as the runtime guard, for the measured host PID.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn phys_footprint(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: the requested version matches the buffer's layout and size.
    // A vanished/inaccessible process returns an error instead of a reading.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, info.as_mut_ptr().cast()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: success filled the correctly sized, initially zeroed buffer.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_the_same_accounting_as_the_runtime_guard() {
        let sampler = Sampler::new(Some(std::process::id()));
        assert_eq!(sampler.source(), MemorySource::PhysFootprint);
        assert!(sampler.footprint().is_some_and(|n| n > 0));
        assert_eq!(phys_footprint(u32::MAX), None);
    }

    #[test]
    fn reads_this_process() {
        let sampler = Sampler::new(Some(std::process::id()));
        assert!(sampler.footprint().is_some_and(|b| b > 0));
        assert_eq!(Sampler::new(None).footprint(), None);
    }
}
