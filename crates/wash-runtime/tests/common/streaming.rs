//! Timing a streamed HTTP response body.
//!
//! A streaming host forwards each piece of a body as it is produced, so a
//! client reading the body sees the arrivals spaced out over the time the guest
//! took to produce them. A host that collects the body before responding emits
//! it as one burst, whatever it was collecting.
//!
//! The *spread* between the first and last arrival is what separates the two,
//! and it is the only part of the measurement that survives a slow host: fixed
//! latency ahead of the first piece — a cold instance, a connection coming up,
//! a loaded machine — pushes every timestamp out together, leaving the spread
//! where it was. Comparing the first arrival against a fraction of the last
//! measures that latency instead, and reports it as buffering.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio::time::timeout;

/// A drained response body and when its chunks arrived.
pub struct Arrivals {
    /// The reassembled body.
    pub body: Vec<u8>,
    /// When the first non-empty chunk arrived.
    pub first_at: Duration,
    /// When the last one did.
    pub last_at: Duration,
}

impl Arrivals {
    /// How long the arrivals spanned.
    pub fn spread(&self) -> Duration {
        self.last_at - self.first_at
    }

    /// The body as text.
    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.clone()).context("body not utf8")
    }

    /// Assert the arrivals spanned at least `min_spread`, naming what was
    /// expected to stream. Pick a floor comfortably under the pacing the guest
    /// performs (half of it leaves room for a chunk or two coalescing) and well
    /// above the milliseconds a single burst spans.
    #[track_caller]
    pub fn assert_streamed(&self, what: &str, min_spread: Duration) {
        let spread = self.spread();
        assert!(
            spread >= min_spread,
            "{what} did not stream: all chunks landed within {spread:?}, expected them \
             spread over at least {min_spread:?} (first at {:?}, last at {:?})",
            self.first_at,
            self.last_at,
        );
    }
}

/// Drain `response`'s body, timing each chunk's arrival against `start` — an
/// [`Instant`] the caller takes *before* sending the request, so a host that
/// withholds the response until the body is complete cannot hide the wait in
/// the response head.
pub async fn time_arrivals(response: reqwest::Response, start: Instant) -> Result<Arrivals> {
    let mut stream = response.bytes_stream();
    let mut first_at: Option<Duration> = None;
    let mut last_at = Duration::ZERO;
    let mut body = Vec::new();
    while let Some(chunk) = timeout(Duration::from_secs(10), stream.next())
        .await
        .context("body chunk timed out")?
        .transpose()?
    {
        // Empty chunks carry no arrival: they are the transfer encoding's, not
        // the guest's.
        if chunk.is_empty() {
            continue;
        }
        let now = start.elapsed();
        first_at.get_or_insert(now);
        last_at = now;
        body.extend_from_slice(&chunk);
    }
    let first_at = first_at.context("response body was empty")?;
    Ok(Arrivals {
        body,
        first_at,
        last_at,
    })
}
