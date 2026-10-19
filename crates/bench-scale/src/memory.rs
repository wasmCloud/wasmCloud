//! The host's memory footprint, including its owned process group or job.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::Serialize;

/// Where footprint numbers come from; the report says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySource {
    /// Linux `/proc/<pid>/status`: `VmRSS + VmSwap`, so swapping out does not
    /// hide memory from the limit.
    ProcStatus,
    ProcStatusTree,
    /// sysinfo's resident size, elsewhere. Pages the OS compressed or swapped
    /// do not show up in it.
    #[cfg(not(target_os = "macos"))]
    Sysinfo,
    #[cfg(not(target_os = "macos"))]
    SysinfoTree,
    #[cfg(target_os = "macos")]
    PhysFootprint,
    #[cfg(target_os = "macos")]
    PhysFootprintTree,
    /// No process to read: attached to a host without `--host-pid`.
    Unknown,
}

impl MemorySource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::ProcStatus => "VmRSS + VmSwap from /proc/<pid>/status",
            Self::ProcStatusTree => "sum of VmRSS + VmSwap across the owned process group",
            #[cfg(not(target_os = "macos"))]
            Self::Sysinfo => "resident size from sysinfo (excludes swapped/compressed pages)",
            #[cfg(not(target_os = "macos"))]
            Self::SysinfoTree => "sum of resident sizes across the owned process job",
            #[cfg(target_os = "macos")]
            Self::PhysFootprint => {
                "phys_footprint from proc_pid_rusage (includes compressed pages)"
            }
            #[cfg(target_os = "macos")]
            Self::PhysFootprintTree => "sum of phys_footprint across the owned process group",
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
    owner: Option<Weak<Mutex<crate::host::ChildGuard>>>,
    system: Mutex<sysinfo::System>,
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
            owner: None,
            system: Mutex::new(sysinfo::System::new()),
        }
    }

    pub fn owned(pid: u32, owner: Weak<Mutex<crate::host::ChildGuard>>) -> Self {
        let mut sampler = Self::new(Some(pid));
        sampler.source = match sampler.source {
            MemorySource::ProcStatus => MemorySource::ProcStatusTree,
            #[cfg(not(target_os = "macos"))]
            MemorySource::Sysinfo => MemorySource::SysinfoTree,
            #[cfg(target_os = "macos")]
            MemorySource::PhysFootprint => MemorySource::PhysFootprintTree,
            source => source,
        };
        sampler.owner = Some(owner);
        sampler
    }

    pub fn source(&self) -> MemorySource {
        self.source
    }

    /// The footprint now, or `None` if unknown or the process is gone.
    pub fn footprint(&self) -> Option<u64> {
        use sysinfo::Pid;
        let pid = self.pid?;
        let ids = if let Some(owner) = &self.owner {
            // Refresh membership on every reading: wrappers can start children
            // after the baseline. A weak guard never extends the process/job life.
            let owner = owner.upgrade()?;
            let ids = owner.lock().ok()?.process_ids()?;
            if !ids.contains(&pid) {
                return None;
            }
            ids
        } else {
            vec![pid]
        };
        let system = self.system.lock().ok()?;
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let system = {
            use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
            let mut system = system;
            let pids: Vec<_> = ids.iter().copied().map(Pid::from_u32).collect();
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                true,
                ProcessRefreshKind::new().with_memory(),
            );
            system
        };
        ids.into_iter().try_fold(0_u64, |total, pid| {
            total.checked_add(self.process_footprint(Pid::from_u32(pid), &system)?)
        })
    }

    fn process_footprint(&self, pid: sysinfo::Pid, system: &sysinfo::System) -> Option<u64> {
        match self.source {
            MemorySource::ProcStatus | MemorySource::ProcStatusTree => {
                let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
                parse_proc_status(&status)
            }
            #[cfg(not(target_os = "macos"))]
            MemorySource::Sysinfo | MemorySource::SysinfoTree => {
                system.process(pid).map(sysinfo::Process::memory)
            }
            #[cfg(target_os = "macos")]
            MemorySource::PhysFootprint | MemorySource::PhysFootprintTree => {
                let _ = system;
                phys_footprint(pid.as_u32())
            }
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
    unavailable: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Watch {
    pub fn spawn(
        sampler: Arc<Sampler>,
        limit: u64,
        halt: Arc<AtomicBool>,
        require_measurement: bool,
    ) -> Self {
        let over = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let unavailable = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let over = Arc::clone(&over);
            let peak = Arc::clone(&peak);
            let unavailable = Arc::clone(&unavailable);
            async move {
                let mut tick = tokio::time::interval(Duration::from_millis(100));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    if let Some(footprint) = sampler.footprint() {
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
            "process footprint became unavailable during an uncapped run"
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
    async fn losing_required_measurements_halts_a_batch() {
        for required in [false, true] {
            let halt = Arc::new(AtomicBool::new(false));
            let watch = Watch::spawn(
                Arc::new(Sampler::new(None)),
                1024,
                Arc::clone(&halt),
                required,
            );
            tokio::time::sleep(Duration::from_millis(101)).await;
            assert_eq!(halt.load(Ordering::Relaxed), required);
            assert_eq!(watch.check_available().is_err(), required);
            assert!(!watch.over.load(Ordering::Relaxed));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn memory_fixture_command(role: &str, addr: std::net::SocketAddr) -> std::process::Command {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "memory::tests::owned_footprint_fixture",
                "--nocapture",
            ])
            .env("BENCH_SCALE_MEMORY_ROLE", role)
            .env("BENCH_SCALE_MEMORY_ADDR", addr.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    #[ignore = "subprocess fixture for owned-memory measurement"]
    fn owned_footprint_fixture() {
        use std::io::{Read as _, Write as _};
        let Ok(role) = std::env::var("BENCH_SCALE_MEMORY_ROLE") else {
            return;
        };
        let addr = std::env::var("BENCH_SCALE_MEMORY_ADDR")
            .unwrap()
            .parse()
            .unwrap();
        if role == "wrapper" {
            let mut child = memory_fixture_command("allocator", addr).spawn().unwrap();
            let _ = child.wait();
            return;
        }
        let mut socket = std::net::TcpStream::connect(addr).unwrap();
        socket.write_all(b"R").unwrap();
        socket.read_exact(&mut [0]).unwrap();
        // Commit incompressible pages only in the child, after the baseline.
        let mut allocation = vec![0_u8; 64 * 1024 * 1024];
        let mut state = 0x1234_5678_90ab_cdef_u64;
        for chunk in allocation.chunks_exact_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_ne_bytes());
        }
        std::hint::black_box(&allocation);
        socket.write_all(b"A").unwrap();
        // EOF releases the child if the parent test fails before killing it.
        let _ = socket.read_exact(&mut [0]);
        std::hint::black_box(allocation);
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[tokio::test]
    async fn child_memory_stops_an_owned_wrapper_run() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut command = memory_fixture_command("wrapper", listener.local_addr().unwrap());
        let guard = Arc::new(Mutex::new(
            crate::host::ChildGuard::spawn(&mut command).unwrap(),
        ));
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(20), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(socket.read_u8().await.unwrap(), b'R');
        let pid = guard.lock().unwrap().pid();
        let sampler = Arc::new(Sampler::owned(pid, Arc::downgrade(&guard)));
        let baseline = sampler.footprint().unwrap();
        let wrapper = Sampler::new(Some(pid));
        let wrapper_baseline = wrapper.footprint().unwrap();
        let limit = baseline + 32 * 1024 * 1024;
        let halt = Arc::new(AtomicBool::new(false));
        let watch = Watch::spawn(Arc::clone(&sampler), limit, Arc::clone(&halt), true);
        socket.write_all(b"G").await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(20), socket.read_u8())
                .await
                .unwrap()
                .unwrap(),
            b'A'
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while !halt.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        watch.check_available().unwrap();
        assert!(watch.over.load(Ordering::Relaxed));
        assert!(watch.peak().unwrap() >= limit);
        assert!(
            wrapper
                .footprint()
                .unwrap()
                .saturating_sub(wrapper_baseline)
                < 16 * 1024 * 1024
        );

        // Sampler/watch stay alive: their weak reference must not retain the
        // process group or the Windows kill-on-close job after owner cleanup.
        drop(guard);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), socket.read_u8())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(sampler.footprint(), None);
    }

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
