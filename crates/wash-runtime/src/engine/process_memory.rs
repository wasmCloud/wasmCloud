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
//! of every new instance (see [`GuestMemoryBudget::with_process_guard`]), and
//! new workloads (see [`ProcessMemoryGuard::admit_start`]), whose bookkeeping
//! grows the host even when they hold no instance. It cannot shed memory an
//! embedder holds, so it also says so loudly and exposes the reading for the
//! embedder to act on.
//!
//! A sampler reads the footprint every [`SAMPLE_INTERVAL`]. Growth and starts
//! also ask for a fresh reading, at most every [`REFRESH_INTERVAL`] across the
//! process, so a host growing fast is caught within a fraction of a second
//! rather than a second after it crossed.
//!
//! This is sampled admission control, not a hard allocation cap. Work already
//! admitted can still allocate; writes into previously committed guest memory
//! and arbitrary host/plugin buffers do not consult this guard. Starts recheck
//! after queueing and before pulls/compilation, but a large allocation can still
//! cross the limit between readings. Reserve headroom and use an OS/container
//! limit where a hard boundary is required.
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
//! | Linux | `RssAnon + VmSwap` from `/proc/self/status` (`VmRSS + VmSwap` on kernels without `RssAnon`) |
//! | elsewhere | the resident size `sysinfo` reports |
//!
//! On Linux, file and shmem pages are left out: `VmRSS` counts a shared page
//! once per mapping, and wasmtime maps each instance's copy-on-write memory
//! image separately, so it would count one copy of the image per warm
//! instance though memory holds one.
//!
//! [`GuestMemoryBudget`]: crate::engine::guest_memory::GuestMemoryBudget
//! [`GuestMemoryBudget::with_process_guard`]: crate::engine::guest_memory::GuestMemoryBudget::with_process_guard
//!
//! # When refusals stop
//!
//! Refusals start past the limit and stop once the footprint falls back under
//! [`RECOVER_PERCENT`] of it, so a host at the line does not flap. They also
//! stop when refusing is plainly not helping: memory the host itself retains
//! (the allocator, compiled code, warm instance pools) does not shrink because
//! guests are refused, and could otherwise hold the footprint in that band,
//! refusing every instantiation, for the rest of the process's life. So once
//! the footprint has stayed within the limit without growing for
//! [`OVER_RECHECK_SAMPLES`] samples, refusals stop and the host says so
//! loudly; they start again the moment it crosses the limit. Past the limit
//! itself refusals continue however flat the footprint is, with a loud
//! reminder each time the same period passes, because letting guests grow
//! there is what an OOM kill follows.
//!
//! A footprint that stops being readable cannot hold refusals in place either:
//! after [`MAX_FAILED_READS`] failed samples in a row the guard goes back to
//! normal and says the limit is not being enforced until a reading returns.

mod footprint;
mod metrics;
mod sampling;
#[cfg(test)]
use footprint::FootprintReader;
#[cfg(test)]
use footprint::linux_footprint;
pub use footprint::{pid_footprint, process_footprint};
use sampling::{SAMPLED, live_guards, lock_ignoring_poison, sample_forever};

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;

use crate::engine::host_memory::render_bytes;

/// How often the footprint is read. Reading it is a syscall or a small file
/// read; a host can grow by gigabytes in the seconds a slower sample misses.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// How old a reading may be when something about to add to the footprint asks
/// for one (see [`ProcessMemoryGuard::refresh`]). A host starting workloads
/// by the thousand a second grows by hundreds of megabytes in one
/// [`SAMPLE_INTERVAL`]; this bounds how far past the limit it gets before the
/// guard sees it, for about twenty reads a second at most.
pub const REFRESH_INTERVAL: Duration = Duration::from_millis(50);

/// Share of the limit at which the host warns that it is getting close.
const HIGH_PERCENT: u64 = 80;

/// Share of the limit the footprint must fall back under before a host that
/// was close is back to normal, so one hovering at [`HIGH_PERCENT`] does not
/// log a pair of lines on every sample.
const NORMAL_PERCENT: u64 = 75;

/// Share of the limit the footprint must fall back under before refusals stop,
/// so a host hovering at the limit does not flap on every sample.
const RECOVER_PERCENT: u64 = 95;

/// Samples an over-limit host may go without growing before the guard
/// reconsiders refusing (see the module docs): a minute at [`SAMPLE_INTERVAL`].
const OVER_RECHECK_SAMPLES: u32 = 60;

/// Share of the limit the footprint must grow by to count as growing while
/// over, so allocator noise does not keep the recheck from ever running.
const OVER_GROWTH_PERCENT: u64 = 1;

/// Failed footprint reads in a row after which the guard stops acting on its
/// last reading.
const MAX_FAILED_READS: u32 = 5;

/// Where the footprint stands against the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessMemoryState {
    /// Under [`HIGH_PERCENT`] of the limit, or back under [`NORMAL_PERCENT`]
    /// after being above it.
    Normal = 0,
    /// Close to the limit. Nothing is refused yet.
    High = 1,
    /// Past the limit. Guest memory growth and new instances are refused
    /// until the footprint falls back under [`RECOVER_PERCENT`] of it, or
    /// stops growing for a while (see the module docs).
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

/// The process's footprint against `max_process_memory`, shared by the
/// engine, its guest memory budget and whatever the embedder reports.
#[derive(Debug)]
pub struct ProcessMemoryGuard {
    limit: u64,
    footprint: AtomicU64,
    high_water: AtomicU64,
    state: AtomicU8,
    /// Set while the footprint cannot be read, so losing and regaining it are
    /// each said once.
    blind: AtomicBool,
    /// How long the footprint has been over without growing. The sampler
    /// writes it once a second; a [`Self::refresh`] only when it moves the
    /// state, so a guest's path does not take it on every call.
    over: Mutex<OverEpisode>,
    /// Workload starts refused while over (see [`Self::admit_start`]).
    refused_starts: AtomicU64,
}

/// One stretch of [`ProcessMemoryState::Over`], measured for growth.
#[derive(Debug, Default)]
struct OverEpisode {
    /// The footprint the current flat stretch is measured from.
    baseline: u64,
    /// Samples since the footprint last grew past `baseline`.
    flat_samples: u32,
    /// Whether this flat stretch is within the limit rather than past it.
    /// Crossing either way starts a new stretch, so refusals lift only after
    /// a whole period within the limit, not on the first sample after a
    /// period past it.
    within: bool,
}

impl ProcessMemoryGuard {
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            limit: limit_bytes,
            footprint: AtomicU64::new(0),
            high_water: AtomicU64::new(0),
            state: AtomicU8::new(ProcessMemoryState::Normal as u8),
            blind: AtomicBool::new(false),
            over: Mutex::new(OverEpisode::default()),
            refused_starts: AtomicU64::new(0),
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

    /// Workload starts [`Self::admit_start`] refused.
    pub fn refused_starts(&self) -> u64 {
        self.refused_starts.load(Ordering::Relaxed)
    }

    /// Read the footprint now, for a caller about to add to it, unless some
    /// caller in this process already did within [`REFRESH_INTERVAL`].
    ///
    /// The once-a-second sampler is too slow on its own for a host that grows
    /// fast: at a thousand workload starts a second the footprint can pass
    /// the limit and keep going for most of a second before a sample sees it.
    /// A refresh only ever moves the state up, into [`ProcessMemoryState::High`]
    /// or [`ProcessMemoryState::Over`]. Leaving them, and the recheck of a
    /// footprint that has stopped growing, stay with the sampler, so how soon
    /// refusals end does not depend on how much traffic is asking.
    ///
    /// Cheap when it does not read: one clock read and an atomic load.
    pub fn refresh(&self) {
        sampling::refresh();
    }

    /// Whether a workload may start, on a fresh reading: `Err` with the reason
    /// to report while the host is past its limit. A workload a host cannot
    /// back would only add the bookkeeping it is already short of, and a
    /// workload that never instantiates until its first request (`pool_size`
    /// 0) adds that bookkeeping without any guest growth for the guard to
    /// refuse. Counted, and logged a few times a minute however many arrive.
    pub fn admit_start(&self) -> Result<(), String> {
        self.refresh();
        if !self.is_over() {
            return Ok(());
        }
        let refused = self
            .refused_starts
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let message = format!(
            "this host is past max-process-memory ({} of {}) and accepts no new workloads until \
             it falls back below {RECOVER_PERCENT}% of it",
            render_bytes(self.footprint_bytes()),
            render_bytes(self.limit),
        );
        if let Some(suppressed) = START_REFUSAL_WARN.admit() {
            tracing::warn!(
                refused_starts = refused,
                suppressed,
                "refused a workload start: {message}. Give this host fewer workloads, or raise \
                 max-process-memory if the machine can back it"
            );
        }
        Err(message)
    }

    /// Take a reading that may only move the state up (see [`Self::refresh`]).
    fn escalate(&self, footprint_bytes: u64) {
        self.record_reading(footprint_bytes);
        let previous = self.state();
        let next = self.next_state(previous, footprint_bytes);
        if (next as u8) > (previous as u8) {
            self.transition(previous, next, footprint_bytes);
        }
    }

    /// Divided before multiplied so a `u64::MAX` limit cannot overflow.
    fn share(&self, percent: u64) -> u64 {
        self.limit / 100 * percent
    }

    fn record_reading(&self, footprint_bytes: u64) {
        self.footprint.store(footprint_bytes, Ordering::Relaxed);
        self.high_water
            .fetch_max(footprint_bytes, Ordering::Relaxed);
        if self.blind.swap(false, Ordering::Relaxed) {
            tracing::info!(
                footprint = %render_bytes(footprint_bytes),
                "this process's memory footprint is readable again; max-process-memory is \
                 enforced again"
            );
        }
    }

    /// Record one footprint reading and move between states, logging each
    /// move once rather than on every sample.
    pub fn observe(&self, footprint_bytes: u64) {
        self.record_reading(footprint_bytes);
        let previous = self.state();
        let mut next = self.next_state(previous, footprint_bytes);
        if next == ProcessMemoryState::Over && self.over_without_growing(footprint_bytes) {
            if footprint_bytes <= self.limit {
                tracing::warn!(
                    footprint = %render_bytes(footprint_bytes),
                    max_process_memory = %render_bytes(self.limit),
                    "this process has stayed close to max-process-memory without growing for \
                     {OVER_RECHECK_SAMPLES} samples. Refusing guests is not bringing it down, \
                     so whatever holds it is not a guest; guest memory growth is allowed again \
                     and will be refused if the footprint crosses the limit. Give this host \
                     fewer workloads, or raise max-process-memory if the machine can back it"
                );
                next = ProcessMemoryState::High;
            } else {
                tracing::error!(
                    footprint = %render_bytes(footprint_bytes),
                    max_process_memory = %render_bytes(self.limit),
                    "this process is still past max-process-memory and has not shrunk in \
                     {OVER_RECHECK_SAMPLES} samples. Guest memory growth and new instances \
                     stay refused; whatever holds the memory is not a guest"
                );
            }
        }
        self.transition(previous, next, footprint_bytes);
    }

    /// Note that a sample could not be read. After [`MAX_FAILED_READS`] in a
    /// row the guard stops acting on its last reading, which would otherwise
    /// hold whatever state it left behind for as long as reads keep failing.
    fn lose_reading(&self) {
        if self.blind.swap(true, Ordering::Relaxed) {
            return;
        }
        tracing::warn!(
            "cannot read this process's memory footprint ({MAX_FAILED_READS} samples in a row); \
             max-process-memory is not being enforced until it can be read again"
        );
        let previous = self.state();
        self.transition(previous, ProcessMemoryState::Normal, self.footprint_bytes());
    }

    /// Whether this sample ends [`OVER_RECHECK_SAMPLES`] of being over without
    /// growing, all on the same side of the limit (see `OverEpisode::within`).
    /// Restarts the count when it does, so the reconsideration comes round
    /// once per period rather than on every sample after the first.
    fn over_without_growing(&self, footprint: u64) -> bool {
        let mut over = self
            .over
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let within = footprint <= self.limit;
        if within != over.within
            || footprint
                > over
                    .baseline
                    .saturating_add(self.share(OVER_GROWTH_PERCENT))
        {
            *over = OverEpisode {
                baseline: footprint,
                flat_samples: 0,
                within,
            };
            return false;
        }
        over.flat_samples = over.flat_samples.saturating_add(1);
        if over.flat_samples < OVER_RECHECK_SAMPLES {
            return false;
        }
        over.flat_samples = 0;
        true
    }

    fn transition(&self, previous: ProcessMemoryState, next: ProcessMemoryState, footprint: u64) {
        if next != ProcessMemoryState::Over {
            // The next episode measures its growth from where it starts.
            *self
                .over
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = OverEpisode::default();
        }
        if next == previous {
            return;
        }
        self.state.store(next as u8, Ordering::Relaxed);
        let footprint = render_bytes(footprint);
        let limit = render_bytes(self.limit);
        match (previous, next) {
            (_, ProcessMemoryState::Over) => tracing::error!(
                %footprint,
                max_process_memory = %limit,
                "this process has outgrown max-process-memory. Guest memory growth and new \
                 instances are refused until it falls back below {RECOVER_PERCENT}% of it. \
                 Whatever is holding the rest is not a guest: give this host fewer workloads, \
                 or raise max-process-memory if the machine can back it"
            ),
            (ProcessMemoryState::Over, _) => tracing::warn!(
                %footprint,
                max_process_memory = %limit,
                state = next.as_str(),
                "this process is no longer refusing for max-process-memory; guest memory \
                 growth is allowed again"
            ),
            (_, ProcessMemoryState::High) => tracing::warn!(
                %footprint,
                max_process_memory = %limit,
                "this process is past {HIGH_PERCENT}% of max-process-memory"
            ),
            (_, ProcessMemoryState::Normal) => tracing::info!(
                %footprint,
                max_process_memory = %limit,
                "this process is back under {NORMAL_PERCENT}% of max-process-memory"
            ),
        }
    }

    fn next_state(&self, previous: ProcessMemoryState, footprint: u64) -> ProcessMemoryState {
        if footprint > self.limit {
            return ProcessMemoryState::Over;
        }
        if previous == ProcessMemoryState::Over && footprint >= self.share(RECOVER_PERCENT) {
            return ProcessMemoryState::Over;
        }
        if footprint >= self.share(HIGH_PERCENT) {
            return ProcessMemoryState::High;
        }
        // Between the two marks a host stays where it was, so one hovering at
        // the high mark does not flip on every sample.
        if previous != ProcessMemoryState::Normal && footprint >= self.share(NORMAL_PERCENT) {
            return ProcessMemoryState::High;
        }
        ProcessMemoryState::Normal
    }

    /// Say where the process stands, for the periodic host status line: at
    /// `info` while it is above [`HIGH_PERCENT`], `debug` otherwise.
    pub fn report(&self) {
        // Rendered in the fields, which are only evaluated when the line is
        // enabled: the usual case logs nothing at `debug`.
        if self.state() == ProcessMemoryState::Normal {
            tracing::debug!(
                footprint = %render_bytes(self.footprint_bytes()),
                high_water = %render_bytes(self.high_water_bytes()),
                max_process_memory = %render_bytes(self.limit),
                "process memory"
            );
        } else {
            tracing::info!(
                state = self.state().as_str(),
                footprint = %render_bytes(self.footprint_bytes()),
                high_water = %render_bytes(self.high_water_bytes()),
                max_process_memory = %render_bytes(self.limit),
                "process memory is close to or past max-process-memory"
            );
        }
    }

    /// Have the process's sampler feed this guard every [`SAMPLE_INTERVAL`].
    ///
    /// The footprint is the process's, not the engine's, so one thread reads
    /// it for every guard in the process, starting with the first guard and
    /// ending once the last is dropped. It holds the guards weakly, like the
    /// epoch ticker, so a dropped engine is not kept alive by it. Attempts an
    /// initial reading before returning; an unavailable reading is retried by
    /// the sampler rather than permanently disabling this guard.
    pub fn spawn_sampler(self: &Arc<Self>) -> anyhow::Result<()> {
        self.spawn_sampler_with_reading(process_footprint)
    }

    fn spawn_sampler_with_reading(
        self: &Arc<Self>,
        read: impl FnOnce() -> Option<u64>,
    ) -> anyhow::Result<()> {
        // Keep the initial read ordered with periodic and on-demand reads.
        let mut sampled = lock_ignoring_poison(&SAMPLED);
        match read() {
            Some(footprint) => {
                sampled.failed_reads = 0;
                self.observe(footprint);
            }
            None => {
                self.blind.store(true, Ordering::Relaxed);
                tracing::warn!(
                    "cannot read this process's initial memory footprint; \
                     max-process-memory is not enforced until a sample succeeds"
                );
            }
        }
        sampled.guards.retain(|guard| guard.strong_count() > 0);
        sampled.guards.push(Arc::downgrade(self));
        if sampled.running {
            return Ok(());
        }
        std::thread::Builder::new()
            .name("wash-process-memory".to_string())
            .spawn(sample_forever)
            .context("failed to spawn the process memory sampler thread")?;
        sampled.running = true;
        Ok(())
    }
}

/// Lets a refused start through at `warn` a few times a minute.
static START_REFUSAL_WARN: crate::observability::rate_limit::RateLimitedWarn =
    crate::observability::rate_limit::RateLimitedWarn::new(Duration::from_secs(10));

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn metrics_follow_provider_lifetimes_without_duplicate_series() -> anyhow::Result<()> {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
            data::{AggregatedMetrics, MetricData},
        };

        // An SDK without a reader creates no-op instruments. It must not
        // prevent a later provider from observing this process.
        ProcessMemoryGuard::register_metrics(&SdkMeterProvider::default().meter("test"));
        let guard = Arc::new(ProcessMemoryGuard::new(u64::MAX));
        guard.spawn_sampler()?;
        for _ in 0..2 {
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(exporter.clone()).build())
                .build();
            // Multiple engines using one provider still emit one series per
            // process gauge. The next iteration replaces that provider.
            ProcessMemoryGuard::register_metrics(&provider.meter("test"));
            ProcessMemoryGuard::register_metrics(&provider.meter("test"));
            provider.force_flush()?;
            let metrics = exporter.get_finished_metrics()?;
            let mut names = std::collections::BTreeSet::new();
            for metric in metrics
                .iter()
                .flat_map(|r| r.scope_metrics())
                .flat_map(|s| s.metrics())
            {
                if let AggregatedMetrics::U64(MetricData::Gauge(gauge)) = metric.data() {
                    assert_eq!(gauge.data_points().count(), 1);
                    names.insert(metric.name());
                }
            }
            assert_eq!(
                names,
                std::collections::BTreeSet::from([
                    "process_memory.footprint",
                    "process_memory.high_water",
                    "process_memory.limit"
                ])
            );
            provider.shutdown()?;
        }
        Ok(())
    }

    #[test]
    fn an_unreadable_initial_sample_is_retried() -> anyhow::Result<()> {
        let guard = Arc::new(ProcessMemoryGuard::new(1));
        guard.spawn_sampler_with_reading(|| None)?;
        // Exercise the real sampler registration and background reader, not
        // just observe(): the regression omitted registration on this path.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (guard.blind.load(Ordering::Relaxed) || !guard.is_over())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(guard.footprint_bytes() > 1);
        assert!(guard.is_over());
        assert!(!guard.blind.load(Ordering::Relaxed));
        assert!(guard.admit_start().is_err());
        Ok(())
    }

    #[test]
    fn the_footprint_of_this_process_is_readable() {
        let footprint = process_footprint().expect("the test platforms all have a reading");
        assert!(
            footprint > MIB,
            "a test binary is larger than 1 MiB: {footprint}"
        );
    }

    #[test]
    fn another_process_reads_like_this_one() {
        let by_pid = pid_footprint(std::process::id()).expect("this process is readable");
        assert!(by_pid > MIB, "a test binary is larger than 1 MiB: {by_pid}");
        assert_eq!(pid_footprint(u32::MAX), None);
    }

    #[test]
    fn linux_counts_swapped_pages() {
        let status = "Name:\tx\nVmRSS:\t  4096 kB\nRssAnon:\t  2048 kB\nVmSwap:\t  1024 kB\n";
        assert_eq!(linux_footprint(status), Some(3 * MIB));
        let no_swap = "VmRSS:\t  4096 kB\nRssAnon:\t  2048 kB\n";
        assert_eq!(linux_footprint(no_swap), Some(2 * MIB));
        assert_eq!(linux_footprint("Name:\tx\n"), None);
    }

    /// Image pages mapped into every warm instance are one copy in memory,
    /// however many times `VmRSS` counts them.
    #[test]
    fn linux_leaves_out_shared_file_and_shmem_pages() {
        let status = "VmRSS:\t 4196352 kB\nRssAnon:\t  2048 kB\nRssFile:\t  4096 kB\n\
                      RssShmem:\t 4190208 kB\nVmSwap:\t  1024 kB\n";
        assert_eq!(linux_footprint(status), Some(3 * MIB));
    }

    #[test]
    fn linux_without_rss_anon_falls_back_to_vm_rss() {
        let status = "VmRSS:\t  2048 kB\nVmSwap:\t  1024 kB\n";
        assert_eq!(linux_footprint(status), Some(3 * MIB));
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

    #[test]
    fn a_refreshed_reading_only_moves_the_state_up() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(100 * MIB);
        guard.escalate(1001 * MIB);
        assert!(guard.is_over(), "a reading past the limit trips at once");
        guard.escalate(10 * MIB);
        assert!(
            guard.is_over(),
            "leaving over is the sampler's call, not a refresh's"
        );
        assert_eq!(guard.footprint_bytes(), 10 * MIB);
        guard.observe(10 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::Normal);
    }

    #[test]
    fn starts_are_refused_and_counted_only_while_over() {
        // Not registered with the sampler, so the real footprint a refresh
        // reads never reaches it.
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(100 * MIB);
        assert!(guard.admit_start().is_ok());
        guard.observe(1001 * MIB);
        let refusal = guard.admit_start().expect_err("over the limit");
        assert!(refusal.contains("max-process-memory"), "{refusal}");
        assert_eq!(guard.refused_starts(), 1);
        guard.observe(10 * MIB);
        assert!(guard.admit_start().is_ok());
        assert_eq!(guard.refused_starts(), 1);
    }

    #[test]
    fn the_reader_can_be_read_again_and_again() {
        let mut reader = FootprintReader::new();
        for _ in 0..3 {
            assert!(reader.read().is_some_and(|footprint| footprint > MIB));
        }
    }

    #[test]
    fn leaving_high_needs_to_fall_under_the_lower_mark() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(810 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::High);
        guard.observe(790 * MIB);
        assert_eq!(
            guard.state(),
            ProcessMemoryState::High,
            "between the marks a high host stays high"
        );
        guard.observe(740 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::Normal);
        guard.observe(790 * MIB);
        assert_eq!(
            guard.state(),
            ProcessMemoryState::Normal,
            "between the marks a normal host stays normal"
        );
    }

    /// Memory the host holds, not its guests, can sit in the recovery band
    /// forever; refusing guests for ever after would not bring it down.
    #[test]
    fn a_flat_footprint_under_the_limit_stops_being_refused() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(1001 * MIB);
        assert!(guard.is_over());
        // The first sample within the limit starts the stretch; the period
        // runs from there.
        for _ in 0..OVER_RECHECK_SAMPLES {
            guard.observe(980 * MIB);
            assert!(guard.is_over());
        }
        guard.observe(980 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::High);
        guard.observe(985 * MIB);
        assert_eq!(guard.state(), ProcessMemoryState::High);
        guard.observe(1001 * MIB);
        assert!(guard.is_over(), "crossing the limit refuses again");
    }

    /// Time spent flat past the limit does not count towards time within it.
    #[test]
    fn a_flat_stretch_past_the_limit_does_not_lift_refusals_within_it() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(1002 * MIB);
        for _ in 1..OVER_RECHECK_SAMPLES {
            guard.observe(1002 * MIB);
        }
        guard.observe(990 * MIB);
        assert!(guard.is_over());
    }

    #[test]
    fn a_footprint_past_the_limit_stays_refused_however_flat() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(1001 * MIB);
        for _ in 0..OVER_RECHECK_SAMPLES * 2 {
            guard.observe(1002 * MIB);
        }
        assert!(guard.is_over());
    }

    #[test]
    fn an_unreadable_footprint_stops_refusing() {
        let guard = ProcessMemoryGuard::new(1000 * MIB);
        guard.observe(1001 * MIB);
        assert!(guard.is_over());
        guard.lose_reading();
        assert_eq!(guard.state(), ProcessMemoryState::Normal);
        guard.observe(1001 * MIB);
        assert!(guard.is_over(), "a reading that returns is acted on again");
    }
}
