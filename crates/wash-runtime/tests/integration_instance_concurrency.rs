//! A warm instance serves several calls at once when the component asks for it.
//!
//! `pool_size` alone gives a component parallelism — N instances on N threads —
//! but each instance still serves one call at a time. For a guest that spends
//! its call awaiting I/O that is the wrong shape: the instance sits idle while
//! queued work falls back to cold stores, so the pool has to be sized to peak
//! concurrency rather than to peak work.
//!
//! `max_concurrency` is the opt-in. It defaults to one, so a component that
//! only asked for `pool_size` behaves exactly as it did before — which matters,
//! because a guest driving its own executor (anything calling `block_on`) would
//! have a second concurrent call try to enter that executor from inside itself.
//!
//! The `http-sleeper` fixture reports the peak number of calls its instance had
//! in flight at once, and that peak is the signal these tests read. Wall clock
//! is deliberately *not*: a request the warm set cannot take is served from a
//! store of its own and runs in parallel anyway, so both configurations finish
//! a burst in about the same time. What differs is where the calls ran, and
//! therefore how much per-instance setup was paid — which the fixture charges
//! once per instance for exactly this reason.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::{Context, Result};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");

async fn start_sleeper(
    host_header: &'static str,
    pool_size: i32,
    max_concurrency: i32,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                pool_size,
                max_invocations: 0,
                max_concurrency,
            }],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    })
    .await
    .context("sleeper workload should start")?;
    Ok((addr, host))
}

/// Fire `n` requests at once; return the highest `peak_in_flight` any instance
/// reported.
async fn burst(addr: std::net::SocketAddr, host_header: &'static str, n: usize) -> Result<u64> {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .build()?;
    let mut tasks = Vec::with_capacity(n);
    for _ in 0..n {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            let resp = client
                .get(format!("http://{addr}/"))
                .header("HOST", host_header)
                .send()
                .await?;
            anyhow::ensure!(resp.status().is_success(), "got {}", resp.status());
            Ok::<u64, anyhow::Error>(json_u64_field(&resp.text().await?, "peak_in_flight"))
        }));
    }
    let mut peak = 0;
    for t in tasks {
        peak = peak.max(t.await.context("request task panicked")??);
    }
    Ok(peak)
}

/// The default. One warm instance, `max_concurrency` unset: it takes one call
/// and every other request is served from a store of its own, so no instance
/// ever sees two calls at once and the peak stays at one.
#[tokio::test]
async fn without_max_concurrency_calls_do_not_overlap_on_an_instance() -> Result<()> {
    let (addr, _host) = start_sleeper("conc-off", 1, 1).await?;

    let peak = burst(addr, "conc-off", 4).await?;

    assert_eq!(
        peak, 1,
        "an instance must serve one call at a time unless the component asked for more"
    );
    Ok(())
}

/// The opt-in. One warm instance with `max_concurrency: 8` takes all four
/// calls at once, so the peak reaches four — all of them served by the one
/// instance that had already paid its setup, rather than three of them by
/// fresh stores paying it again.
#[tokio::test]
async fn max_concurrency_overlaps_calls_on_one_instance() -> Result<()> {
    let (addr, _host) = start_sleeper("conc-on", 1, 8).await?;

    let peak = burst(addr, "conc-on", 4).await?;

    assert_eq!(
        peak, 4,
        "all four calls should have been in flight on the one instance at once"
    );
    Ok(())
}

/// Concurrency composes with `pool_size` rather than replacing it: two
/// instances at two calls each cover the burst, and no instance exceeds its own
/// limit.
#[tokio::test]
async fn concurrency_is_bounded_per_instance() -> Result<()> {
    let (addr, _host) = start_sleeper("conc-bounded", 2, 2).await?;

    let peak = burst(addr, "conc-bounded", 4).await?;

    assert!(
        (1..=2).contains(&peak),
        "no instance may exceed max_concurrency of 2, saw {peak}"
    );
    Ok(())
}
