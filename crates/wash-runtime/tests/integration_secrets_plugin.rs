//! Integration test: `wasmcloud:secrets` served by the `secrets-host` host
//! component plugin, with credentials delivered per-workload via workload
//! lifecycle.
//!
//! Proves two things end to end, driven over HTTP through the `secrets-caller`
//! workload:
//!   - a capability (`store.get`/`reveal`) crosses the host component plugin
//!     store boundary — the plugin serves it from its own supervised store;
//!   - the interface config a workload declares (its credentials) reaches the
//!     plugin via `on-workload-bind` and is served back, correlated to the
//!     calling workload by the host identity import.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, WorkloadStopRequest};

mod common;
use common::{
    component_workload_request, secrets_caller_host_interfaces_with_config,
    start_host_with_component_plugin, start_host_with_component_plugin_by_host,
};

const SECRETS_HOST_WASM: &[u8] = include_bytes!("wasm/secrets_host.wasm");
const SECRETS_CALLER_WASM: &[u8] = include_bytes!("wasm/secrets_caller.wasm");
const PLUGIN_ID: &str = "wasmcloud-secrets";

/// GET `http://{addr}{path}` with the `HOST` header selecting the workload,
/// returning the status and body text.
async fn req(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    host: &str,
    path: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let resp = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", host)
            .send(),
    )
    .await
    .context("request timed out")??;
    let status = resp.status();
    let body = resp.text().await?;
    Ok((status, body))
}

/// A `secrets-caller` workload addressed by `host`, with `config` set on its
/// `wasmcloud:secrets` interface entry (the credentials `on-workload-bind`
/// delivers to the plugin).
fn caller_workload(
    host: &str,
    config: &[(&str, &str)],
) -> wash_runtime::types::WorkloadStartRequest {
    let config = config
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    component_workload_request(
        "secrets-caller",
        host,
        SECRETS_CALLER_WASM,
        LocalResources::default(),
        secrets_caller_host_interfaces_with_config(host, config),
    )
}

/// A workload's `wasmcloud:secrets` import resolves to the host component
/// plugin; the credentials it declared at deploy time are served back through
/// `store.get` + `reveal`, and an unknown key reads as `none` (404).
#[tokio::test]
async fn test_secret_delivered_and_revealed() -> Result<()> {
    let host = "secrets-basic";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, SECRETS_HOST_WASM).await?;
    h.workload_start(caller_workload(
        host,
        &[
            ("registry-username", "alice"),
            ("registry-password", "s3cr3t"),
        ],
    ))
    .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, host, "/get?key=registry-username").await?;
    assert_eq!(status.as_u16(), 200, "a bound secret must resolve");
    assert_eq!(body, "alice", "the revealed value must round-trip");

    let (status, body) = req(&client, &addr, host, "/get?key=registry-password").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "s3cr3t");

    let (status, _) = req(&client, &addr, host, "/get?key=absent").await?;
    assert_eq!(status.as_u16(), 404, "an unbound key reads as none");

    Ok(())
}

/// Secrets are partitioned per workload: two workloads that declare the same key
/// with different values each resolve to their own value, proving the plugin
/// keys bind-time config by the calling workload's identity rather than sharing
/// one global map.
///
/// Multi-threaded: the by-host router uses `block_in_place`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_secrets_isolated_per_workload() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, SECRETS_HOST_WASM)
            .await?;
    h.workload_start(caller_workload(
        "secrets-a",
        &[("registry-username", "alice")],
    ))
    .await?;
    h.workload_start(caller_workload(
        "secrets-b",
        &[("registry-username", "bob")],
    ))
    .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "secrets-a", "/get?key=registry-username").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "alice");

    let (status, body) = req(&client, &addr, "secrets-b", "/get?key=registry-username").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "bob", "each workload must see only its own secret");

    Ok(())
}

/// Stopping one workload delivers `on-workload-unbind` for it without disturbing
/// the others: a surviving workload keeps resolving its own secret, and the
/// stopped workload is no longer routable.
///
/// Multi-threaded: the by-host router uses `block_in_place`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_secret_survives_other_workload_stop() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, SECRETS_HOST_WASM)
            .await?;
    let stopped = caller_workload("secrets-stop", &[("registry-username", "carol")]);
    let stopped_id = stopped.workload_id.clone();
    h.workload_start(stopped).await?;
    h.workload_start(caller_workload(
        "secrets-keep",
        &[("registry-username", "dave")],
    ))
    .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "secrets-keep", "/get?key=registry-username").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "dave");

    h.workload_stop(WorkloadStopRequest {
        workload_id: stopped_id,
    })
    .await?;

    // The survivor is untouched by the other workload's unbind.
    let (status, body) = req(&client, &addr, "secrets-keep", "/get?key=registry-username").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(
        body, "dave",
        "stopping another workload must not disturb this one"
    );

    // The stopped workload is no longer routable (poll: teardown is async).
    let mut stopped_status = 0;
    for _ in 0..40 {
        stopped_status = req(&client, &addr, "secrets-stop", "/get?key=registry-username")
            .await?
            .0
            .as_u16();
        if stopped_status == 404 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        stopped_status, 404,
        "a stopped workload is no longer routable"
    );

    Ok(())
}
