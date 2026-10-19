//! Shared sampler lifecycle and throttled on-demand reads.
use super::footprint::FootprintReader;
use super::{MAX_FAILED_READS, ProcessMemoryGuard, REFRESH_INTERVAL, SAMPLE_INTERVAL};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

/// Every guard in the process, and whether a thread is sampling for them.
pub(super) struct Sampled {
    pub(super) guards: Vec<std::sync::Weak<ProcessMemoryGuard>>,
    pub(super) running: bool,
}

pub(super) static SAMPLED: Mutex<Sampled> = Mutex::new(Sampled {
    guards: Vec::new(),
    running: false,
});

/// A poisoned lock here only means a panic elsewhere mid-update of plain data.
pub(super) fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `read` over the live guards, under the registry's lock.
pub(super) fn live_guards<R>(
    read: impl FnOnce(&mut dyn Iterator<Item = Arc<ProcessMemoryGuard>>) -> R,
) -> R {
    let sampled = lock_ignoring_poison(&SAMPLED);
    read(&mut sampled.guards.iter().filter_map(std::sync::Weak::upgrade))
}

/// A reading for [`ProcessMemoryGuard::refresh`], or `None` when one was taken
/// within [`REFRESH_INTERVAL`] (or is being taken now) and the last stands.
///
/// One caller per interval wins the clock and reads; the rest return at once,
/// without waiting on the reader or each other.
pub(super) fn refresh_reading() -> Option<u64> {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);
    static READER: Mutex<Option<FootprintReader>> = Mutex::new(None);

    let now = u64::try_from(
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    let next = NEXT_MS.load(Ordering::Relaxed);
    if now < next
        || NEXT_MS
            .compare_exchange(
                next,
                now.saturating_add(REFRESH_INTERVAL.as_millis() as u64),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_err()
    {
        return None;
    }
    let mut reader = READER.try_lock().ok()?;
    reader.get_or_insert_with(FootprintReader::new).read()
}

/// The process's one sampler: reads the footprint every [`SAMPLE_INTERVAL`]
/// and hands it to every live guard, until none is left.
pub(super) fn sample_forever() {
    let mut reader = FootprintReader::new();
    let mut failed_reads = 0_u32;
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        let reading = reader.read();
        // Decided under the lock `spawn_sampler` takes, so a guard registered
        // as this thread gives up finds `running` cleared and starts another.
        let mut sampled = lock_ignoring_poison(&SAMPLED);
        sampled.guards.retain(|guard| guard.strong_count() > 0);
        if sampled.guards.is_empty() {
            sampled.running = false;
            return;
        }
        match reading {
            Some(_) => failed_reads = 0,
            None => failed_reads = failed_reads.saturating_add(1),
        }
        for guard in sampled.guards.iter().filter_map(std::sync::Weak::upgrade) {
            match reading {
                Some(footprint) => guard.observe(footprint),
                None if failed_reads >= MAX_FAILED_READS => guard.lose_reading(),
                None => {}
            }
        }
    }
}
