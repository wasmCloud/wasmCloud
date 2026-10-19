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
    reader: Option<FootprintReader>,
    pub(super) failed_reads: u32,
}

impl Sampled {
    const fn new() -> Self {
        Self {
            guards: Vec::new(),
            running: false,
            reader: None,
            failed_reads: 0,
        }
    }

    fn read_and_publish(
        &mut self,
        read: impl FnOnce(&mut FootprintReader) -> Option<u64>,
        periodic: bool,
    ) {
        let reading = read(self.reader.get_or_insert_with(FootprintReader::new));
        match reading {
            Some(_) => self.failed_reads = 0,
            None if periodic => self.failed_reads = self.failed_reads.saturating_add(1),
            None => {}
        }
        for guard in self.guards.iter().filter_map(std::sync::Weak::upgrade) {
            match reading {
                Some(footprint) if periodic => guard.observe(footprint),
                Some(footprint) => guard.escalate(footprint),
                None if periodic && self.failed_reads >= MAX_FAILED_READS => guard.lose_reading(),
                None => {}
            }
        }
    }
}

pub(super) static SAMPLED: Mutex<Sampled> = Mutex::new(Sampled::new());

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

/// Refresh unless another caller read within [`REFRESH_INTERVAL`] or a
/// registry operation is in progress.
///
/// One caller per interval wins the clock and reads; the rest return at once,
/// without waiting on the reader or each other.
pub(super) fn refresh() {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);

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
        return;
    }
    try_refresh(&SAMPLED, FootprintReader::read);
}

fn try_refresh(
    sampled: &Mutex<Sampled>,
    read: impl FnOnce(&mut FootprintReader) -> Option<u64>,
) -> bool {
    let Ok(mut sampled) = sampled.try_lock() else {
        return false;
    };
    // Acquisition and publication share the lock: an older periodic sample
    // must never clear an over-limit decision from a newer refresh.
    sampled.read_and_publish(read, false);
    true
}

fn sample_once(
    sampled: &Mutex<Sampled>,
    read: impl FnOnce(&mut FootprintReader) -> Option<u64>,
) -> bool {
    let mut sampled = lock_ignoring_poison(sampled);
    sampled.guards.retain(|guard| guard.strong_count() > 0);
    if sampled.guards.is_empty() {
        sampled.running = false;
        return false;
    }
    sampled.read_and_publish(read, true);
    true
}

/// The process's one sampler: reads the footprint every [`SAMPLE_INTERVAL`]
/// and hands it to every live guard, until none is left.
pub(super) fn sample_forever() {
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        // Decided under the lock `spawn_sampler` takes, so a guard registered
        // as this thread gives up finds `running` cleared and starts another.
        if !sample_once(&SAMPLED, FootprintReader::read) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn refresh_cannot_overtake_an_older_periodic_read() {
        for older_reading in [Some(900), None] {
            let guard = Arc::new(ProcessMemoryGuard::new(1000));
            guard.observe(1100);
            let sampled = Mutex::new(Sampled {
                guards: vec![Arc::downgrade(&guard)],
                failed_reads: MAX_FAILED_READS - 1,
                ..Sampled::new()
            });
            let (reading_tx, reading_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            std::thread::scope(|scope| {
                let sampled = &sampled;
                let sampler = scope.spawn(move || {
                    sample_once(sampled, |_| {
                        reading_tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                        older_reading
                    })
                });
                reading_rx.recv().unwrap();
                let refreshed = try_refresh(sampled, |_| Some(1100));
                // Resume before asserting, so a failed assertion cannot leave
                // the scoped sampler waiting forever.
                resume_tx.send(()).unwrap();
                assert!(sampler.join().unwrap());
                assert!(!refreshed, "refresh must not overtake a pending sample");
                assert!(try_refresh(sampled, |_| Some(1100)));
            });
            assert!(guard.is_over());
            assert_eq!(guard.footprint_bytes(), 1100);
            assert!(!guard.blind.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn successful_refresh_breaks_a_streak_of_failed_periodic_reads() {
        let guard = Arc::new(ProcessMemoryGuard::new(1000));
        let sampled = Mutex::new(Sampled {
            guards: vec![Arc::downgrade(&guard)],
            failed_reads: MAX_FAILED_READS - 1,
            ..Sampled::new()
        });
        assert!(try_refresh(&sampled, |_| Some(1100)));
        assert!(sample_once(&sampled, |_| None));
        assert!(
            guard.is_over(),
            "a successful read restarts the failure count"
        );
        for _ in 1..MAX_FAILED_READS {
            assert!(sample_once(&sampled, |_| None));
        }
        assert!(
            !guard.is_over(),
            "five new failures still release stale pressure"
        );
    }
}
