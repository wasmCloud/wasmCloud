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
//! - `async_query_streams_rows_incrementally` (`/paced`, after a `/warmup` call
//!   that keeps setup out of the timed window): eight ~9KB rows emitted ~120ms
//!   apart (a per-row `pg_sleep`), forwarded to the response body as each
//!   arrives. Timing chunk arrivals proves the rows stream through
//!   incrementally — a host that buffered the result set (`try_collect`) would
//!   withhold every row until the query finished and then emit one burst, so the
//!   arrivals would span almost no time at all. (The rows are large on purpose:
//!   postgres output-buffers small rows and flushes them together at query end,
//!   so only a row that overflows its ~8KB send buffer is delivered on its own.)
//!   Mirrors the byte-stream pacer test.
//!
//! Requires Docker; both are `#[ignore]` and run only under
//! `cargo test --include-ignored`.
#![cfg(feature = "wasmcloud-postgres")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::time::timeout;

mod common;
use common::postgres::{start_postgres, start_postgres_workload};
use common::streaming::time_arrivals;

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

/// Drive the fixture's `/warmup` route until it round-trips its row, so the
/// timed request that follows measures the query and not the setup around it:
/// the container may still be refusing connections, and the first call also pays
/// instantiating the guest and opening the plugin's connection.
async fn warm_up(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = "no response".to_string();
    while Instant::now() < deadline {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/warmup"))
                .header("HOST", host_header)
                .send(),
        )
        .await
        .context("warm-up request timed out")?
        .context("warm-up request failed")?;
        let status = response.status();
        let body = timeout(Duration::from_secs(10), response.text())
            .await
            .context("warm-up body timed out")?
            .context("warm-up body failed")?;
        if status.is_success() && body.trim() == "ok" {
            // Let the instance land back in the pool before the clock starts:
            // the workload runs `pool_size: 1`, and a call arriving while that
            // one instance is still checked out is served from a store of its
            // own — paying instantiation inside the timed window.
            tokio::time::sleep(Duration::from_millis(100)).await;
            return Ok(());
        }
        last = format!("{status} {:?}", body.trim());
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!(
        "warm-up never round-tripped a row (last response {last}); a staged fixture \
         predating the `/warmup` route answers from `items` instead — rebuild it with \
         `cargo xtask build-fixtures`"
    )
}

// A timing test wants the host's HTTP server, the guest's driver and this task
// on separate threads: on a current-thread runtime the guest's CPU work blocks
// the body-reading loop below, and pacing the host performed would be recorded
// as chunks arriving together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn async_query_streams_rows_incrementally() -> Result<()> {
    let (_container, host_addr) = start_postgres().await?;
    let (addr, _host) = start_postgres_workload(&host_addr, "pg-paced").await?;

    let client = reqwest::Client::new();
    warm_up(&client, addr, "pg-paced").await?;

    let start = Instant::now();
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

    let arrivals = time_arrivals(response, start).await?;

    // Eight rows paced ~120ms apart span the query's ~0.96s when the host
    // forwards each as it arrives; collected into one burst they span
    // milliseconds.
    arrivals.assert_streamed("the paced rows", Duration::from_millis(400));

    // The reassembled body must be all eight rows, in order and intact: each is
    // 9000 `x`s followed by a newline.
    let text = arrivals.text()?;
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
