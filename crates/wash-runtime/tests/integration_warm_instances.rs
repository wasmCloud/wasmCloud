//! Integration tests for warm-instance pooling (`pool_size` /
//! `max_invocations` on a [`Component`]), across both paths that build a store
//! per call by default.
//!
//! Every case reads a counter out of the guest's linear memory, so the
//! response says directly whether an instance was reused or freshly built:
//! `1` every time means a fresh instance per call, a climbing count means one
//! instance served them all.
//!
//!  * **Linked calls** — `ephemeral-caller-p3` calls the plain-value async
//!    `calls()` export of the linked `ephemeral-callee-p3`.
//!  * **HTTP dispatch** — `svc-no-run` deployed as a plain component (not a
//!    service), reporting its own per-instance request count.
//!  * **Linked components share the store's lifetime**, so a component is kept
//!    warm only when everything instantiated alongside it has opted in too.
//!    Otherwise `pool_size: 0` on a callee would stop meaning "my state is
//!    ephemeral" the moment something else imported it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");
/// A p3 HTTP handler reporting a per-instance request count.
const SVC_NO_RUN_WASM: &[u8] = include_bytes!("wasm/svc_no_run.wasm");

/// Start a host running the caller/callee pair, with the callee configured to
/// the given warm-instance limits. The caller is never pooled, so only the
/// callee's behavior is under test.
async fn start_pair(
    host_header: &'static str,
    callee_pool_size: i32,
    callee_max_invocations: i32,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    start_pair_with_caller_pool(host_header, 0, callee_pool_size, callee_max_invocations).await
}

/// As [`start_pair`], but with the caller's `pool_size` set too — the caller's
/// store is what the callee is instantiated into, so the two interact.
async fn start_pair_with_caller_pool(
    host_header: &'static str,
    caller_pool_size: i32,
    callee_pool_size: i32,
    callee_max_invocations: i32,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "ephemeral-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: caller_pool_size,
                    max_invocations: 0,
                    max_concurrency: 1,
                },
                Component {
                    name: "ephemeral-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: callee_pool_size,
                    max_invocations: callee_max_invocations,
                    max_concurrency: 1,
                },
            ],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    })
    .await
    .context("warm-instance workload should start")?;

    Ok((addr, host))
}

/// Start a host running `svc-no-run` as a plain **component** (not a service),
/// so requests take the HTTP dispatch path: a store and an instance per
/// request unless `pool_size` says otherwise. Its body reports how many
/// requests its own instance has served.
async fn start_http_component(
    host_header: &'static str,
    pool_size: i32,
    max_invocations: i32,
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
                name: "http-counter".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(SVC_NO_RUN_WASM),
                local_resources: LocalResources::default(),
                pool_size,
                max_invocations,
                max_concurrency,
            }],
            host_interfaces: http_only_host_interfaces(host_header),
            volumes: vec![],
        },
    })
    .await
    .context("HTTP component workload should start")?;

    Ok((addr, host))
}

/// Issue `n` sequential `GET /` requests, returning the `http_calls` count the
/// instance reports for each.
async fn http_call_counts(
    addr: std::net::SocketAddr,
    host_header: &str,
    n: usize,
) -> Result<Vec<u64>> {
    // No connection reuse: a retried GET on a stale pooled connection would
    // land twice and break the exactly-once counts.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;
    let mut counts = Vec::with_capacity(n);
    for i in 0..n {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", host_header)
                .send(),
        )
        .await
        .with_context(|| format!("request {i} timed out"))?
        .with_context(|| format!("request {i} failed"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "request {i} returned {}",
            response.status()
        );
        counts.push(common::json_u64_field(
            &response.text().await?,
            "http_calls",
        ));
    }
    Ok(counts)
}

/// Issue `n` sequential `GET /calls` requests, returning each body parsed as
/// the callee instance's call count.
async fn call_counts(addr: std::net::SocketAddr, host_header: &str, n: usize) -> Result<Vec<u32>> {
    let client = reqwest::Client::new();
    let mut counts = Vec::with_capacity(n);
    for i in 0..n {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/calls"))
                .header("HOST", host_header)
                .send(),
        )
        .await
        .with_context(|| format!("request {i} timed out"))?
        .with_context(|| format!("request {i} failed"))?;
        anyhow::ensure!(
            response.status().is_success(),
            "request {i} returned {}",
            response.status()
        );
        let body = response.text().await?;
        counts.push(
            body.trim()
                .parse::<u32>()
                .with_context(|| format!("request {i} returned non-numeric body {body:?}"))?,
        );
    }
    Ok(counts)
}

/// Without pooling every linked call gets its own instance, so the callee's
/// in-memory counter is back at zero each time. This is the default and pins
/// the ephemeral contract: component state does not survive a call.
#[tokio::test]
async fn unpooled_component_gets_a_fresh_instance_per_call() -> Result<()> {
    let (addr, _host) = start_pair("warm-off", 0, 0).await?;

    let counts = call_counts(addr, "warm-off", 5).await?;

    assert_eq!(
        counts,
        vec![1, 1, 1, 1, 1],
        "each call to an unpooled component must run in a fresh instance"
    );
    Ok(())
}

/// With `pool_size: 1` the instance is parked after each call and picked up by
/// the next, so guest state accumulates — the reason a component would opt in.
#[tokio::test]
async fn pooled_component_reuses_a_warm_instance() -> Result<()> {
    let (addr, _host) = start_pair("warm-on", 1, 0).await?;

    let counts = call_counts(addr, "warm-on", 5).await?;

    assert_eq!(
        counts,
        vec![1, 2, 3, 4, 5],
        "a pooled component must keep serving from the same warm instance"
    );
    Ok(())
}

/// `max_invocations` bounds how long any one instance lives: it serves that
/// many calls and is then retired, so the next call starts from a cold store
/// and the counter restarts.
#[tokio::test]
async fn max_invocations_retires_a_warm_instance() -> Result<()> {
    let (addr, _host) = start_pair("warm-retire", 1, 3).await?;

    let counts = call_counts(addr, "warm-retire", 7).await?;

    assert_eq!(
        counts,
        vec![1, 2, 3, 1, 2, 3, 1],
        "an instance must be retired once it has served max_invocations calls"
    );
    Ok(())
}

/// A component reached over **HTTP** — no service, no linked call — is also
/// kept warm when it opts in. The runtime builds a store and instantiates per
/// request by default, which is what `pool_size` is meant to avoid.
#[tokio::test]
async fn pooled_component_serving_http_reuses_a_warm_instance() -> Result<()> {
    let (addr, _host) = start_http_component("warm-http", 1, 0, 1).await?;

    let counts = http_call_counts(addr, "warm-http", 5).await?;

    assert_eq!(
        counts,
        vec![1, 2, 3, 4, 5],
        "an HTTP component that opted in must be served from one warm instance"
    );
    Ok(())
}

/// The default for an HTTP component is unchanged: a store and an instance per
/// request, so nothing carries over.
#[tokio::test]
async fn unpooled_component_serving_http_is_fresh_per_request() -> Result<()> {
    let (addr, _host) = start_http_component("cold-http", 0, 0, 1).await?;

    let counts = http_call_counts(addr, "cold-http", 4).await?;

    assert_eq!(
        counts,
        vec![1, 1, 1, 1],
        "an HTTP component that did not opt in must get a fresh instance per request"
    );
    Ok(())
}

/// A warm store is reused indefinitely by default, so anything a request
/// leaves behind in it — resource-table entries for the request, the response,
/// their bodies — accumulates instead of being reclaimed with the store. This
/// drives a few thousand requests through a single warm instance and checks it
/// still serves correctly at the end, which a store leaking a table entry per
/// request would not.
#[tokio::test]
async fn a_warm_instance_survives_many_requests() -> Result<()> {
    const REQUESTS: usize = 3000;
    let (addr, _host) = start_http_component("warm-soak", 1, 0, 1).await?;

    let counts = http_call_counts(addr, "warm-soak", REQUESTS).await?;

    // One instance served every request, and its count never restarted — so
    // nothing forced a retirement partway through.
    assert_eq!(
        counts.first().copied(),
        Some(1),
        "the first request should land on a fresh instance"
    );
    assert_eq!(
        counts.last().copied(),
        Some(REQUESTS as u64),
        "one warm instance must serve all {REQUESTS} requests without being retired"
    );
    Ok(())
}

/// `max_invocations` applies to the HTTP path too.
#[tokio::test]
async fn max_invocations_retires_an_http_instance() -> Result<()> {
    let (addr, _host) = start_http_component("warm-http-retire", 1, 2, 1).await?;

    let counts = http_call_counts(addr, "warm-http-retire", 6).await?;

    assert_eq!(
        counts,
        vec![1, 2, 1, 2, 1, 2],
        "an HTTP instance must be retired once it has served max_invocations requests"
    );
    Ok(())
}

/// A store holds the component's linked components too, and they live exactly
/// as long as it does. So a component is kept warm only when everything
/// instantiated alongside it has also opted in — otherwise `pool_size: 0` on
/// the callee would stop meaning "my state is ephemeral" as soon as something
/// else imported it.
///
/// Here the caller opts in and the callee does not, so neither is kept warm:
/// the callee reports `1` every time.
#[tokio::test]
async fn a_linked_component_that_did_not_opt_in_keeps_the_caller_cold() -> Result<()> {
    let (addr, _host) = start_pair_with_caller_pool("warm-mixed", 4, 0, 0).await?;

    let counts = call_counts(addr, "warm-mixed", 4).await?;

    assert_eq!(
        counts,
        vec![1, 1, 1, 1],
        "a warm caller must not silently give warm state to a callee that did not opt in"
    );
    Ok(())
}

/// The same shape with both opted in does keep them warm, so the rule above is
/// about the callee's own setting and not an accident of the caller's.
#[tokio::test]
async fn both_opted_in_keeps_the_pair_warm() -> Result<()> {
    let (addr, _host) = start_pair_with_caller_pool("warm-both", 4, 1, 0).await?;

    let counts = call_counts(addr, "warm-both", 4).await?;

    assert_eq!(
        counts,
        vec![1, 2, 3, 4],
        "with both components opted in the callee instance must be reused"
    );
    Ok(())
}
