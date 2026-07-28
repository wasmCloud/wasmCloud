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
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::time::timeout;

use std::sync::Arc;

use wash_runtime::engine::Engine;
use wash_runtime::engine::workload::{UnresolvedWorkload, WorkloadComponent};
use wash_runtime::host::http::{DevRouter, DynamicRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::plugin::{HostPlugin, WitInterfaces};
use wash_runtime::types::{
    LocalResources, WorkloadState, WorkloadStatusRequest, WorkloadStopRequest,
};
use wash_runtime::wit::WitInterface;

mod common;
use common::{
    acme_kv_interface, component_workload_request, kv_plugin_caller_host_interfaces_with_config,
    start_host_with_component_plugin, start_host_with_component_plugin_by_host,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");
const KV_PLUGIN_SERVICE_WASM: &[u8] = include_bytes!("wasm/kv_plugin_service.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");
const BADLIFECYCLE_WASM: &[u8] = include_bytes!("wasm/badlifecycle.wasm");
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

/// Compilation-cache key for the one component every workload in this file
/// deploys. `digest` is used for nothing but the engine's compiled-component
/// cache (`Engine::load_component_bytes`), and without it EVERY `workload_start`
/// re-runs cranelift over `kv-plugin-caller`.
///
/// That matters here because the timing-sensitive tests below wrap
/// `workload_start` in a deadline: an uncached compile puts an unbounded,
/// machine-dependent cost inside a window meant to measure only the plugin's
/// lifecycle behavior. Every workload in this file deploys the same bytes, so a
/// single constant key is correct — the first deploy per engine compiles and
/// the rest hit the cache.
const CALLER_DIGEST: &str = "sha256:kv-plugin-caller-fixture";

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
    let mut request = component_workload_request(
        "kv-plugin-caller",
        host,
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces_with_config(host, config),
    );
    for component in &mut request.workload.components {
        component.digest = Some(CALLER_DIGEST.to_string());
    }
    request
}

/// A workload whose SERVICE — not a component — imports the plugin capability.
/// `host_interfaces` carries only `acme:kv`: the service exports `wasi:cli/run`
/// and serves no HTTP, so it is observed through a co-tenant caller rather than
/// addressed directly.
fn service_workload(name: &str) -> wash_runtime::types::WorkloadStartRequest {
    wash_runtime::types::WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: wash_runtime::types::Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: Some(wash_runtime::types::Service {
                digest: None,
                bytes: bytes::Bytes::from_static(KV_PLUGIN_SERVICE_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            components: vec![],
            host_interfaces: vec![acme_kv_interface()],
            volumes: vec![],
        },
    }
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

/// The ambient identity contract inside a lifecycle hook: `get-workload-id` is
/// the workload the hook concerns, and `get-component-id` reports nothing —
/// a hook is delivered about a whole workload, not on behalf of any one of its
/// items, so the host never names a component. The fixture appends a
/// `<hook>-ident-mismatch:<workload>|<component>` line when either half is
/// wrong, so an absence of those lines is the contract holding.
fn assert_hook_identity_contract(log: &[String]) {
    let mismatches: Vec<&String> = log
        .iter()
        .filter(|l| l.contains("ident-mismatch"))
        .collect();
    assert!(
        mismatches.is_empty(),
        "a lifecycle hook saw the wrong ambient identity — the host must report \
         the hook's workload and no component: {mismatches:?}"
    );
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
    // A component-only workload reports no service — the field is delivered
    // and empty rather than absent, and the service id does NOT leak into
    // `components` (the fixture renders `none` as `-`).
    assert!(
        info.contains(";service=-;"),
        "a workload without a service must report none, got: {info}"
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
    assert_hook_identity_contract(&log);
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

/// Unix epoch seconds `secs_from_now` in the future — the wall-clock threshold
/// after which a `trap-after-epoch-secs` bind starts trapping. The plugin reads
/// the same real clock, which (unlike the store) survives a restart.
/// How long the poison workload's bind stays clean before it is armed to trap.
///
/// Both deploys must complete inside this window, and the guest compares
/// `system_clock` at whole-second granularity — so a threshold `N` seconds out
/// is worth as little as `N-1` seconds depending on where in the current second
/// the test starts. Sized for headroom on a loaded runner rather than for
/// speed; the test's own sleep is derived from it below so the two cannot drift
/// apart.
const POISON_ARM_SECS: u64 = 5;

/// Bounded settle budget for the post-fault polling loops: eviction needs two
/// replay strikes, each costing a restart plus its backoff
/// (`200ms * restarts`, capped at `plugin_restart_backoff_max`) and a replay of
/// every bound workload. 50ms x this is the ceiling, not the expected wait.
const SETTLE_POLLS: usize = 200;

fn epoch_secs_from_now(secs_from_now: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now + secs_from_now
}

/// Quarantine + attribution + failed health, end to end. A workload's bind
/// succeeds on deploy but is armed to trap once a wall-clock threshold passes;
/// after the threshold a store fault forces a restart, and the workload's
/// REPLAYED bind then crash-loops. The supervisor attributes each replay trap
/// to that workload (via the serial-replay marker — the only reliable
/// attribution, since a trapping task's own handler never runs), strikes it,
/// and evicts it at the ceiling — WITHOUT taking the co-tenant workload down.
/// The evicted workload is reported failed, so its scheduling health becomes
/// `Error`, while the healthy workload keeps serving.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_poison_replay_bind_is_quarantined_and_evicted() -> Result<()> {
    let (addr, h, plugin) = start_host_keeping_plugin_by_host("127.0.0.1:0").await?;

    // A: a well-behaved co-tenant. P: armed to trap on any bind after ~2s.
    let good = caller_workload_with_config("kv-lc-good2", &[("tier", "gold")]);
    h.workload_start(good).await?;
    let poison = caller_workload_with_config(
        "kv-lc-poison",
        &[(
            "trap-after-epoch-secs",
            &epoch_secs_from_now(POISON_ARM_SECS).to_string(),
        )],
    );
    let poison_id = poison.workload_id.clone();
    h.workload_start(poison).await?;
    let client = reqwest::Client::new();

    // Both bind cleanly on deploy (threshold not yet passed) and serve.
    let (s, _) = req(&client, &addr, "kv-lc-good2", "/set?key=k&value=v").await?;
    assert!(s.is_success(), "good workload should serve pre-fault");
    let (s, _) = req(&client, &addr, "kv-lc-poison", "/set?key=k&value=v").await?;
    assert!(
        s.is_success(),
        "poison workload should bind and serve pre-fault"
    );

    // Let the wall-clock threshold pass, then fault the shared store so a
    // restart replays both binds — P's now traps.
    // Overshoot the threshold rather than landing on it: the guest's
    // comparison is `>=` at second granularity.
    tokio::time::sleep(Duration::from_millis(POISON_ARM_SECS * 1000 + 500)).await;
    let (s, _) = req(&client, &addr, "kv-lc-good2", "/boom").await?;
    assert!(s.is_server_error(), "the boom must fault the store");

    // The poison workload is evicted (dropped from the replay set) after its
    // strikes accrue, and reported failed.
    let mut evicted = false;
    for _ in 0..SETTLE_POLLS {
        if !plugin.is_bound(&poison_id) {
            evicted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        evicted,
        "the crash-looping poison bind must be evicted; bind-trap log was {:?}",
        plugin.bind_trap_log()
    );
    assert!(
        plugin.bind_trap_log().contains(&poison_id),
        "the replay trap must be attributed to the poison workload"
    );

    // Its scheduling health reflects the eviction.
    let mut failed = false;
    for _ in 0..SETTLE_POLLS {
        let status = h
            .workload_status(WorkloadStatusRequest {
                workload_id: poison_id.clone(),
            })
            .await?;
        if status.workload_status.workload_state == WorkloadState::Error {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(failed, "the evicted workload's health must become Error");

    // The co-tenant is unharmed: the plugin recovered and serves it. The
    // restart wiped the shared store's in-memory state (a documented
    // consequence), so assert a FRESH round-trip rather than the pre-fault key.
    let mut good_ok = false;
    for _ in 0..SETTLE_POLLS {
        let set = req(&client, &addr, "kv-lc-good2", "/set?key=after&value=ok").await;
        let get = req(&client, &addr, "kv-lc-good2", "/get?key=after").await;
        if let (Ok((s1, _)), Ok((s2, body))) = (set, get)
            && s1.is_success()
            && s2.as_u16() == 200
            && body == "ok"
        {
            good_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        good_ok,
        "the healthy co-tenant must keep serving after the poison workload is quarantined"
    );
    Ok(())
}

/// Deferred rollback unbind (Problem 2): a bind hook that overruns the host's
/// timeout fails the deploy PROMPTLY (at the budget, not the hook's full
/// duration), and the host still reclaims what the uncancellable hook
/// provisions once it returns — delivering the unbind after the bind, never
/// before, so no phantom state is orphaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_bind_timeout_defers_rollback_unbind() -> Result<()> {
    let (addr, h) =
        start_host_with_lifecycle_timeout("127.0.0.1:0", Duration::from_secs(1)).await?;
    // A healthy co-tenant to query the plugin's global lifecycle log and bind
    // records through (the slow workload's own deploy fails, so it is not
    // routable).
    h.workload_start(caller_workload_with_config("obs", &[("tier", "gold")]))
        .await?;
    let client = reqwest::Client::new();

    // A ~6s bind against a 1s budget: the deploy must fail near the budget, well
    // before the hook finishes. The gap is wide (1s budget vs 6s hook) so that
    // scheduler starvation under the concurrent test load — which inflates the
    // 1s timer's wall-clock — cannot blur "failed at the budget" into "waited
    // out the hook".
    let slow = caller_workload_with_config("slow", &[("slow-bind-ms", "6000")]);
    let slow_id = slow.workload_id.clone();
    let started = Instant::now();
    let resp = h.workload_start(slow).await?;
    let elapsed = started.elapsed();

    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Error,
        "an over-budget bind must fail the deploy; got {:?} ({})",
        resp.workload_status.workload_state,
        resp.workload_status.message
    );
    assert!(
        elapsed < Duration::from_millis(3500),
        "the deploy must fail near the ~1s budget, not wait out the ~6s hook (took {elapsed:?})"
    );

    // The hook keeps running; once it returns (~6s) the deferred rollback fires.
    // Eventually the log shows the slow bind both completed AND was unbound, and
    // its late-provisioned state is reclaimed (bind-info 404).
    let mut settled = false;
    for _ in 0..240 {
        let (ls, log) = req(&client, &addr, "obs", "/lifecycle-log").await?;
        let (bs, _) = req(
            &client,
            &addr,
            "obs",
            &format!("/bind-info?workload={slow_id}"),
        )
        .await?;
        if ls.as_u16() == 200
            && log.lines().any(|l| l == format!("bind:{slow_id}"))
            && log.lines().any(|l| l == format!("unbind:{slow_id}"))
            && bs.as_u16() == 404
        {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        settled,
        "the deferred unbind must run after the slow bind completes and reclaim its state; \
         log was {:?}",
        lifecycle_log(&client, &addr, "obs").await?
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

/// A workload's SERVICE binds to a plugin just as a component does, and is
/// reported in `workload-info.service` rather than being folded into
/// `components`. The id delivered at bind time is the same id
/// `identity.get-component-id` reports for calls the service itself makes — so
/// a plugin can correlate a service's capability calls back to its bind.
///
/// Multi-threaded: the host-header router uses `block_in_place`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_lifecycle_bind_reports_workload_service() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    // A component co-tenant: the service serves no HTTP, so its bind record and
    // its capability writes are read back through this workload.
    h.workload_start(caller_workload_with_config("svc-obs", &[]))
        .await?;

    let service = service_workload("kv-svc");
    let service_id = service.workload_id.clone();
    h.workload_start(service).await?;
    let client = reqwest::Client::new();

    // The bind record names the service and leaves `components` empty — the
    // service id must NOT appear as a component.
    let (status, info) = req(
        &client,
        &addr,
        "svc-obs",
        &format!("/bind-info?workload={service_id}"),
    )
    .await?;
    assert_eq!(status.as_u16(), 200, "the service's bind must be captured");
    let bound_service = info
        .split_once(";service=")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map(|(service, _)| service)
        .unwrap_or_default();
    assert!(
        !bound_service.is_empty() && bound_service != "-",
        "a service-bearing workload must report its service id, got: {info}"
    );
    assert!(
        info.contains(";components=;"),
        "a service-only workload has no components, got: {info}"
    );

    // The service ran and reached the plugin: it wrote the ambient identity the
    // plugin saw for its own call. Poll — `cli/run` is co-driven, so the write
    // lands shortly after the deploy returns.
    let mut whoami = String::new();
    for _ in 0..SETTLE_POLLS {
        let (status, body) = req(&client, &addr, "svc-obs", "/get?key=service-whoami").await?;
        if status.as_u16() == 200 && !body.is_empty() {
            whoami = body;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        whoami,
        format!("{service_id}|{bound_service}"),
        "the identity on a service's own capability call must be its workload id \
         and the very id delivered as workload-info.service"
    );

    // A capability call from the service names it; the service's own BIND does
    // not — the two halves of the contract, on the same workload.
    let log = lifecycle_log(&client, &addr, "svc-obs").await?;
    assert_hook_identity_contract(&log);
    Ok(())
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

/// A malformed lifecycle signature (here `on-workload-bind` takes a bare
/// string instead of the `workload-info` record) is rejected at registration —
/// `ComponentHostPlugin::new` fails with a clear message — rather than being
/// accepted and failing on the first workload deploy.
#[tokio::test]
async fn test_malformed_lifecycle_signature_rejected_at_registration() -> Result<()> {
    let engine = Engine::builder().build()?;
    let err = ComponentHostPlugin::new("badlifecycle-plugin", BADLIFECYCLE_WASM, engine)
        .map(|_| ())
        .expect_err("a malformed lifecycle signature must be rejected at construction");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("on-workload-bind") && msg.contains("workload-info record"),
        "the error must name the offending hook and expected shape, got: {msg}"
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
/// (`stop`/`start`), inspect its quarantine state, or drive its `HostPlugin`
/// hooks alongside a live host. `DevRouter` (last-resolved workload).
async fn start_host_keeping_plugin(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi, Arc<ComponentHostPlugin>)> {
    start_host_keeping_plugin_router(addr, DevRouter::default()).await
}

/// Like [`start_host_keeping_plugin`] but with a `Host`-header router so two
/// distinct workloads are individually reachable — for quarantine tests where
/// one workload's poison bind must not take the other down.
async fn start_host_keeping_plugin_by_host(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi, Arc<ComponentHostPlugin>)> {
    start_host_keeping_plugin_router(addr, DynamicRouter::default()).await
}

async fn start_host_keeping_plugin_router(
    addr: &str,
    router: impl wash_runtime::host::http::Router,
) -> Result<(std::net::SocketAddr, impl HostApi, Arc<ComponentHostPlugin>)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(router, addr.parse()?).await?;
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

/// Start a host-header-routed host whose component plugin uses a shortened
/// lifecycle-call `timeout` — for exercising the bind-timeout path without
/// waiting out the default budget.
async fn start_host_with_lifecycle_timeout(
    addr: &str,
    timeout: Duration,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DynamicRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine.clone())?
        .with_lifecycle_call_timeout(timeout);
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(plugin) as Arc<dyn HostPlugin>)?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((bound_addr, host))
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
