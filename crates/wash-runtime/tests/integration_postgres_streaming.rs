//! Streaming and backpressure e2e for the async `wasmcloud:postgres@0.2.0`
//! `query`, whose result is a `stream<row>` + completion `future`.
//!
//! The `postgres-stream-p3` fixture picks its query by request path (see that
//! fixture); these tests drive two:
//!
//! - `async_query_streams_a_huge_result_set` (`/huge`): a 50k-row
//!   `generate_series`. The guest reduces it to a count/sum without holding the
//!   rows, so the whole set flows through the host's bounded (16-row) channel a
//!   handful of rows at a time. An exact count+sum proves every row arrived,
//!   in order and once, at a scale far beyond any buffer.
//!
//! - `async_query_streams_rows_incrementally` (`/paced`): eight ~9KB rows
//!   emitted ~120ms apart (a per-row `pg_sleep`), forwarded to the response body
//!   as each arrives. Timing chunk arrivals from *before* the request proves the
//!   rows stream through incrementally — a host that buffered the result set
//!   (`try_collect`) would withhold every row until the query finished, so the
//!   first chunk would land with the last. (The rows are large on purpose:
//!   postgres output-buffers small rows and flushes them together at query end,
//!   so only a row that overflows its ~8KB send buffer is delivered on its own.)
//!   Mirrors the byte-stream pacer test.
//!
//! Requires Docker; both are `#[ignore]` and run only under
//! `cargo test --include-ignored`.
#![cfg(feature = "wasmcloud-postgres")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio::time::timeout;

mod common;
use common::postgres::{start_postgres, start_postgres_workload};

#[tokio::test]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn async_query_streams_a_huge_result_set() -> Result<()> {
    let (_container, host_addr) = start_postgres().await?;
    let (addr, _host) = start_postgres_workload(&host_addr, "pg-huge").await?;

    // `/huge` streams 50_000 rows through the host's 16-row channel; the guest
    // keeps only a running count/sum. `generate_series(1, 50_000)` needs no
    // table, so there is nothing to seed.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(60),
        client
            .get(format!("http://{addr}/huge"))
            .header("HOST", "pg-huge")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");

    // sum(1..=50_000) = 50_000 * 50_001 / 2 = 1_250_025_000. `cols=n` also
    // confirms the query's returned `list<column-name>` (the aliased column)
    // threads back through the host's streaming binding.
    assert_eq!(
        body, "count=50000 sum=1250025000 cols=n",
        "every one of the 50k streamed rows should be counted and summed exactly, \
         and the column name returned"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn async_query_streams_rows_incrementally() -> Result<()> {
    let (_container, host_addr) = start_postgres().await?;
    let (addr, _host) = start_postgres_workload(&host_addr, "pg-paced").await?;

    // Time from before the request: a buffering host withholds the whole result
    // set, so the response — and its first chunk — cannot arrive until the query
    // finishes (~0.96s), landing first ≈ last.
    let start = Instant::now();
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/paced"))
            .header("HOST", "pg-paced")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    assert!(
        response.status().is_success(),
        "paced handler should return 2xx, got {}",
        response.status()
    );

    let mut stream = response.bytes_stream();
    let mut first_at: Option<Duration> = None;
    let mut last_at = Duration::ZERO;
    let mut body = Vec::new();
    while let Some(chunk) = timeout(Duration::from_secs(10), stream.next())
        .await
        .context("body chunk timed out")?
        .transpose()?
    {
        if chunk.is_empty() {
            continue;
        }
        let now = start.elapsed();
        first_at.get_or_insert(now);
        last_at = now;
        body.extend_from_slice(&chunk);
    }
    let first_at = first_at.context("response body was empty")?;

    // Sanity: the query really did pace its rows over time (8 × ~120ms). Guards
    // against a degenerate instant result that would make the check below vacuous.
    assert!(
        last_at >= Duration::from_millis(500),
        "expected paced rows to span >=500ms, last chunk at {last_at:?}"
    );

    // The streaming assertion: the first row must arrive well before the last.
    // Under a buffered (`try_collect`) host, first ≈ last and this fails.
    assert!(
        first_at < last_at / 2,
        "rows were buffered, not streamed: first chunk at {first_at:?}, last at {last_at:?} \
         (first should be < last/2)"
    );

    // The reassembled body must be all eight rows, in order and intact: each is
    // 9000 `x`s followed by a newline.
    let text = String::from_utf8(body).context("body not utf8")?;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        8,
        "expected 8 streamed rows, got {}",
        lines.len()
    );
    assert!(
        lines
            .iter()
            .all(|l| l.len() == 9000 && l.bytes().all(|b| b == b'x')),
        "each streamed row should be 9000 'x' bytes"
    );

    Ok(())
}
