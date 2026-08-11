//! Integration tests for the plugin → workload direction: a host component
//! plugin that *imports* `acme:events/handler`, which no host built-in
//! provides, so the host routes each call to the export of a bound workload.
//!
//! The `events-plugin` component also exports `acme:events/control`, so both
//! ways of choosing a target are drivable from an ordinary request:
//!   - a call with no target handle held goes back to the calling workload;
//!   - a call under a `wasmcloud:host/workload-call` target handle goes to the
//!     workload that handle names, for as long as it is alive;
//!   - `callable` reports the workloads that are up, each with the interfaces
//!     of it the plugin may actually call.

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

/// Neither `workload-lifecycle` hook can call the workload it is about. A
/// workload is callable exactly while it is running, and a hook lands either
/// side of that: still deploying at `on-workload-bind`, already torn down at
/// `on-workload-unbind` — its service aborted and its warm instances dropped
/// before the hook runs. The plugin checks with `target.open` and gets `none`
/// both times.
///
/// The probe record lives in the plugin instance's own memory, so reading it
/// back after the workload has gone is also evidence the plugin never
/// restarted: a hook that called blind would trap, faulting the shared store
/// and clearing this.
#[tokio::test]
async fn test_a_lifecycle_hook_cannot_call_its_workload() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let client = reqwest::Client::new();

    // Beta is callable while it runs, which is what makes the hooks' `none`
    // meaningful rather than just "this plugin can never call anything".
    let (_status, body) = req(&client, &addr, "alpha", "/dispatch?id=wl-beta&msg=up").await?;
    assert_eq!(
        body, "beta:dispatch:wl-beta:up",
        "beta is callable while running"
    );

    let (status, body) = req(&client, &addr, "alpha", "/probe?id=wl-beta").await?;
    assert_eq!(status.as_u16(), 200, "/probe should succeed");
    assert_eq!(
        body, "bind:none",
        "a workload is still deploying when its bind hook runs, so it is not callable there"
    );

    h.workload_stop(WorkloadStopRequest {
        workload_id: "wl-beta".to_string(),
    })
    .await?;

    let (status, body) = req(&client, &addr, "alpha", "/probe?id=wl-beta").await?;
    assert_eq!(status.as_u16(), 200, "/probe should succeed");
    assert_eq!(
        body, "bind:none|unbind:none",
        "a workload is already torn down when its unbind hook runs, so it is not callable there \
         either — and the bind record surviving means the plugin did not restart"
    );

    // The plugin is the same live instance, still serving.
    let (status, body) = req(&client, &addr, "alpha", "/echo?msg=alive").await?;
    assert_eq!(status.as_u16(), 200, "the plugin should still be serving");
    assert_eq!(body, "alpha:echo:alive");

    Ok(())
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
        body, "beta:dispatch:wl-beta:hi",
        "a named target should override the calling workload, reaching beta from alpha's request"
    );

    // ...and the reverse, so the result cannot be an artifact of which workload
    // happened to deploy last.
    let (status, body) = req(&client, &addr, "beta", "/dispatch?id=wl-alpha&msg=yo").await?;
    assert_eq!(status.as_u16(), 200, "/dispatch should succeed");
    assert_eq!(
        body, "alpha:dispatch:wl-alpha:yo",
        "dispatch should route both ways"
    );

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

/// A target for a workload that is not callable is reported to the plugin as
/// `none` rather than trapping it. The plugin stays up and keeps serving, which
/// is what makes a `callable`-then-dispatch loop safe against a workload
/// stopping in between — a trap there would take the driver down and spend a
/// supervised restart every time a workload was undeployed.
#[tokio::test]
async fn test_unroutable_target_is_reported_not_trapped() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/dispatch?id=wl-gone&msg=hi").await?;
    assert_eq!(status.as_u16(), 200, "an unroutable target is not an error");
    assert_eq!(
        body, "unroutable:wl-gone",
        "the plugin should observe `none` from target.open rather than trapping"
    );

    // The plugin is still the same live instance serving the same store: a
    // restart would have been observable as a failed or slow call here.
    let (status, body) = req(&client, &addr, "alpha", "/echo?msg=still-here").await?;
    assert_eq!(status.as_u16(), 200, "the plugin should still be serving");
    assert_eq!(body, "alpha:echo:still-here");

    // Same again for a workload that WAS callable and then stopped — the race a
    // dispatch loop actually hits.
    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let (_status, body) = req(&client, &addr, "alpha", "/dispatch?id=wl-beta&msg=hi").await?;
    assert_eq!(
        body, "beta:dispatch:wl-beta:hi",
        "beta should be reachable first"
    );
    h.workload_stop(WorkloadStopRequest {
        workload_id: "wl-beta".to_string(),
    })
    .await?;
    let (status, body) = req(&client, &addr, "alpha", "/dispatch?id=wl-beta&msg=hi").await?;
    assert_eq!(status.as_u16(), 200, "a stopped target is not an error");
    assert_eq!(
        body, "unroutable:wl-beta",
        "a workload that stopped should read as not callable, not trap the plugin"
    );

    Ok(())
}

/// `callable` reports the workloads that are running and export an interface
/// the plugin imports — appearing when they deploy and going when they stop.
///
/// Each is reported with the interfaces of it that are callable, which is the
/// part a plugin cannot work out for itself. `events-plugin` imports two
/// workload-facing interfaces and `events-caller` exports only `handler`, so
/// `metrics` must be absent from every entry: calling an interface a workload
/// does not export has no error result to return, so the host could only trap
/// and take the shared plugin store down with it.
#[tokio::test]
async fn test_callable_reports_each_workloads_callable_interfaces() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, EVENTS_PLUGIN_WASM)
            .await?;
    h.workload_start(events_workload("wl-alpha", "alpha", "alpha"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(status.as_u16(), 200, "/callable should succeed");
    assert_eq!(
        body, "wl-alpha=acme:events/handler@0.1.0",
        "the deployed workload is callable on the one interface it exports, and the interface the \
         plugin imports but nothing exports is not reported as callable on it"
    );

    h.workload_start(events_workload("wl-beta", "beta", "beta"))
        .await?;
    let (_status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(
        body, "wl-alpha=acme:events/handler@0.1.0\nwl-beta=acme:events/handler@0.1.0",
        "a second workload should become callable once it is running, sorted by id"
    );

    h.workload_stop(WorkloadStopRequest {
        workload_id: "wl-beta".to_string(),
    })
    .await?;
    let (_status, body) = req(&client, &addr, "alpha", "/callable").await?;
    assert_eq!(
        body, "wl-alpha=acme:events/handler@0.1.0",
        "a stopped workload should no longer be callable"
    );

    Ok(())
}
