//! Integration tests for the plugin → workload direction: a host component
//! plugin that *imports* `acme:events/handler`, which no host built-in
//! provides, so the host routes each call to the export of a bound workload.
//!
//! The `events-plugin` component also exports `acme:events/control`, so both
//! ways of choosing a target are drivable from an ordinary request:
//!   - a call with no target handle held goes back to the calling workload;
//!   - a call under a `wasmcloud:host/workload` target handle goes to the
//!     workload that handle names, for as long as it is alive;
//!   - `callable` reports the workloads that are up and export the interface.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest,
};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_incoming_handler_interface, req, start_host_with_component_plugin_by_host};

const EVENTS_PLUGIN_WASM: &[u8] = include_bytes!("wasm/events_plugin.wasm");
const EVENTS_CALLER_WASM: &[u8] = include_bytes!("wasm/events_caller.wasm");
const PLUGIN_ID: &str = "acme-events-plugin";

/// The `acme:events` binding: `control` the plugin serves, `handler` the
/// workload serves. One manifest entry covers both directions, since interface
/// matching already looks at a workload's exports as well as its imports.
fn acme_events_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "events".to_string(),
        interfaces: ["control".to_string(), "handler".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// An `events-caller` workload with a caller-chosen id (so a test can name it
/// as a dispatch target) and a `tag` it stamps on every callback reply (so a
/// test can tell which workload actually handled one).
fn events_workload(workload_id: &str, host: &str, tag: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "events-caller".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(EVENTS_CALLER_WASM),
                local_resources: LocalResources {
                    environment: HashMap::from([("EVENT_TAG".to_string(), tag.to_string())]),
                    ..LocalResources::default()
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 4,
            }],
            host_interfaces: vec![
                http_incoming_handler_interface(host, None),
                acme_events_interface(),
            ],
            volumes: vec![],
        },
    }
}

/// A plugin call that names no target goes back to the workload whose
/// capability call it is serving — the callback shape, where the plugin never
/// has to address anyone.
#[tokio::test]
async fn test_plugin_calls_back_into_its_caller() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/echo?msg=hi").await?;
    assert_eq!(status.as_u16(), 200, "/echo should succeed");
    assert_eq!(
        body, "alpha:echo:hi",
        "the plugin's unaddressed call should reach the calling workload's own handler export"
    );

    Ok(())
}

/// A plugin call under a target handle goes to the workload that handle names,
/// not to the one that called in — the trigger shape, where the plugin picks.
#[tokio::test]
async fn test_plugin_dispatches_to_a_named_workload() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/dispatch?id=wl-beta&msg=hi").await?;
    assert_eq!(status.as_u16(), 200, "/dispatch should succeed");
    assert_eq!(
        body, "beta:dispatch:hi",
        "a named target should override the calling workload, reaching beta from alpha's request"
    );

    // ...and the reverse, so the result cannot be an artifact of which workload
    // happened to deploy last.
    let (status, body) = req(&client, &addr, "beta", "/dispatch?id=wl-alpha&msg=yo").await?;
    assert_eq!(status.as_u16(), 200, "/dispatch should succeed");
    assert_eq!(body, "alpha:dispatch:yo", "dispatch should route both ways");

    Ok(())
}

/// A target's scope is its handle's lifetime: a call inside the handle goes to
/// the workload it names, and the next call — after it is dropped — falls back
/// to the caller.
#[tokio::test]
async fn test_target_scope_ends_with_the_handle() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/nested?id=wl-beta&msg=hi").await?;
    assert_eq!(status.as_u16(), 200, "/nested should succeed");
    assert_eq!(
        body, "beta:inner:hi|alpha:outer:hi",
        "the call inside the handle should reach beta and the one after it should fall back to \
         the calling workload"
    );

    Ok(())
}

/// `callable` reports the workloads that are running and export an interface
/// the plugin imports — appearing when they deploy and going when they stop.
#[tokio::test]
async fn test_callable_tracks_running_workloads() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(status.as_u16(), 200, "/callable should succeed");
    assert_eq!(
        body, "wl-alpha",
        "only the deployed workload should be callable"
    );

    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let (_status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(
        body, "wl-alpha\nwl-beta",
        "a second workload should become callable once it is running, sorted by id"
    );

    h.workload_stop(WorkloadStopRequest {
        workload_id: "wl-beta".to_string(),
    })
    .await?;
    let (_status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(
        body, "wl-alpha",
        "a stopped workload should no longer be callable"
    );

    Ok(())
}
