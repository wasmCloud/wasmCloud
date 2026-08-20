//! A host component plugin calling `wasi:cli/run` on a workload component.
//!
//! `run` is the one interface of `wasi:cli` the WASI base does not serve, so a
//! plugin importing it is asking to run a workload and the host routes the call
//! to whichever component of a bound workload exports `run`.
//!
//! The `cli-run-plugin` fixture sits on both sides: it imports `run` to call its
//! workloads, and exports `run` as its own dispatch loop. The export has to stay
//! a run loop — read as a capability, the plugin would claim to provide
//! `wasi:cli/run` to every workload importing it.
//!
//! The workload splits the two roles across components: `cli-run-plugin-caller`
//! serves HTTP and calls the plugin's control interface, `cli-run-callee`
//! exports `run` and nothing else.
//!
//! The plugin imports a second workload-facing interface, `acme:clirun/probe`,
//! purely for its signature: `check: async func() -> result<string>` is the
//! component model's `result<T>`, whose error arm carries no payload. It covers
//! the same payload-less error path `run` takes, with an ok payload beside it.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_incoming_handler_interface, req, start_host_with_component_plugin_by_host};

const CLI_RUN_PLUGIN_WASM: &[u8] = include_bytes!("wasm/cli_run_plugin.wasm");
const CLI_RUN_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/cli_run_plugin_caller.wasm");
const CLI_RUN_CALLEE_WASM: &[u8] = include_bytes!("wasm/cli_run_callee.wasm");
const PLUGIN_ID: &str = "acme-cli-run-plugin";

/// The plugin's control surface and the `probe` the plugin calls back into,
/// both carried by the HTTP-serving component.
fn acme_clirun_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "clirun".to_string(),
        interfaces: ["control".to_string(), "probe".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// `wasi:cli/run`, exported by the callee component for the plugin to call.
/// An interface entry is matched against ONE component at a time, so this is
/// its own entry rather than being folded into the caller's.
fn wasi_cli_run_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "cli".to_string(),
        interfaces: ["run".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.3.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// The same, with `PROBE_MODE` set on the HTTP-serving component so its
/// `probe.check` traps instead of answering.
fn cli_run_workload_with_probe_mode(
    workload_id: &str,
    host: &str,
    mode: &str,
) -> WorkloadStartRequest {
    let mut req = cli_run_workload(workload_id, host);
    for component in &mut req.workload.components {
        component
            .local_resources
            .environment
            .insert("PROBE_MODE".to_string(), mode.to_string());
    }
    req
}

/// A workload of two components: one serving HTTP and calling the plugin, one
/// exporting `wasi:cli/run` for the plugin to run.
fn cli_run_workload(workload_id: &str, host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "cli-run-plugin-caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CLI_RUN_PLUGIN_CALLER_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 4,
                },
                Component {
                    name: "cli-run-callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CLI_RUN_CALLEE_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 1000,
                    max_concurrency: 4,
                },
            ],
            host_interfaces: vec![
                http_incoming_handler_interface(host, None),
                acme_clirun_interface(),
                wasi_cli_run_interface(),
            ],
            volumes: vec![],
        },
    }
}

/// Both ways a plugin's `wasi:cli/run` call finds its workload: inherited from
/// the call it is serving, and named with a target handle.
#[tokio::test]
async fn test_plugin_runs_a_workload_component() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, CLI_RUN_PLUGIN_WASM)
            .await?;
    h.workload_start(cli_run_workload("wl-runner", "runner"))
        .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "runner", "/run-inherited").await?;
    assert_eq!(status.as_u16(), 200, "/run-inherited should succeed");
    assert_eq!(
        body, "ok",
        "with no target held the run goes to the calling workload's `run` component"
    );

    let (status, body) = req(&client, &addr, "runner", "/run-workload?id=wl-runner").await?;
    assert_eq!(status.as_u16(), 200, "/run-workload should succeed");
    assert_eq!(body, "ok", "a target handle names the same workload");

    // A workload that does not exist is not callable, so the plugin's `open`
    // answers `none` — it never makes a call that could fail.
    let (status, body) = req(&client, &addr, "runner", "/run-workload?id=wl-missing").await?;
    assert_eq!(status.as_u16(), 200, "/run-workload should still answer");
    assert_eq!(body, "unroutable:wl-missing");

    Ok(())
}

/// The plugin's OWN exported `wasi:cli/run` is co-driven as its long-running
/// work, not served as a capability, and it can run a workload from outside any
/// inbound call — a trigger driving workloads off its own clock.
#[tokio::test]
async fn test_plugin_cli_run_loop_dispatches() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, CLI_RUN_PLUGIN_WASM)
            .await?;
    h.workload_start(cli_run_workload("wl-ticker", "ticker"))
        .await?;
    let client = reqwest::Client::new();

    // The loop ticks every 20ms; poll rather than sleep a fixed span.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let dispatched = loop {
        let (status, body) = req(&client, &addr, "ticker", "/dispatched").await?;
        assert_eq!(status.as_u16(), 200, "/dispatched should succeed");
        if body.starts_with("wl-ticker=") {
            break body;
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the plugin's cli/run loop never dispatched a run; last answer: {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let count: u64 = dispatched
        .trim_start_matches("wl-ticker=")
        .parse()
        .unwrap_or_default();
    assert!(
        count > 0,
        "the loop should have dispatched at least one run"
    );

    Ok(())
}

/// A workload-facing import in the `result<T>` form — an ok payload beside an
/// error arm that carries nothing. The host has to report a failure through it
/// as a value, which means writing an empty `err` into a slot whose ok side
/// holds a `string`; getting that value shape wrong would trap the plugin.
#[tokio::test]
async fn test_a_failure_is_reported_through_an_empty_error_arm() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, CLI_RUN_PLUGIN_WASM)
            .await?;
    h.workload_start(cli_run_workload_with_probe_mode(
        "wl-ok", "probe-ok", "fine",
    ))
    .await?;
    h.workload_start(cli_run_workload_with_probe_mode(
        "wl-trap",
        "probe-trap",
        "trap",
    ))
    .await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, "probe-ok", "/probe?id=wl-ok").await?;
    assert_eq!(status.as_u16(), 200, "/probe should succeed");
    assert_eq!(body, "ok:checked:fine", "the ok arm carries its payload");

    // The callee traps. The plugin receives an errorless `err` and keeps
    // serving — a trap here would take down the store it shares with wl-ok.
    let (status, body) = req(&client, &addr, "probe-ok", "/probe?id=wl-trap").await?;
    assert_eq!(status.as_u16(), 200, "/probe should still answer");
    assert_eq!(body, "err", "a failure reads as the payload-less error arm");

    // Same plugin instance, still serving.
    let (status, body) = req(&client, &addr, "probe-ok", "/probe?id=wl-ok").await?;
    assert_eq!(status.as_u16(), 200, "the plugin should still be serving");
    assert_eq!(body, "ok:checked:fine");

    Ok(())
}
