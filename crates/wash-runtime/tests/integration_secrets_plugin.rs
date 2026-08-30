//! Integration test: `wasmcloud:secrets` served by the native
//! `wasmcloud-secrets` plugin.
//!
//! Proves the interface config a workload declares (its credentials) reaches
//! the plugin via `on-workload-bind` and is served back through
//! `store.get`/`reveal`, correlated to the calling workload.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, WorkloadState, WorkloadStopRequest};

mod common;
use common::{
    component_workload_request, http_incoming_handler_interface, req,
    secrets_caller_host_interfaces_with_config, secrets_interface, start_host_with_dev_router,
    start_host_with_dynamic_router,
};

const SECRETS_CALLER_WASM: &[u8] = include_bytes!("wasm/secrets_caller.wasm");

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

/// A workload's `wasmcloud:secrets` import resolves to the native plugin; the
/// credentials it declared at deploy time are served back through
/// `store.get` + `reveal`, and an unknown key reads as `none` (404).
#[tokio::test]
async fn test_secret_delivered_and_revealed() -> Result<()> {
    let host = "secrets-basic";
    let (addr, h) = start_host_with_dev_router("127.0.0.1:0").await?;
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
    let (addr, h) = start_host_with_dynamic_router("127.0.0.1:0").await?;
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
    let (addr, h) = start_host_with_dynamic_router("127.0.0.1:0").await?;
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

/// A workload that declares the same secret key across two separate
/// Two entries of one binding that *agree* are one binding, and deploy.
///
/// Resolution stamps the folded config onto every entry of a binding, so a
/// plugin that summed its matched interfaces would count each key once per
/// entry and read the repetition as a collision — refusing a workload whose
/// entries never disagreed about anything.
#[tokio::test]
async fn test_split_secret_config_across_entries_binds() -> Result<()> {
    let host = "secrets-split";
    let (_addr, h) = start_host_with_dev_router("127.0.0.1:0").await?;

    let request = component_workload_request(
        "secrets-caller",
        host,
        SECRETS_CALLER_WASM,
        LocalResources::default(),
        vec![
            http_incoming_handler_interface(host, None),
            secrets_interface(
                [("registry-username".to_string(), "alice".to_string())]
                    .into_iter()
                    .collect(),
            ),
            secrets_interface(
                [("registry-password".to_string(), "s3cr3t".to_string())]
                    .into_iter()
                    .collect(),
            ),
        ],
    );

    let response = h.workload_start(request).await?;
    assert_ne!(
        response.workload_status.workload_state,
        WorkloadState::Error,
        "disjoint keys across two entries of one binding must deploy, got: {:?}",
        response.workload_status
    );
    Ok(())
}

/// `wasmcloud:secrets` bindings is rejected at bind time: `store.get` takes
/// only a key, with no way to say which binding it's asking on behalf of, so
/// one binding's value must never silently shadow the other's.
#[tokio::test]
async fn test_colliding_secret_key_rejects_bind() -> Result<()> {
    let host = "secrets-collide";
    let (_addr, h) = start_host_with_dev_router("127.0.0.1:0").await?;

    let config_a = [("registry-username".to_string(), "alice".to_string())]
        .into_iter()
        .collect();
    let config_b = [("registry-username".to_string(), "mallory".to_string())]
        .into_iter()
        .collect();

    let request = component_workload_request(
        "secrets-caller",
        host,
        SECRETS_CALLER_WASM,
        LocalResources::default(),
        vec![
            http_incoming_handler_interface(host, None),
            secrets_interface(config_a),
            secrets_interface(config_b),
        ],
    );

    // A rejected bind doesn't surface as `Err` from `workload_start` — the
    // call succeeds, but the returned status reports the deploy failure.
    let response = h.workload_start(request).await?;
    assert_eq!(
        response.workload_status.workload_state,
        WorkloadState::Error,
        "colliding secret keys across bindings must fail to deploy, got: {:?}",
        response.workload_status
    );
    // Caught by binding resolution rather than by the plugin: the entries of
    // one binding are folded into a single config before any plugin sees them,
    // so two that disagree are refused there, with the key named.
    let message = &response.workload_status.message;
    assert!(
        message.contains("conflicting values for `registry-username`"),
        "expected the refusal to name the colliding key, got: {:?}",
        response.workload_status
    );
    assert!(
        !message.contains("alice") && !message.contains("mallory"),
        "the message must not carry the values; they are secrets: {:?}",
        response.workload_status
    );

    Ok(())
}
