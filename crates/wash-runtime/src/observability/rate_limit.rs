//! Warnings a caller can trigger on every request, let through a few times a
//! minute rather than on every one.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// A warning a client can trigger on every request, let through at most once
/// per interval. In between, the occurrences are logged at DEBUG and counted,
/// and the next warning says how many it stands for: a scan of unserved
/// hostnames should still be visible, not cost a log line per request.
///
/// Lock-free: when it fires on every request on every core (a full pool, say),
/// a mutex here would put them all in one queue on the error path.
pub(crate) struct RateLimitedWarn {
    interval_ms: u64,
    /// When the next warning may be logged, in milliseconds since
    /// [`warn_clock_ms`] started.
    next_ms: AtomicU64,
    suppressed: AtomicU64,
}

impl RateLimitedWarn {
    pub(crate) const fn new(interval: Duration) -> Self {
        Self {
            interval_ms: interval.as_millis() as u64,
            next_ms: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
        }
    }

    /// `Some(suppressed)` when this occurrence should be warned about, with
    /// how many were held back since the last warning; `None` when it is one
    /// of those.
    pub(crate) fn admit(&self) -> Option<u64> {
        self.admit_at(warn_clock_ms())
    }

    /// [`Self::admit`] at `now`, in milliseconds on the [`warn_clock_ms`] clock.
    fn admit_at(&self, now: u64) -> Option<u64> {
        let next = self.next_ms.load(Relaxed);
        if now >= next
            && self
                .next_ms
                .compare_exchange(next, now.saturating_add(self.interval_ms), Relaxed, Relaxed)
                .is_ok()
        {
            Some(self.suppressed.swap(0, Relaxed))
        } else {
            self.suppressed.fetch_add(1, Relaxed);
            None
        }
    }
}

fn warn_clock_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    u64::try_from(
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-request warning is let through once per interval, and the next
    /// one counts the occurrences held back in between.
    #[test]
    fn a_rate_limited_warning_counts_what_it_held_back() {
        let limiter = RateLimitedWarn::new(Duration::from_millis(50));
        assert_eq!(limiter.admit_at(0), Some(0));
        assert_eq!(limiter.admit_at(10), None);
        assert_eq!(limiter.admit_at(49), None);
        assert_eq!(limiter.admit_at(50), Some(2));
        assert_eq!(limiter.admit_at(60), None);
        assert_eq!(limiter.admit_at(1_000), Some(1));
    }
}
