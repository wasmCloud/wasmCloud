//! What a binding has published, and what the host itself dropped on the way.
//!
//! The delivery side has counted its own shedding exactly since the core
//! backlog landed: `shed_total`, `shed_bytes`, attributed to a subject. The
//! publish side counted nothing, so a fan-out that loses most of its messages
//! is indistinguishable from one that never published them — no counter on
//! either side of the gap, and a core publish is fire-and-forget, so the guest
//! cannot tell either.
//!
//! This is the other end of that accounting. It reports in the same field
//! names as [`super::subscriber::record_shed`], deliberately, so the two ends
//! can be compared and a harness matching one matches both.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{info, warn};

/// How often a binding that is dropping publishes reports its running totals.
const REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Growth in the running drop total that earns a report before the interval is
/// up, for the same reason the shed report has one: a burst that finishes
/// inside one interval would otherwise be recorded at whatever the first
/// message made the total.
const REPORT_GROWTH_FACTOR: u64 = 10;

/// Why the host did not put a publish on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublishDrop {
    /// Refused before the wire — outside the grant, over the payload cap, or
    /// carrying a header the server would reject.
    Refused,
    /// Handed to the client and the client could not take it.
    Failed,
}

impl PublishDrop {
    fn reason(self) -> &'static str {
        match self {
            Self::Refused => "publish refused",
            Self::Failed => "publish failed",
        }
    }
}

/// Per-binding publish counters.
#[derive(Debug, Default)]
pub struct PublishLedger {
    published: AtomicU64,
    published_bytes: AtomicU64,
    dropped: AtomicU64,
    dropped_bytes: AtomicU64,
    /// The drop total as of the last report, so a line carries the window.
    reported: AtomicU64,
    last_report: Mutex<Option<tokio::time::Instant>>,
}

impl PublishLedger {
    /// Counts a publish the host put on the wire.
    ///
    /// "On the wire" is all a core publish can promise: it resolves once
    /// written to the connection, not once the server accepted it. That is
    /// precisely why this counter is worth keeping — it is the last point at
    /// which the message is known to exist, so a receiver that saw fewer bounds
    /// the loss to everything downstream of here.
    pub(super) fn published(&self, bytes: usize) {
        self.published.fetch_add(1, Ordering::Relaxed);
        self.published_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Counts a publish that never reached the wire, and reports the running
    /// totals no more than once per interval or order of magnitude.
    pub(super) fn dropped(&self, subject: &str, bytes: usize, kind: PublishDrop) {
        let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let total_bytes = self
            .dropped_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed)
            + bytes as u64;

        let now = tokio::time::Instant::now();
        let Ok(mut last_report) = self.last_report.lock() else {
            return;
        };
        let reported = self.reported.load(Ordering::Relaxed);
        let grew_by_an_order = total >= reported.saturating_mul(REPORT_GROWTH_FACTOR).max(1);
        if !last_report.is_none_or(|at| now.duration_since(at) >= REPORT_INTERVAL)
            && !grew_by_an_order
        {
            return;
        }
        let window = last_report.map(|at| now.duration_since(at));
        // `total` was read at the `fetch_add` above, outside the mutex, so a
        // thread descheduled between the two can resume with `reported` long
        // past it. The count is advisory; underflowing it is not.
        let since_last = total.saturating_sub(self.reported.swap(total, Ordering::Relaxed));
        *last_report = Some(now);
        warn!(
            subject = %subject,
            reason = kind.reason(),
            shed_total = total,
            shed_bytes = total_bytes,
            shed_in_window = since_last,
            window_ms = window.map(|w| w.as_millis() as u64).unwrap_or(0),
            published_total = self.published.load(Ordering::Relaxed),
            published_bytes = self.published_bytes.load(Ordering::Relaxed),
            "wasmcloud:nats is dropping publishes at the publish side"
        );
    }

    /// Reports the binding's lifetime totals.
    ///
    /// Emitted at unbind rather than only on the interval, because the figure
    /// this exists to provide is most useful for a run that has finished — and
    /// a run finishing inside one report interval is exactly the case an
    /// interval-only report gets wrong.
    pub(super) fn flush(&self, workload_id: &str, binding: &str) {
        let published = self.published.load(Ordering::Relaxed);
        let dropped = self.dropped.load(Ordering::Relaxed);
        if published == 0 && dropped == 0 {
            return;
        }
        info!(
            workload_id,
            binding,
            published_total = published,
            published_bytes = self.published_bytes.load(Ordering::Relaxed),
            shed_total = dropped,
            shed_bytes = self.dropped_bytes.load(Ordering::Relaxed),
            "wasmcloud:nats publish ledger at unbind: {published} messages reached the wire. \
             A receiver that saw fewer lost them downstream of this host, which nothing on \
             the publish side can observe."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_publish_side_counts_in_the_same_shape_as_the_delivery_side() {
        let ledger = PublishLedger::default();
        for _ in 0..10 {
            ledger.published(64);
        }
        for _ in 0..1_000 {
            ledger.dropped("fan.work", 64, PublishDrop::Failed);
        }
        assert_eq!(ledger.published.load(Ordering::Relaxed), 10);
        assert_eq!(ledger.published_bytes.load(Ordering::Relaxed), 640);
        assert_eq!(ledger.dropped.load(Ordering::Relaxed), 1_000);
        assert_eq!(ledger.dropped_bytes.load(Ordering::Relaxed), 64_000);
    }

    /// The same staleness guard the shed report has: a burst that finishes
    /// inside one interval must not be recorded at whatever its first message
    /// made the total.
    #[test]
    fn a_burst_inside_one_interval_is_not_reported_as_one_drop() {
        let ledger = PublishLedger::default();
        for _ in 0..10_000 {
            ledger.dropped("fan.work", 64, PublishDrop::Failed);
        }
        let reported = ledger.reported.load(Ordering::Relaxed);
        assert!(
            reported * REPORT_GROWTH_FACTOR >= ledger.dropped.load(Ordering::Relaxed),
            "last reported {reported} is more than {REPORT_GROWTH_FACTOR}x behind the truth"
        );
    }
}
