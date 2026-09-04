//! Pacing an accept loop that is failing.
//!
//! `accept` reports two unrelated things through one return value. An error
//! concerning the connection it just dequeued — a peer that went away, a
//! firewall verdict, a pending error Linux surfaces on the new socket — is over
//! as soon as it is read, and the next call proceeds. A condition of the
//! *process* is not: `EMFILE` leaves the connection queued, so every call
//! returns the same error until descriptors come back.
//!
//! Retrying the second at full speed pins a core and writes a log line per turn
//! while accepting nothing, which is how a live host stops answering its own
//! liveness probe.
//!
//! Only an error that leaves the connection queued can repeat, so only those are
//! paced. A count of consecutive failures cannot find them — descriptor
//! exhaustion under churn lets the occasional call succeed — so the pacing is on
//! their *rate*.

use core::time::Duration;
#[cfg(any(not(unix), test))]
use std::io::ErrorKind;
use std::time::Instant;

/// How many failures without a quiet [`FAILURE_WINDOW`] mean the loop is
/// spinning rather than meeting the occasional bad connection.
const FAILURES_BEFORE_BACKOFF: u32 = 1024;
const FAILURE_WINDOW: Duration = Duration::from_secs(1);

/// How long a spinning loop pauses between calls, and how far that grows. The
/// cap bounds how long the listener leaves its backlog alone.
const RETRY_MIN: Duration = Duration::from_millis(5);
const RETRY_MAX: Duration = Duration::from_secs(1);

/// Whether this error is the process's own rather than one connection's.
///
/// Named by errno, not by kind: `EMFILE` and the per-connection errors
/// `accept(2)` says to retry like `EAGAIN` — `EPROTO`, `ENOPROTOOPT`,
/// `ENETRESET` — all reach Rust as `ErrorKind::Uncategorized`, so a kind cannot
/// tell them apart. These four leave the connection queued, which is what makes
/// the next call fail identically.
#[cfg(unix)]
fn concerns_the_process(e: &std::io::Error) -> bool {
    use rustix::io::Errno;
    e.raw_os_error().is_some_and(|raw| {
        let errno = Errno::from_raw_os_error(raw);
        matches!(
            errno,
            Errno::MFILE | Errno::NFILE | Errno::NOBUFS | Errno::NOMEM
        )
    })
}

/// Without errnos to name, keep the kinds that are definitely one connection's
/// and treat the rest as the process's.
#[cfg(not(unix))]
fn concerns_the_process(e: &std::io::Error) -> bool {
    !matches!(
        e.kind(),
        ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::Interrupted
            | ErrorKind::WouldBlock
    )
}

/// Tracks how fast an accept loop is failing, and how long it should wait.
#[derive(Debug)]
pub(crate) struct AcceptBackoff {
    failures: u32,
    last_failure: Instant,
    retry_delay: Duration,
}

impl Default for AcceptBackoff {
    fn default() -> Self {
        Self {
            failures: 0,
            last_failure: Instant::now(),
            retry_delay: RETRY_MIN,
        }
    }
}

impl AcceptBackoff {
    /// How long to wait before the next `accept`, if the loop is failing fast
    /// enough to be spinning. Call once per turn, before accepting.
    ///
    /// Only a window with no failure in it clears the pause. Clearing on the
    /// window's age instead would reset mid-escalation — the escalating sleeps
    /// alone outlast a window — leaving the loop to re-spin its way to the
    /// threshold once a second, forever.
    ///
    /// Taking the delay before doubling it is what makes the first pause
    /// [`RETRY_MIN`] rather than twice it.
    pub(crate) fn pause(&mut self) -> Option<Duration> {
        if self.last_failure.elapsed() >= FAILURE_WINDOW {
            self.failures = 0;
            self.retry_delay = RETRY_MIN;
        }
        if self.failures <= FAILURES_BEFORE_BACKOFF {
            return None;
        }
        let taken = self.retry_delay;
        self.retry_delay = (self.retry_delay * 2).min(RETRY_MAX);
        Some(taken)
    }

    /// Record one failed `accept`. `true` when the loop is failing fast enough
    /// that the condition is the process's own and worth an `error!`.
    pub(crate) fn failed(&mut self, e: &std::io::Error) -> bool {
        if !concerns_the_process(e) {
            return false;
        }
        self.last_failure = Instant::now();
        self.failures = self.failures.saturating_add(1);
        self.failures > FAILURES_BEFORE_BACKOFF
    }

    /// How many failures have gone by without a quiet window, for the log line.
    pub(crate) fn failures(&self) -> u32 {
        self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emfile() -> std::io::Error {
        // `EMFILE` reaches Rust with no `ErrorKind` of its own, which is the
        // whole reason this module paces on a rate rather than on a kind.
        std::io::Error::from_raw_os_error(24)
    }

    fn spin(backoff: &mut AcceptBackoff) {
        for _ in 0..=FAILURES_BEFORE_BACKOFF {
            backoff.failed(&emfile());
        }
    }

    #[test]
    fn a_single_failure_never_pauses_the_loop() {
        let mut backoff = AcceptBackoff::default();
        assert!(!backoff.failed(&emfile()), "one failure is not a spin");
        assert_eq!(backoff.pause(), None);
    }

    /// A peer that connects and resets in a loop is not a reason to stop
    /// accepting. Counting those would let anyone throttle the listener for
    /// everyone else by doing something every TCP stack permits.
    #[test]
    fn errors_about_one_connection_do_not_pace_the_loop() {
        let mut backoff = AcceptBackoff::default();
        let per_connection: Vec<std::io::Error> = vec![
            ErrorKind::ConnectionAborted.into(),
            ErrorKind::ConnectionReset.into(),
            // `EPROTO`: `accept(2)` says to retry it like `EAGAIN`, and it
            // reaches Rust with the same uncategorized kind `EMFILE` does.
            #[cfg(target_os = "linux")]
            std::io::Error::from_raw_os_error(71),
            #[cfg(target_os = "macos")]
            std::io::Error::from_raw_os_error(100),
        ];
        for _ in 0..FAILURES_BEFORE_BACKOFF {
            for e in &per_connection {
                assert!(!backoff.failed(e), "{e:?} concerns one connection");
            }
        }
        assert_eq!(backoff.pause(), None, "a reset storm must not pause anyone");
        assert_eq!(backoff.failures(), 0);
    }

    #[test]
    fn the_pause_starts_at_the_minimum_and_grows_to_the_cap() {
        let mut backoff = AcceptBackoff::default();
        spin(&mut backoff);
        assert!(
            backoff.failed(&emfile()),
            "past the threshold this is worth reporting"
        );

        assert_eq!(
            backoff.pause(),
            Some(RETRY_MIN),
            "the first pause is the minimum, not twice it"
        );
        let mut last = RETRY_MIN;
        for _ in 0..64 {
            let Some(next) = backoff.pause() else {
                panic!("a loop over its threshold must keep pausing");
            };
            assert!(next >= last, "the delay only grows");
            last = next;
        }
        assert_eq!(last, RETRY_MAX, "and stops at the cap");
    }

    /// The pause has to end on its own, or a host that recovered would sleep
    /// before every accept forever.
    #[test]
    fn a_window_with_no_failure_in_it_clears_the_pause() {
        let mut backoff = AcceptBackoff::default();
        spin(&mut backoff);
        assert!(backoff.pause().is_some());

        backoff.last_failure = Instant::now() - FAILURE_WINDOW;
        assert_eq!(
            backoff.pause(),
            None,
            "a quiet window means the loop recovered"
        );
        assert_eq!(backoff.failures(), 0);
    }

    /// Sustained failure must not clear itself. The escalating sleeps outlast a
    /// window on their own, so ageing the window rather than the last failure
    /// would reset mid-escalation and re-spin to the threshold every second.
    #[test]
    fn sustained_failure_keeps_escalating_across_windows() {
        let mut backoff = AcceptBackoff::default();
        spin(&mut backoff);
        for _ in 0..12 {
            backoff.pause();
        }
        let escalated = backoff.retry_delay;

        // A window's worth of time passes, and the failures never stopped. Under
        // a reset keyed on the window's age rather than the last failure, this
        // is where the escalation would be thrown away.
        backoff.last_failure = Instant::now() - FAILURE_WINDOW;
        backoff.failed(&emfile());
        assert_eq!(
            backoff.pause(),
            Some(escalated),
            "a loop still failing must not be handed the minimum again"
        );
    }
}
