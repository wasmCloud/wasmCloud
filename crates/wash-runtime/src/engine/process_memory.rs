//! The whole process's memory, held to what the machine can back.
//!
//! [`GuestMemoryBudget`] counts guest linear-memory growth and nothing else.
//! Everything a guest is not — compiled code, host buffers, an embedder's own
//! state — is invisible to it, and can grow a host past the machine while the
//! guest budget reads half empty. In a container the correction is an OOM
//! kill. On a laptop there may be none: macOS does not kill an ordinary
//! process for its size, it compresses and swaps until the whole machine
//! stalls.
//!
//! A [`ProcessMemoryGuard`] samples the process's real footprint and compares
//! it with `max_process_memory`. Past it, the host refuses what it can refuse
//! on its own account: guest memory growth, which includes the initial memory
//! of every new instance (see [`GuestMemoryBudget::with_process_guard`]). It
//! cannot shed memory an embedder holds, so it also says so loudly and
//! exposes the reading for the embedder to act on.
//!
//! # Footprint, not RSS
//!
//! Resident set size understates memory exactly when it matters: under
//! pressure, pages move to swap or to the macOS compressor and RSS *falls*
//! while the process keeps growing. So the reading counts those pages:
//!
//! | Platform | Reading |
//! | --- | --- |
//! | macOS | `phys_footprint` from `proc_pid_rusage`, what Activity Monitor shows as Memory: resident plus compressed |
//! | Linux | `VmRSS + VmSwap` from `/proc/self/status` |
//! | elsewhere | the resident size `sysinfo` reports |
//!
//! [`GuestMemoryBudget`]: crate::engine::guest_memory::GuestMemoryBudget
//! [`GuestMemoryBudget::with_process_guard`]: crate::engine::guest_memory::GuestMemoryBudget::with_process_guard

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Context as _;

use crate::engine::host_memory::render_bytes;

/// How often the footprint is read. Reading it is a syscall or a small file
/// read; a host can grow by gigabytes in the seconds a slower sample misses.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Share of the limit at which the host warns that it is getting close.
const HIGH_PERCENT: u64 = 80;

/// Share of the limit the footprint must fall back under before refusals stop,
/// so a host hovering at the limit does not flap on every sample.
const RECOVER_PERCENT: u64 = 95;

/// Where the footprint stands against the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessMemoryState {
    /// Under [`HIGH_PERCENT`] of the limit.
    Normal = 0,
    /// Close to the limit. Nothing is refused yet.
    High = 1,
    /// Past the limit. Guest memory growth and new instances are refused
    /// until the footprint falls back under [`RECOVER_PERCENT`] of it.
    Over = 2,
}

impl ProcessMemoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
            Self::Over => "over",
        }
    }

    fn from_u8(raw: u8) -> Self {
        match raw {
            2 => Self::Over,
            1 => Self::High,
            _ => Self::Normal,
        }
    }
}

/// This process's memory footprint, in bytes, counting pages the kernel has
/// swapped or compressed. `None` where it cannot be read.
pub fn process_footprint() -> Option<u64> {
    read_footprint()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn read_footprint() -> Option<u64> {
    let pid = libc::c_int::try_from(std::process::id()).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `RUSAGE_INFO_V2` tells the kernel to write one
    // `rusage_info_v2`, which is exactly what the buffer holds, for a pid that
    // is our own and therefore alive.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, info.as_mut_ptr().cast()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a zero return means the kernel filled the buffer, and it was
    // zeroed before that, so every field is initialised either way.
    Some(unsafe { info.assume_init() }.ri_phys_footprint)
}

#[cfg(target_os = "linux")]
fn read_footprint() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    linux_footprint(&status)
}

/// `VmRSS + VmSwap` out of `/proc/self/status`, in bytes.
#[cfg(any(target_os = "linux", test))]
fn linux_footprint(status: &str) -> Option<u64> {
    let kib = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().strip_suffix("kB"))
            .and_then(|n| n.trim().parse::<u64>().ok())
    };
    // Swap is absent on kernels built without it, which means none.
    let kib_total = kib("VmRSS:")?.saturating_add(kib("VmSwap:").unwrap_or(0));
    Some(kib_total.saturating_mul(1024))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_footprint() -> Option<u64> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let pid = sysinfo::get_current_pid().ok()?;
    let mut system = System::new_with_specifics(RefreshKind::new());
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::new().with_memory(),
    );
    system.process(pid).map(|process| process.memory())
}

/// The process's footprint against `max_process_memory`, shared by the
/// engine, its guest memory budget and whatever the embedder reports.
#[derive(Debug)]
pub struct ProcessMemoryGuard {
    limit: u64,
    footprint: AtomicU64,
    high_water: AtomicU64,
    state: AtomicU8,
}

impl ProcessMemoryGuard {
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            limit: limit_bytes,
            footprint: AtomicU64::new(0),
            high_water: AtomicU64::new(0),
            state: AtomicU8::new(ProcessMemoryState::Normal as u8),
        }
    }

    /// The `max_process_memory` this guard holds the process to, in bytes.
    pub fn limit_bytes(&self) -> u64 {
        self.limit
    }

    /// The footprint at the last sample, in bytes. Zero before the first, or where the
    /// footprint cannot be read.
    pub fn footprint_bytes(&self) -> u64 {
        self.footprint.load(Ordering::Relaxed)
    }

    /// The largest footprint sampled since the guard was built, in bytes.
    pub fn high_water_bytes(&self) -> u64 {
        self.high_water.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> ProcessMemoryState {
        ProcessMemoryState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// Whether the host is past its limit and refusing guest growth.
    pub fn is_over(&self) -> bool {
        self.state() == ProcessMemoryState::Over
    }

    /// Record one footprint reading and move between states, logging each
    /// move once rather than on every sample.
    pub fn observe(&self, footprint_bytes: u64) {
        self.footprint.store(footprint_bytes, Ordering::Relaxed);
        self.high_water
            .fetch_max(footprint_bytes, Ordering::Relaxed);

        let previous = self.state();
        let next = self.next_state(previous, footprint_bytes);
        if next == previous {
            return;
        }
        self.state.store(next as u8, Ordering::Relaxed);
        let footprint = render_bytes(footprint_bytes);
        let limit = render_bytes(self.limit);
        match next {
            ProcessMemoryState::Over => tracing::error!(
                %footprint,
                max_process_memory = %limit,
                "this process has outgrown max-process-memory. Guest memory growth and new \
                 instances are refused until it falls back below {RECOVER_PERCENT}% of it. \
                 Whatever is holding the rest is not a guest: give this host fewer workloads, \
                 or raise max-process-memory if the machine can back it"
            ),
            ProcessMemoryState::High if previous == ProcessMemoryState::Normal => tracing::warn!(
                %footprint,
                max_process_memory = %limit,
                "this process is past {HIGH_PERCENT}% of max-process-memory"
            ),
            ProcessMemoryState::High => tracing::warn!(
                %footprint,
                max_process_memory = %limit,
                "this process is back under max-process-memory; guest memory growth is \
                 allowed again"
            ),
            ProcessMemoryState::Normal => tracing::info!(
                %footprint,
                max_process_memory = %limit,
                "this process is back under {HIGH_PERCENT}% of max-process-memory"
            ),
        }
    }

    fn next_state(&self, previous: ProcessMemoryState, footprint: u64) -> ProcessMemoryState {
        // Divided before multiplied so a `u64::MAX` limit cannot overflow.
        let share = |percent: u64| self.limit / 100 * percent;
        if footprint > self.limit {
            return ProcessMemoryState::Over;
        }
        if previous == ProcessMemoryState::Over && footprint >= share(RECOVER_PERCENT) {
            return ProcessMemoryState::Over;
        }
        if footprint >= share(HIGH_PERCENT) {
            return ProcessMemoryState::High;
        }
        ProcessMemoryState::Normal
    }

    /// Say where the process stands, for the periodic host status line: at
    /// `info` while it is above [`HIGH_PERCENT`], `debug` otherwise.
    pub fn report(&self) {
        let state = self.state();
        let footprint = render_bytes(self.footprint_bytes());
        let high_water = render_bytes(self.high_water_bytes());
        let limit = render_bytes(self.limit);
        if state == ProcessMemoryState::Normal {
            tracing::debug!(%footprint, %high_water, max_process_memory = %limit, "process memory");
        } else {
            tracing::info!(
                state = state.as_str(),
                %footprint,
                %high_water,
                max_process_memory = %limit,
                "process memory is close to or past max-process-memory"
            );
        }
    }

    /// Start sampling the footprint every [`SAMPLE_INTERVAL`].
    ///
    /// Holds a weak reference, like the epoch ticker, so a dropped engine's
    /// guard ends its thread at the next tick. Takes one reading before
    /// returning, so the guard is never acting on a zero.
    pub fn spawn_sampler(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(first) = process_footprint() else {
            tracing::warn!(
                "cannot read this process's memory footprint on this platform; \
                 max-process-memory will not be enforced"
            );
            return Ok(());
        };
        self.observe(first);
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("wash-process-memory".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(SAMPLE_INTERVAL);
                    let Some(guard) = weak.upgrade() else {
                        return;
                    };
                    if let Some(footprint) = process_footprint() {
                        guard.observe(footprint);
                    }
                }
            })
            .context("failed to spawn the process memory sampler thread")?;
        Ok(())
    }

    /// Publish the guard as OpenTelemetry gauges, read on the exporter's own
    /// schedule like the guest memory budget's.
    pub(crate) fn register_metrics(self: &Arc<Self>, meter: &opentelemetry::metrics::Meter) {
        macro_rules! observe {
            ($name:literal, $doc:literal, $read:expr) => {{
                let guard = Arc::downgrade(self);
                let _ = meter
                    .u64_observable_gauge($name)
                    .with_description($doc)
                    .with_unit("By")
                    .with_callback(move |observer| {
                        if let Some(guard) = guard.upgrade() {
                            #[allow(clippy::redundant_closure_call)]
                            observer.observe($read(&guard), &[]);
                        }
                    })
                    .build();
            }};
        }
        observe!(
            "process_memory.footprint",
            "This process's memory footprint, counting swapped and compressed pages",
            |g: &Arc<Self>| g.footprint_bytes()
        );
        observe!(
            "process_memory.high_water",
            "The largest memory footprint sampled since this host started",
            |g: &Arc<Self>| g.high_water_bytes()
        );
        observe!(
            "process_memory.limit",
            "This host's max-process-memory",
            |g: &Arc<Self>| g.limit_bytes()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn the_footprint_of_this_process_is_readable() {
        let footprint = process_footprint().expect("the test platforms all have a reading");
        assert!(
            footprint > MIB,
            "a test binary is larger than 1 MiB: {footprint}"
        );
    }

    #[test]
    fn linux_counts_swapped_pages() {
        let status = "Name:\tx\nVmRSS:\t  2048 kB\nVmSwap:\t  1024 kB\n";
        assert_eq!(linux_footprint(status), Some(3 * MIB));
        let no_swap = "VmRSS:\t  2048 kB\n";
        assert_eq!(linux_footprint(no_swap), Some(2 * MIB));
        assert_eq!(linux_footprint("Name:\tx\n"), None);
    }

    #[test]
    fn crossing_the_limit_refuses_until_well_under_it() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(100 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::Normal);
        guard.observe(850 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::High);
        assert!(!guard.is_over());
        guard.observe(1001 * MIB);
        assert!(guard.is_over());
        // Back under the limit but not under the recovery mark: still over,
        // so a host at the line does not flap.
        guard.observe(990 * MIB);
        assert!(guard.is_over());
        guard.observe(900 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::High);
        guard.observe(10 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::Normal);
        assert_eq!(guard.high_water_bytes(), 1001 * MIB);
        assert_eq!(guard.footprint_bytes(), 10 * MIB);
    }

    #[test]
    fn an_unbounded_limit_never_trips() {
        let guard = ProcessMemoryGuard::new(u64::MAX);
        guard.observe(u64::MAX - 1);
        assert_ne!(guard.state(), ProcessMemoryState::Over);
    }
}
