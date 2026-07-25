//! Integration tests for warm-instance pooling on the ephemeral linked-call
//! path (`pool_size` / `max_invocations` on a [`Component`]).
//!
//! All three cases drive the same pair of fixtures: `ephemeral-caller-p3`
//! serves HTTP and calls the plain-value async `calls()` export of the linked
//! `ephemeral-callee-p3`, which returns how many calls **its own instance**
//! has served. Because that count lives in the callee's linear memory, the
//! response says directly whether the call reused an instance or got a fresh
//! one:
//!
//!  * `pool_size: 0` — every call builds and drops its own store, so the
//!    count is `1` every time. This is the default and the behavior before
//!    warm instances existed.
//!  * `pool_size: 1` — the instance is parked between calls and reused, so
//!    the count climbs.
//!  * `pool_size: 1, max_invocations: N` — the instance is retired once it has
//!    served `N` calls, so the count climbs to `N` and then restarts.

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

/// Start a host running the caller/callee pair, with the callee configured to
/// the given warm-instance limits. The caller is never pooled, so only the
/// callee's behavior is under test.
async fn start_pair(
    host_header: &'static str,
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
                    pool_size: 0,
                    max_invocations: 0,
                },
                Component {
                    name: "ephemeral-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: callee_pool_size,
                    max_invocations: callee_max_invocations,
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

/// Issue `n` sequential `GET /calls` requests, returning each body parsed as
/// the callee instance's call count.
async fn call_counts(
    addr: std::net::SocketAddr,
    host_header: &str,
    n: usize,
) -> Result<Vec<u32>> {
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
