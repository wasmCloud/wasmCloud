//! Integration tests for the `wasmcloud:host/workload-lifecycle` export of
//! host component plugins: the host delivers `on-workload-bind` (with the
//! workload's identity and per-interface manifest config) before any
//! capability call from that workload, and `on-workload-unbind` when it goes
//! away. The `kv-plugin` fixture captures binds in guest state and exposes
//! them through `bound-config`/`bind-info`/`lifecycle-log` on its capability
//! interface, so the tests observe the hooks end-to-end through a caller
//! workload.
//!
//! Covers:
//!   - typed `workload-info` delivery (identity, components, interface
//!     bindings with version and config), correlated at call time via the
//!     identity import
//!   - bind rejection failing the workload deploy (and the plugin staying
//!     healthy for other workloads)
//!   - unbind on workload stop
//!   - bind replay into a fresh incarnation after a supervised restart
//!   - reserved `wasmcloud:host` exports never becoming workload-matchable
//!     capabilities
//!   - plugins without the export being entirely unaffected

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use std::sync::Arc;

use wash_runtime::engine::Engine;
use wash_runtime::engine::workload::{UnresolvedWorkload, WorkloadComponent};
use wash_runtime::host::http::{DevRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::plugin::{HostPlugin, WitInterfaces};
use wash_runtime::types::{LocalResources, WorkloadState, WorkloadStopRequest};
use wash_runtime::wit::WitInterface;

mod common;
use common::{
    acme_kv_interface, component_workload_request, kv_plugin_caller_host_interfaces_with_config,
    start_host_with_component_plugin, start_host_with_component_plugin_by_host,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");
const PLUGIN_ID: &str = "acme-kv-plugin";

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

/// A `kv-plugin-caller` workload addressed by `host`, with `config` set on its
/// `acme:kv` interface entry (the config `on-workload-bind` delivers).
fn caller_workload_with_config(
    host: &str,
    config: &[(&str, &str)],
) -> wash_runtime::types::WorkloadStartRequest {
    let config = config
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    component_workload_request(
        "kv-plugin-caller",
        host,
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces_with_config(host, config),
    )
}

/// The lines of `/lifecycle-log` as seen by the workload addressed by `host`.
async fn lifecycle_log(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    host: &str,
) -> Result<Vec<String>> {
    let (status, body) = req(client, addr, host, "/lifecycle-log").await?;
    anyhow::ensure!(status.as_u16() == 200, "/lifecycle-log got {status}");
    Ok(body.lines().map(str::to_string).collect())
}

/// Bind delivery: the plugin's `on-workload-bind` receives the typed
/// `workload-info` — id, name, namespace, component ids, and the matched
/// interface binding with its version and manifest config — before the
/// workload's first capability call, and capability calls correlate back to
/// that state via the identity import.
#[tokio::test]
async fn test_lifecycle_bind_delivers_typed_workload_info() -> Result<()> {
    let host = "kv-lc-info";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let request = caller_workload_with_config(host, &[("tier", "gold"), ("region", "us-east")]);
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;
    let client = reqwest::Client::new();

    // The very first capability call sees bind-time config: the bind completed
    // before any call from this workload was served.
    let (status, body) = req(&client, &addr, host, "/bound-config?key=tier").await?;
    assert_eq!(status.as_u16(), 200, "bind-time config must be captured");
    assert_eq!(body, "gold");
    let (status, body) = req(&client, &addr, host, "/bound-config?key=region").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "us-east");
    let (status, _) = req(&client, &addr, host, "/bound-config?key=absent").await?;
    assert_eq!(status.as_u16(), 404, "an absent config key reads as none");

    // Every typed field of workload-info, as the guest captured it.
    let (status, info) = req(
        &client,
        &addr,
        host,
        &format!("/bind-info?workload={workload_id}"),
    )
    .await?;
    assert_eq!(status.as_u16(), 200, "the bind must be captured by id");
    assert!(
        info.contains(&format!("id={workload_id};")),
        "workload id must round-trip, got: {info}"
    );
    assert!(
        info.contains(&format!(";name={host};")),
        "workload name must round-trip, got: {info}"
    );
    assert!(
        info.contains(";ns=test;"),
        "workload namespace must round-trip, got: {info}"
    );
    let components = info
        .split_once("components=")
        .and_then(|(_, rest)| rest.split_once(";ifaces="))
        .map(|(components, _)| components)
        .unwrap_or_default();

    // The delivered component ids are the same id space the identity import
    // reports at capability-call time: /whoami's component half must be
    // exactly the single-component workload's `components` entry.
    let (status, whoami) = req(&client, &addr, host, "/whoami").await?;
    assert_eq!(status.as_u16(), 200);
    let call_component = whoami
        .split_once('|')
        .map(|(_, component)| component)
        .unwrap_or_default();
    assert!(
        !call_component.is_empty(),
        "whoami must report a component id, got: {whoami}"
    );
    assert_eq!(
        components, call_component,
        "bind-time component ids must match the identity seen at call time"
    );
    // The matched interface binding: namespace/package/interface, the typed
    // version, and the manifest config (sorted by key: region < tier).
    assert!(
        info.contains("ifaces=acme:kv/store@0.1.0?region=us-east&tier=gold"),
        "the interface binding must carry version and config, got: {info}"
    );

    // The bind is also visible as an event in this incarnation's log.
    let log = lifecycle_log(&client, &addr, host).await?;
    assert_eq!(
        log,
        vec![format!("bind:{workload_id}")],
        "exactly one bind event for the one bound workload"
    );
    Ok(())
}

/// Bind rejection: a workload whose bind the plugin rejects fails to deploy,
/// with the guest's message surfaced — and the plugin keeps serving other
/// workloads afterwards.
#[tokio::test]
async fn test_lifecycle_bind_rejection_fails_deploy() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let rejected =
        caller_workload_with_config("kv-lc-rejected", &[("reject", "tenant quota exceeded")]);
    let rejected_id = rejected.workload_id.clone();
    let resp = h.workload_start(rejected).await?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Error,
        "a rejected bind must fail the deploy; got {:?} ({})",
        resp.workload_status.workload_state,
        resp.workload_status.message
    );
    assert!(
        resp.workload_status
            .message
            .contains("tenant quota exceeded"),
        "the guest's rejection message must surface, got: {}",
        resp.workload_status.message
    );

    // The plugin is unharmed: a clean workload binds and serves.
    let good = caller_workload_with_config("kv-lc-good", &[("tier", "silver")]);
    let good_id = good.workload_id.clone();
    h.workload_start(good).await?;
    let client = reqwest::Client::new();
    let (status, body) = req(&client, &addr, "kv-lc-good", "/bound-config?key=tier").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "silver");

    // The rejected workload never became a bind event (the log may carry its
    // post-failure unbind; it must not carry a bind).
    let log = lifecycle_log(&client, &addr, "kv-lc-good").await?;
    assert!(
        log.contains(&format!("bind:{good_id}")),
        "the clean workload's bind must be logged, got: {log:?}"
    );
    assert!(
        !log.contains(&format!("bind:{rejected_id}")),
        "a rejected bind must not be recorded as bound, got: {log:?}"
    );
    Ok(())
}

/// Unbind delivery: stopping a workload delivers `on-workload-unbind` to the
/// plugin, which reclaims that workload's state while continuing to serve the
/// other workload.
///
/// Multi-threaded: the host-header router uses `block_in_place`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lifecycle_unbind_on_workload_stop() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let first = caller_workload_with_config("kv-lc-stop-a", &[("tier", "gold")]);
    let first_id = first.workload_id.clone();
    h.workload_start(first).await?;
    let second = caller_workload_with_config("kv-lc-stop-b", &[]);
    let second_id = second.workload_id.clone();
    h.workload_start(second).await?;
    let client = reqwest::Client::new();

    h.workload_stop(WorkloadStopRequest {
        workload_id: first_id.clone(),
    })
    .await?;

    // The surviving workload observes the unbind event; the stopped workload's
    // bind state is gone while the survivor's remains.
    let mut log = Vec::new();
    for _ in 0..50 {
        log = lifecycle_log(&client, &addr, "kv-lc-stop-b").await?;
        if log.contains(&format!("unbind:{first_id}")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        log.contains(&format!("bind:{first_id}"))
            && log.contains(&format!("bind:{second_id}"))
            && log.contains(&format!("unbind:{first_id}")),
        "the stop must surface as an unbind event, got: {log:?}"
    );
    let (status, _) = req(
        &client,
        &addr,
        "kv-lc-stop-b",
        &format!("/bind-info?workload={first_id}"),
    )
    .await?;
    assert_eq!(
        status.as_u16(),
        404,
        "the stopped workload's bind state must be reclaimed"
    );
    let (status, _) = req(
        &client,
        &addr,
        "kv-lc-stop-b",
        &format!("/bind-info?workload={second_id}"),
    )
    .await?;
    assert_eq!(status.as_u16(), 200, "the survivor's bind state remains");
    Ok(())
}

/// Bind replay: a guest trap rebuilds the plugin store, wiping all guest state
/// — including everything `on-workload-bind` provisioned. The fresh
/// incarnation must re-receive a bind for the still-running workload before
/// serving its calls, so bind-time config is available again without the
/// workload redeploying.
#[tokio::test]
async fn test_lifecycle_replay_after_trap_restart() -> Result<()> {
    let host = "kv-lc-replay";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let request = caller_workload_with_config(host, &[("tier", "gold")]);
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, host, "/bound-config?key=tier").await?;
    assert_eq!((status.as_u16(), body.as_str()), (200, "gold"));

    // Trap the plugin store; the supervisor rebuilds it.
    let (status, _) = req(&client, &addr, host, "/boom").await?;
    assert!(status.is_server_error(), "the trap must fail, got {status}");

    // The fresh incarnation starts with EMPTY guest state, so a 200 here can
    // only come from the host replaying the bind into it. The status split is
    // the ordering assertion itself: a 5xx is the fault/restart window (retry),
    // but a 404 means a capability call was served BEFORE the replayed bind
    // completed — the replay-before-serving guarantee is broken — so it fails
    // the test immediately rather than being retried into a false pass.
    let mut recovered = false;
    for _ in 0..50 {
        if let Ok((status, body)) = req(&client, &addr, host, "/bound-config?key=tier").await {
            anyhow::ensure!(
                status.as_u16() != 404,
                "a capability call was served before the replayed bind completed"
            );
            if status.as_u16() == 200 && body == "gold" {
                recovered = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        recovered,
        "bind-time config must be replayed into the restarted plugin"
    );

    // And the new incarnation's log shows exactly the one replayed bind.
    let log = lifecycle_log(&client, &addr, host).await?;
    assert_eq!(
        log,
        vec![format!("bind:{workload_id}")],
        "the fresh incarnation sees exactly one (replayed) bind"
    );
    Ok(())
}

/// The `acme:kv` interface set with `config` on it, as the engine would pass
/// to `on_workload_bind` after matching.
fn acme_kv_matched(config: &[(&str, &str)]) -> HashSet<WitInterface> {
    let mut interface = acme_kv_interface();
    interface.config = config
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    [interface].into_iter().collect()
}

/// A minimal `UnresolvedWorkload` for driving the plugin's `HostPlugin` hooks
/// directly, without a running host.
fn bare_workload(id: &str) -> UnresolvedWorkload {
    UnresolvedWorkload::new(
        id,
        "bare",
        "test",
        None,
        std::iter::empty::<WorkloadComponent>(),
        Vec::new(),
    )
}

/// Reserved exports are host contracts, not capabilities: the lifecycle export
/// must not appear in the plugin's world, so no workload import can ever match
/// (and thus call) it — while the real capability still does.
#[tokio::test]
async fn test_reserved_lifecycle_export_not_workload_matchable() -> Result<()> {
    let engine = Engine::builder().build()?;
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine)?;
    let world = plugin.world();
    assert!(
        world
            .imports
            .iter()
            .any(|i| i.namespace == "acme" && i.package == "kv"),
        "the capability export must be matchable"
    );
    assert!(
        !world
            .imports
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "host"),
        "reserved wasmcloud:host exports must not be workload-matchable, got: {:?}",
        world.imports
    );
    Ok(())
}

/// Direct hook drive: the plugin delivers a bind whose typed fields include
/// the `Some` shapes (instance label, pre-release + build version metadata),
/// rejects one carrying the fixture's `reject` config with the guest's
/// message, and treats unbind as best-effort success.
#[tokio::test]
async fn test_lifecycle_hooks_driven_directly() -> Result<()> {
    let engine = Engine::builder().build()?;
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine)?;
    plugin.start().await?;

    // A bind exercising every optional field of the typed records: an instance
    // label and a version with pre-release and build metadata. Acceptance
    // proves the host-built values typecheck against the plugin's compiled
    // lifecycle types.
    let mut labeled = acme_kv_interface();
    labeled.name = Some("cache".to_string());
    labeled.version = Some(semver::Version::parse("0.1.0-rc.1+build5").unwrap());
    labeled.config = HashMap::from([("tier".to_string(), "gold".to_string())]);
    let labeled: HashSet<WitInterface> = [labeled].into_iter().collect();
    plugin
        .on_workload_bind(&bare_workload("wl-labeled"), WitInterfaces::new(&labeled))
        .await
        .context("a bind with labeled + pre-release interface data must be accepted")?;

    // The fixture's reject knob fails the bind with the configured message.
    let rejecting = acme_kv_matched(&[("reject", "no thanks")]);
    let err = plugin
        .on_workload_bind(
            &bare_workload("wl-rejected"),
            WitInterfaces::new(&rejecting),
        )
        .await
        .expect_err("a bind the guest rejects must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("rejected") && msg.contains("no thanks"),
        "the rejection must carry the guest's message, got: {msg}"
    );

    // Unbind is best-effort success, including for ids never bound.
    plugin
        .on_workload_unbind("wl-labeled", WitInterfaces::new(&labeled))
        .await?;
    plugin
        .on_workload_unbind("wl-never-bound", WitInterfaces::new(&labeled))
        .await?;

    plugin.stop().await?;
    Ok(())
}

/// A plugin that is not running cannot accept a bind (the workload deploy must
/// fail loudly), while unbind stays best-effort success.
#[tokio::test]
async fn test_lifecycle_bind_fails_when_plugin_not_running() -> Result<()> {
    let engine = Engine::builder().build()?;
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine)?;

    let matched = acme_kv_matched(&[]);
    let err = plugin
        .on_workload_bind(&bare_workload("wl-early"), WitInterfaces::new(&matched))
        .await
        .expect_err("a bind before start() must fail");
    assert!(
        format!("{err:#}").contains("not running"),
        "the failure must say the plugin is not running, got: {err:#}"
    );
    plugin
        .on_workload_unbind("wl-early", WitInterfaces::new(&matched))
        .await?;
    Ok(())
}

/// Like the common `start_host_with_component_plugin` helper, but returning
/// the plugin `Arc` too — for tests that drive the plugin's own lifecycle
/// (`stop`/`start`) or its `HostPlugin` hooks alongside a live host.
async fn start_host_keeping_plugin(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi, Arc<ComponentHostPlugin>)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let plugin = Arc::new(ComponentHostPlugin::new(
        PLUGIN_ID,
        KV_PLUGIN_WASM,
        engine.clone(),
    )?);
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::clone(&plugin) as Arc<dyn HostPlugin>)?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((bound_addr, host, plugin))
}

/// Replay covers EVERY bound workload, not just one: two workloads bind with
/// distinct config, one traps the shared store, and the fresh incarnation must
/// re-receive both binds — each completing before that workload's capability
/// calls are served (a 404 mid-recovery would mean a call outran its replayed
/// bind and fails immediately).
///
/// Multi-threaded: the host-header router uses `block_in_place`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lifecycle_replay_covers_all_bound_workloads() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let a = caller_workload_with_config("kv-lc-multi-a", &[("tier", "gold")]);
    let a_id = a.workload_id.clone();
    h.workload_start(a).await?;
    let b = caller_workload_with_config("kv-lc-multi-b", &[("tier", "silver")]);
    let b_id = b.workload_id.clone();
    h.workload_start(b).await?;
    let client = reqwest::Client::new();

    for (host, want) in [("kv-lc-multi-a", "gold"), ("kv-lc-multi-b", "silver")] {
        let (status, body) = req(&client, &addr, host, "/bound-config?key=tier").await?;
        assert_eq!((status.as_u16(), body.as_str()), (200, want));
    }

    let (status, _) = req(&client, &addr, "kv-lc-multi-a", "/boom").await?;
    assert!(status.is_server_error(), "the trap must fail, got {status}");

    for (host, want) in [("kv-lc-multi-a", "gold"), ("kv-lc-multi-b", "silver")] {
        let mut recovered = false;
        for _ in 0..50 {
            if let Ok((status, body)) = req(&client, &addr, host, "/bound-config?key=tier").await {
                anyhow::ensure!(
                    status.as_u16() != 404,
                    "{host}: a capability call was served before its replayed bind completed"
                );
                if status.as_u16() == 200 && body == want {
                    recovered = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            recovered,
            "{host}'s bind must be replayed after the restart"
        );
    }

    // The fresh incarnation's log holds exactly the two replayed binds.
    // Replays are spawned concurrently, so compare order-insensitively.
    let mut log = lifecycle_log(&client, &addr, "kv-lc-multi-b").await?;
    log.sort();
    let mut expected = vec![format!("bind:{a_id}"), format!("bind:{b_id}")];
    expected.sort();
    assert_eq!(
        log, expected,
        "the fresh incarnation sees exactly the two replayed binds"
    );
    Ok(())
}

/// `stop()` then `start()` replays leftover binds: workloads bound before the
/// stop are still bound after it, so the new incarnation must rebuild their
/// state from the plugin's bound-workloads map — the same replay path a fault
/// restart uses, but crossing a supervisor teardown instead.
#[tokio::test]
async fn test_lifecycle_replay_after_stop_start_cycle() -> Result<()> {
    let host = "kv-lc-stopstart";
    let (addr, h, plugin) = start_host_keeping_plugin("127.0.0.1:0").await?;
    let request = caller_workload_with_config(host, &[("tier", "gold")]);
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;
    let client = reqwest::Client::new();

    let (status, body) = req(&client, &addr, host, "/bound-config?key=tier").await?;
    assert_eq!((status.as_u16(), body.as_str()), (200, "gold"));

    plugin.stop().await?;
    // While stopped, capability calls fail promptly rather than queueing.
    let (status, _) = req(&client, &addr, host, "/bound-config?key=tier").await?;
    assert!(
        status.is_server_error(),
        "calls against a stopped plugin must fail, got {status}"
    );

    plugin.start().await?;
    // Same 404-is-failure semantics as the trap-restart test: the leftover
    // bind must be replayed before the fresh incarnation serves any call.
    let mut recovered = false;
    for _ in 0..50 {
        if let Ok((status, body)) = req(&client, &addr, host, "/bound-config?key=tier").await {
            anyhow::ensure!(
                status.as_u16() != 404,
                "a capability call was served before the replayed bind completed"
            );
            if status.as_u16() == 200 && body == "gold" {
                recovered = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(recovered, "leftover binds must replay across stop/start");

    let log = lifecycle_log(&client, &addr, host).await?;
    assert_eq!(
        log,
        vec![format!("bind:{workload_id}")],
        "the restarted incarnation sees exactly the one replayed bind"
    );
    Ok(())
}

/// Every optional field of the typed records round-trips to the guest with its
/// content intact: a direct bind on the live host's plugin carries an instance
/// label and a pre-release+build version, and the guest's rendering — read
/// back through a real caller workload — reproduces all of them.
#[tokio::test]
async fn test_lifecycle_optional_fields_round_trip() -> Result<()> {
    let host = "kv-lc-optional";
    let (addr, h, plugin) = start_host_keeping_plugin("127.0.0.1:0").await?;
    h.workload_start(caller_workload_with_config(host, &[]))
        .await?;
    let client = reqwest::Client::new();

    let mut labeled = acme_kv_interface();
    labeled.name = Some("cache".to_string());
    labeled.version = Some(semver::Version::parse("0.1.0-rc.1+build5").unwrap());
    labeled.config = HashMap::from([("tier".to_string(), "gold".to_string())]);
    let labeled: HashSet<WitInterface> = [labeled].into_iter().collect();
    plugin
        .on_workload_bind(&bare_workload("wl-optional"), WitInterfaces::new(&labeled))
        .await
        .context("the labeled bind must be accepted")?;

    let (status, info) = req(&client, &addr, host, "/bind-info?workload=wl-optional").await?;
    assert_eq!(status.as_u16(), 200, "the direct bind must be captured");
    assert!(
        info.contains("ifaces=acme:kv/store@0.1.0-rc.1+build5#cache?tier=gold"),
        "label, pre-release, and build metadata must round-trip, got: {info}"
    );
    Ok(())
}

/// A plugin WITHOUT the lifecycle export is entirely unaffected: binds and
/// unbinds are accepted as no-ops (nothing is delivered to the guest, nothing
/// is tracked for replay).
#[tokio::test]
async fn test_plugin_without_lifecycle_export_is_unaffected() -> Result<()> {
    let engine = Engine::builder().build()?;
    let plugin = ComponentHostPlugin::new("bridge-backend-plugin", BRIDGE_BACKEND_WASM, engine)?;
    plugin.start().await?;

    let matched: HashSet<WitInterface> = [WitInterface::from("wasmcloud:bridge/ops@0.1.0")]
        .into_iter()
        .collect();
    plugin
        .on_workload_bind(&bare_workload("wl-plain"), WitInterfaces::new(&matched))
        .await
        .context("a bind on a lifecycle-less plugin must be a no-op success")?;
    plugin
        .on_workload_unbind("wl-plain", WitInterfaces::new(&matched))
        .await?;
    plugin.stop().await?;
    Ok(())
}
