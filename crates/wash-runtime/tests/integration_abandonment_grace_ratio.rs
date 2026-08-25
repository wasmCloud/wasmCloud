//! Abandonment when the ingress deadline is much longer than the grace, which
//! is the shipping configuration.
//!
//! `integration_abandoned_calls` sets every ingress deadline to 2s against a 1s
//! grace so its traps land quickly. The defaults are the other way round by a
//! factor of sixty: 600s deadlines against a 10s grace. Deadlines are cached
//! process-wide on first read, so a second ratio needs a second binary.
//!
//! The gap matters because dropping a dispatcher arms the flag, so an ordinary
//! client disconnect abandons a call with most of its ingress deadline left to
//! run. For all of that window the epoch callback is free to trap the store as
//! soon as its conditions line up, which for a service with a `wasi:cli/run`
//! tick loop takes only a grace's worth of fires. `watch_until_abandoned` is
//! what deregisters the call, and this covers that it does so regardless of how
//! long the ingress deadline is.

#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_dynamic_router};

const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");

/// The shipping shape, scaled down: an in-store bound thirty times the grace.
const RESPONSE_TIMEOUT_SECS: u64 = 30;
const GRACE_SECS: u64 = 1;

/// A client disconnect must not cost a healthy guest its service, however much
/// of the ingress deadline is left to run.
///
/// The guest hops 200 x 50ms, pure awaiting with no CPU at all, and its client
/// walks away 300ms in, arming the flag on the spot. Every trap condition then
/// lines up within a couple of seconds: it is the only call on the store, the
/// grace passes, and the hops wake often enough that the epoch callback fires
/// every sampling window and reads as continuous execution. Deregistration is
/// the only thing that saves it, and the guest yields, so the grace timer
/// sharing its task gets to run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disconnect_does_not_trap_a_healthy_guest_when_the_deadline_outlives_the_grace()
-> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    // Runs before any host starts and before anything reads the environment,
    // and this binary holds exactly one test.
    unsafe {
        std::env::set_var(
            "WASH_HTTP_RESPONSE_TIMEOUT_SECS",
            RESPONSE_TIMEOUT_SECS.to_string(),
        );
        std::env::set_var("WASH_ABANDONED_CALL_GRACE_SECS", GRACE_SECS.to_string());
    }

    let (addr, host) = start_host_with_dynamic_router("127.0.0.1:0").await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "healthy".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_SLEEPER_WASM),
                local_resources: LocalResources::default(),
                // A restart is the failure this test looks for; allow it so it
                // reads as a count restarting at one rather than a dead host.
                max_restarts: 2,
            }),
            components: vec![],
            host_interfaces: http_only_host_interfaces("healthy"),
            volumes: vec![],
        },
    })
    .await
    .context("sleeper service should start")?;

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(30))
        .build()?;
    let served = async |c: &reqwest::Client| -> Result<u64> {
        let resp = c
            .get(format!("http://{addr}/"))
            .header("HOST", "healthy")
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success(), "status {}", resp.status());
        Ok(json_u64_field(&resp.text().await?, "served"))
    };

    let before = served(&client).await?;

    let mut gone = Box::pin(
        client
            .get(format!("http://{addr}/chatter?hops=200&hop_ms=50"))
            .header("HOST", "healthy")
            .send(),
    );
    tokio::select! {
        _ = &mut gone => anyhow::bail!("the chatter answered before the client gave up"),
        () = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    drop(gone);

    // Several times the grace and the credit the callback needs, while barely a
    // third of the way into the 30s ingress deadline.
    tokio::time::sleep(Duration::from_secs(8)).await;

    let after = served(&client).await?;
    anyhow::ensure!(
        after > before,
        "a disconnect trapped a healthy guest's service: served {before} -> {after}"
    );
    Ok(())
}
