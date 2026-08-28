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
use std::time::Duration;

use anyhow::{Context, Result};

use wash_runtime::host::HostApi;
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest,
};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");
/// A component that calls a linked one per request, and the callee it calls.
const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");

async fn start_sleeper(
    host_header: &'static str,
    pool_size: i32,
    max_concurrency: i32,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let (addr, host, _id) = start_sleeper_with(host_header, pool_size, max_concurrency, 0).await?;
    Ok((addr, host))
}

async fn start_sleeper_with(
    host_header: &'static str,
    pool_size: i32,
    max_concurrency: i32,
    max_invocations: i32,
) -> Result<(std::net::SocketAddr, impl HostApi, String)> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    let workload_id = uuid::Uuid::new_v4().to_string();
    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
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
                max_invocations,
                max_concurrency,
                ..Default::default()
            }],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    })
    .await
    .context("sleeper workload should start")?;
    Ok((addr, host, workload_id))
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

/// One request; returns the body, or the error the host produced.
async fn get(addr: std::net::SocketAddr, host_header: &str, path: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(20))
        .build()?;
    let resp = client
        .get(format!("http://{addr}{path}"))
        .header("HOST", host_header)
        .send()
        .await?;
    anyhow::ensure!(resp.status().is_success(), "status {}", resp.status());
    Ok(resp.text().await?)
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

/// A trap poisons the store it runs in, so it takes down the calls sharing that
/// instance. What must *not* happen is the pool going with it: the faulted
/// instance is reaped and the next call is served by a fresh one.
///
/// This is the cost of sharing an instance, and the reason `max_concurrency`
/// bounds how much of it a single trap can reach.
#[tokio::test]
async fn a_trap_retires_its_instance_and_the_pool_recovers() -> Result<()> {
    let (addr, _host) = start_sleeper("conc-trap", 2, 4).await?;

    // Warm an instance and confirm it serves.
    get(addr, "conc-trap", "/").await?;

    // Poison one. The request itself must fail rather than hang.
    let trapped = get(addr, "conc-trap", "/trap").await;
    assert!(
        trapped.is_err(),
        "a trapping request must fail, not succeed"
    );

    // The pool keeps serving: the faulted instance is reaped and the next call
    // lands on a live or fresh one.
    for i in 0..4 {
        get(addr, "conc-trap", "/")
            .await
            .with_context(|| format!("request {i} after the trap should still be served"))?;
    }
    Ok(())
}

/// A client that goes away mid-request must not disturb the calls sharing its
/// instance. Before instances were shared this was free -- the request owned
/// its store and dropping it hurt nobody. Now it has to be checked.
#[tokio::test]
async fn a_disconnected_client_leaves_its_siblings_alone() -> Result<()> {
    let (addr, _host) = start_sleeper("conc-drop", 1, 8).await?;

    // Warm the single instance so both requests below land on it.
    get(addr, "conc-drop", "/").await?;

    // A request we abandon while it is in flight.
    let abandoned = tokio::spawn(async move {
        let _ = get(addr, "conc-drop", "/").await;
    });
    // ...and a sibling on the same instance.
    let sibling = tokio::spawn(async move { get(addr, "conc-drop", "/").await });

    tokio::time::sleep(Duration::from_millis(2)).await;
    abandoned.abort();
    let _ = abandoned.await;

    sibling
        .await
        .context("sibling task panicked")?
        .context("a sibling call must survive a client disconnecting")?;

    // And the instance is still serving afterwards.
    get(addr, "conc-drop", "/")
        .await
        .context("the instance must keep serving after a client disconnect")?;
    Ok(())
}

/// `max_invocations` and `max_concurrency` compose: an instance admits up to
/// its invocation budget, drains what it took, and is replaced. The replacement
/// has paid no setup yet, so it reports a peak of its own rather than
/// continuing the retired instance's count.
#[tokio::test]
async fn max_invocations_retires_a_concurrent_instance() -> Result<()> {
    let (addr, _host, _id) = start_sleeper_with("conc-retire", 1, 4, 4).await?;

    // Four calls at once: one instance takes all of them and is then retired.
    let first = burst(addr, "conc-retire", 4).await?;
    assert!(
        first > 1,
        "the instance should have overlapped its four calls, saw peak {first}"
    );

    // It has spent its budget, so this burst is served by a replacement.
    for i in 0..4 {
        get(addr, "conc-retire", "/")
            .await
            .with_context(|| format!("request {i} after retirement should be served"))?;
    }
    Ok(())
}

/// Stopping a workload has to take its warm instances with it. Each one owns a
/// store and a task that outlive any single call, so nothing else would.
#[tokio::test]
async fn stopping_a_workload_shuts_down_its_warm_instances() -> Result<()> {
    let (addr, host, workload_id) = start_sleeper_with("conc-stop", 2, 4, 0).await?;
    burst(addr, "conc-stop", 4).await?;

    // Stopping must complete rather than hang on the live drivers...
    tokio::time::timeout(
        Duration::from_secs(30),
        host.workload_stop(WorkloadStopRequest { workload_id }),
    )
    .await
    .context("workload stop should not hang on warm instances")?
    .context("workload stop should succeed")?;

    // ...and the instances must be gone with it, not still answering.
    assert!(
        get(addr, "conc-stop", "/").await.is_err(),
        "a stopped workload's warm instances must not keep serving"
    );
    Ok(())
}

/// A component reached over a *linked call* rather than HTTP shares the same
/// warm instances, so `max_concurrency` reaches it too. Without that, the knob
/// would be silently inert for a component another component calls — which is
/// how the template reaches its backends.
///
/// The callee counts calls in its own linear memory, so a climbing count is
/// one instance serving them all.
#[tokio::test]
async fn linked_calls_share_the_warm_instances() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "linked-conc".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 2,
                    max_invocations: 0,
                    max_concurrency: 4,
                    ..Default::default()
                },
                Component {
                    name: "callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 0,
                    max_concurrency: 4,
                    ..Default::default()
                },
            ],
            host_interfaces: http_only_host_interfaces("linked-conc"),
            volumes: vec![],
        },
    })
    .await
    .context("linked concurrency workload should start")?;

    // `/calls` returns the callee instance's own call count.
    let mut counts = Vec::new();
    for _ in 0..5 {
        counts.push(
            get(addr, "linked-conc", "/calls")
                .await?
                .trim()
                .parse::<u64>()?,
        );
    }

    assert_eq!(
        counts,
        vec![1, 2, 3, 4, 5],
        "linked calls must be served by the one warm callee instance, not a store each"
    );
    Ok(())
}
