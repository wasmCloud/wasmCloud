//! What the per-call HTTP timeout does about a wedged guest, on both shapes of
//! shared instance.
//!
//! `WASH_HTTP_RESPONSE_TIMEOUT_SECS` bounds the whole exchange for any HTTP
//! call served on a shared instance — the workload's service and a pooled
//! component's warm instances alike, since both run the same task. What
//! happens *after* it fires differs, because the wedged guest work cannot be
//! cancelled from the host:
//!
//!  * A **pooled instance** is retired: it stops admitting, drains, is reaped,
//!    and its store's teardown is what finally ends the stalled work. The next
//!    call gets a fresh instance.
//!  * A **service** has no such remedy — its singleton instance keeps serving,
//!    stalled task and all. The timeout only bounds how long the client waits.
//!
//! The `http-sleeper` fixture's `/wedge` parks for an hour before producing
//! the response head, and every reply carries `served`, the instance's own
//! request count. That counter is what tells a retired instance from a merely
//! recovered slot: a replacement starts over at one, the survivor counts on.

// `std::env::set_var` is unsafe on edition 2024. The override below runs once,
// before any host is started and before anything else in this process reads
// the environment, which is the soundness condition it needs.
#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Once;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");

/// Shrink the exchange timeout for this whole test binary. The value is
/// cached process-wide on first read (`LazyLock`), so every test must want
/// the same one — and must call this before starting a host.
fn short_http_timeout() {
    static SET: Once = Once::new();
    SET.call_once(|| unsafe {
        std::env::set_var("WASH_HTTP_RESPONSE_TIMEOUT_SECS", "1");
    });
}

/// A `/wedge` request must come back as a failure — error or 5xx, depending on
/// where the drop surfaces — well inside the fixture's one-hour park, and in
/// the neighbourhood of the 1s timeout.
async fn assert_wedge_bounded(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<()> {
    let started = Instant::now();
    let outcome = client
        .get(format!("http://{addr}/wedge"))
        .header("HOST", host_header)
        .send()
        .await;
    let elapsed = started.elapsed();
    anyhow::ensure!(
        elapsed < Duration::from_secs(10),
        "the wedged call must be bounded by the exchange timeout, took {elapsed:?}"
    );
    // The dropped response surfaces as a 5xx or as a closed connection,
    // depending on where the drop lands; either is a bounded failure.
    if let Ok(resp) = outcome {
        anyhow::ensure!(
            resp.status().is_server_error(),
            "a wedged call must fail, got {}",
            resp.status()
        );
    }
    Ok(())
}

/// GET `/` and return the instance's `served` count.
async fn served(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<u64> {
    let resp = client
        .get(format!("http://{addr}/"))
        .header("HOST", host_header)
        .send()
        .await?;
    anyhow::ensure!(resp.status().is_success(), "status {}", resp.status());
    Ok(json_u64_field(&resp.text().await?, "served"))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(15))
        .build()?)
}

/// A pooled instance wedged past the timeout is retired and replaced: the
/// request after the wedge is served by a *fresh* instance, whose own count
/// restarts at one. A surviving instance would have answered `served: 2`.
#[tokio::test]
async fn a_wedged_pooled_instance_is_retired_and_replaced() -> Result<()> {
    short_http_timeout();
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "wedge-pooled".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 0,
                max_concurrency: 4,
            }],
            host_interfaces: http_only_host_interfaces("wedge-pooled"),
            volumes: vec![],
        },
    })
    .await
    .context("pooled sleeper workload should start")?;
    let client = client()?;

    // Warm the one instance; its own count reads 1.
    assert_eq!(served(&client, addr, "wedge-pooled").await?, 1);

    assert_wedge_bounded(&client, addr, "wedge-pooled").await?;

    // The wedged instance had served 2 requests; had it survived, this would
    // read 3. A fresh instance reading 1 is the retirement, observed.
    assert_eq!(
        served(&client, addr, "wedge-pooled").await?,
        1,
        "the request after the wedge must land on a fresh instance"
    );
    Ok(())
}

/// A service wedged the same way is *not* replaced — there is exactly one
/// service instance, and retiring it is not the pool's call to make. The
/// timeout bounds the client's wait, and the same instance keeps serving
/// around the stalled task.
#[tokio::test]
async fn a_wedged_service_keeps_serving_with_a_bounded_wait() -> Result<()> {
    short_http_timeout();
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "wedge-svc".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
                ports: Vec::new(),
            }),
            components: vec![],
            host_interfaces: http_only_host_interfaces("wedge-svc"),
            volumes: vec![],
        },
    })
    .await
    .context("sleeper service workload should start")?;
    let client = client()?;

    assert_eq!(served(&client, addr, "wedge-svc").await?, 1);

    assert_wedge_bounded(&client, addr, "wedge-svc").await?;

    // Same instance, still serving: the wedge was its second request, so the
    // next reads 3. A restarted or replaced instance would read 1.
    assert_eq!(
        served(&client, addr, "wedge-svc").await?,
        3,
        "the service instance must keep serving around the stalled task"
    );
    Ok(())
}
