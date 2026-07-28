//! Integration tests for host component plugins: a bespoke, handle-free
//! `acme:kv/store` capability provided by the `kv-plugin` component running in
//! its own persistent, host-scoped store, imported by the `kv-plugin-caller`
//! workload and driven over HTTP.
//!
//! Covers the technical-plan P0 areas:
//!   A — linking + handle-free end-to-end (`/set` then `/get` round-trips)
//!   B — host-scoped singleton lifecycle (shared across workloads; state
//!       survives a workload restart)
//!   C — concurrent capability calls, including no head-of-line blocking

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, WorkloadState, WorkloadStopRequest};

mod common;
use common::{
    component_workload_request, kv_plugin_caller_host_interfaces, start_host_with_component_plugin,
    start_host_with_component_plugin_by_host, start_host_with_component_plugin_max_restarts,
    start_host_with_p3_http_handler,
};

const KV_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("wasm/kv_plugin_caller.wasm");
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

/// A `kv-plugin-caller` workload addressed by `host`.
fn caller_workload(host: &str) -> wash_runtime::types::WorkloadStartRequest {
    component_workload_request(
        "kv-plugin-caller",
        host,
        KV_PLUGIN_CALLER_WASM,
        LocalResources::default(),
        kv_plugin_caller_host_interfaces(host),
    )
}

/// A: a workload's `acme:kv/store` import resolves to the host component plugin;
/// `set` then `get` round-trips the value through the plugin's persistent store,
/// and a missing key returns `none` (surfaced as 404 by the fixture).
#[tokio::test]
async fn test_component_plugin_set_get_roundtrip() -> Result<()> {
    let host = "kv-roundtrip";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    let (status, _) = req(&client, &addr, host, "/set?key=greeting&value=hello").await?;
    assert!(status.is_success(), "/set should succeed, got {status}");

    let (status, body) = req(&client, &addr, host, "/get?key=greeting").await?;
    assert_eq!(status.as_u16(), 200, "/get of a set key should be 200");
    assert_eq!(
        body, "hello",
        "value should round-trip through the plugin store"
    );

    let (status, _) = req(&client, &addr, host, "/get?key=absent").await?;
    assert_eq!(
        status.as_u16(),
        404,
        "/get of an absent key should be 404 (none)"
    );

    // delete removes the key: the next get sees none.
    let (status, _) = req(&client, &addr, host, "/delete?key=greeting").await?;
    assert!(status.is_success(), "/delete should succeed, got {status}");
    let (status, _) = req(&client, &addr, host, "/get?key=greeting").await?;
    assert_eq!(status.as_u16(), 404, "a deleted key reads back as none");

    Ok(())
}

/// A (negative): a workload importing a capability that no plugin provides fails
/// to resolve with a clear error naming the interface — not a silent unsatisfied
/// import or a hang. (`workload_start` reports failure via the workload state.)
#[tokio::test]
async fn test_component_plugin_missing_provider_errors() -> Result<()> {
    let host = "kv-missing";
    // A plain p3 host has no `acme:kv` provider.
    let (_addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    let resp = h.workload_start(caller_workload(host)).await?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Error,
        "a workload importing an unprovided capability must fail to start; got state {:?} ({})",
        resp.workload_status.workload_state,
        resp.workload_status.message
    );
    // The message embeds the unmatched `WitInterface` debug form, so the
    // namespace and package appear as separate fields.
    assert!(
        resp.workload_status.message.contains("not available")
            && resp.workload_status.message.contains("acme")
            && resp.workload_status.message.contains("kv"),
        "error should name the unsatisfied interface, got: {}",
        resp.workload_status.message
    );
    Ok(())
}

/// B: the plugin is a host-scoped singleton — two workloads bound to it reach
/// the SAME instance, so state written through one is visible through the other.
///
/// Uses the host-header router so the two callers are genuinely distinct
/// workloads (the `DevRouter` would send both requests to the last-resolved
/// workload, making the assertion vacuous). Runs multi-threaded so the live
/// HTTP server serves requests in parallel with the test's request loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_component_plugin_shared_across_workloads() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("caller-a")).await?;
    h.workload_start(caller_workload("caller-b")).await?;
    let client = reqwest::Client::new();

    let (status, _) = req(&client, &addr, "caller-a", "/set?key=shared&value=fromA").await?;
    assert!(status.is_success());

    let (status, body) = req(&client, &addr, "caller-b", "/get?key=shared").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(
        body, "fromA",
        "a second workload must see the first's write — proving one shared instance"
    );
    Ok(())
}

/// B: the plugin instance is tied to the host, not to any workload — its state
/// survives a workload being stopped and started again.
#[tokio::test]
async fn test_component_plugin_state_survives_workload_restart() -> Result<()> {
    let host = "kv-restart";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let client = reqwest::Client::new();

    let first = caller_workload(host);
    let first_id = first.workload_id.clone();
    h.workload_start(first).await?;

    let (status, _) = req(&client, &addr, host, "/set?key=persist&value=v1").await?;
    assert!(status.is_success());

    h.workload_stop(WorkloadStopRequest {
        workload_id: first_id,
    })
    .await?;

    // Redeploy a fresh workload; the plugin (and its state) outlived the old one.
    h.workload_start(caller_workload(host)).await?;
    let (status, body) = req(&client, &addr, host, "/get?key=persist").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(
        body, "v1",
        "plugin state must survive a workload restart (lifecycle is host-scoped)"
    );
    Ok(())
}

/// C: many concurrent capability calls into the single plugin store all complete
/// correctly — one `Accessor::spawn` task per call, interleaving at await points.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_concurrent_calls() -> Result<()> {
    let host = "kv-concurrent";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    let mut handles = Vec::new();
    for i in 0..30u32 {
        let client = client.clone();
        let host = host.to_string();
        handles.push(tokio::spawn(async move {
            let (s, _) = req(&client, &addr, &host, &format!("/set?key=k{i}&value=v{i}")).await?;
            anyhow::ensure!(s.is_success(), "set k{i} failed: {s}");
            let (s, body) = req(&client, &addr, &host, &format!("/get?key=k{i}")).await?;
            anyhow::ensure!(s.as_u16() == 200, "get k{i} not 200: {s}");
            anyhow::ensure!(body == format!("v{i}"), "get k{i} got {body}");
            Ok::<(), anyhow::Error>(())
        }));
    }
    for handle in handles {
        handle.await??;
    }
    Ok(())
}

/// Streams + futures relocate across the capability boundary: a caller->plugin
/// `stream<u8>` (`/total`), a plugin->caller `stream<u8>` fed by the persistent
/// store (`/emit`), and a plugin->caller `future<u64>` (`/eventually`).
#[tokio::test]
async fn test_component_plugin_streams_and_futures() -> Result<()> {
    let host = "kv-relocate";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    // caller -> plugin stream
    let (status, body) = req(&client, &addr, host, "/total?count=5000").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "5000", "plugin should total every streamed byte");

    // plugin -> caller stream (persistent store feeds it while the caller drains)
    let (status, body) = req(&client, &addr, host, "/emit?count=9000").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(
        body, "9000",
        "caller should drain every byte the plugin emits"
    );

    // plugin -> caller future
    let (status, body) = req(&client, &addr, host, "/eventually?value=4242").await?;
    assert_eq!(status.as_u16(), 200);
    assert_eq!(body, "4242", "future should resolve to the plugin's value");

    Ok(())
}

/// D: re-entrancy + the in-flight-task ceiling. The plugin imports its own
/// capability and `recurse(n)` re-enters the persistent store across the bridge
/// `n` times — exercising suspended-store re-entry (the concurrent-driven store
/// accepts the re-entrant call). A shallow recursion completes; a runaway
/// recursion whose live hops exceed the store's in-flight ceiling is rejected
/// with a clear error (surfaced as 5xx) rather than hanging or exhausting
/// resources, and the plugin recovers under supervision afterward.
#[tokio::test]
async fn test_component_plugin_reentrancy_and_ceiling() -> Result<()> {
    let host = "kv-recurse";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    // Shallow recursion re-enters the store a few times and completes.
    let (status, body) = req(&client, &addr, host, "/recurse?n=3").await?;
    assert_eq!(status.as_u16(), 200, "shallow recursion should complete");
    assert_eq!(body, "3", "recurse(3) counts 3 re-entrant hops");

    // A runaway recursion (far more live hops than the in-flight ceiling, 512)
    // is rejected: the call traps (5xx) rather than hanging or exhausting memory.
    let (status, _) = req(&client, &addr, host, "/recurse?n=2000").await?;
    assert!(
        status.is_server_error(),
        "a runaway recursion must be bounded by the in-flight ceiling and trap, got {status}"
    );

    // The plugin recovers under supervision and serves again.
    let mut recovered = false;
    for _ in 0..50 {
        if let Ok((s, body)) = req(&client, &addr, host, "/recurse?n=2").await
            && s.as_u16() == 200
            && body == "2"
        {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        recovered,
        "plugin must recover and serve after a bounded runaway"
    );

    Ok(())
}

/// D (resilience): a guest trap in one capability call must not permanently
/// poison the shared plugin store. `/boom` panics inside the plugin; the
/// caller's request fails, but the plugin recovers under supervision and keeps
/// serving other callers — genuinely distinct workloads via the host-header
/// router (the `DevRouter` would collapse both callers onto one workload).
///
/// NOTE on blast radius: because the plugin is a host-scoped singleton, a guest
/// trap faults the driver and the supervisor restarts the whole store — so all
/// tenants' in-memory state is lost on recovery (a documented consequence of
/// the shared-singleton model). This test asserts *recovery* (the plugin serves
/// again), not state survival.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_component_plugin_survives_guest_trap() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("boom-a")).await?;
    h.workload_start(caller_workload("boom-b")).await?;
    let client = reqwest::Client::new();

    // Trigger a guest trap; the trapping call must fail (not hang).
    let (status, _) = req(&client, &addr, "boom-a", "/boom").await?;
    assert!(
        status.is_server_error(),
        "the trapping call must fail, got {status}"
    );

    // The plugin must recover and serve other callers. Allow a brief window for
    // the supervisor to restart the store.
    let mut recovered = false;
    for _ in 0..50 {
        let set = req(&client, &addr, "boom-b", "/set?key=post&value=ok").await;
        let get = req(&client, &addr, "boom-b", "/get?key=post").await;
        if let (Ok((s1, _)), Ok((s2, body))) = (set, get)
            && s1.is_success()
            && s2.as_u16() == 200
            && body == "ok"
        {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        recovered,
        "plugin must recover and serve requests after a guest trap poisons its instance"
    );

    Ok(())
}

/// D (resilience): a guest trap must fail sibling IN-FLIGHT calls promptly —
/// not leave them hanging. A long-running `begin` from one workload is
/// mid-flight when another workload traps the store; the begin request must
/// complete with an error (its reply channel died with the incarnation), and
/// the plugin must then recover.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_trap_fails_inflight_calls_cleanly() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("mid-a")).await?;
    h.workload_start(caller_workload("mid-b")).await?;
    let client = reqwest::Client::new();

    // A long begin (~200 * 25ms ≈ 5s) owned by workload "mid-b".
    let inflight = tokio::spawn({
        let client = client.clone();
        async move {
            req(
                &client,
                &addr,
                "mid-b",
                "/begin?name=mid&ticks=200&tick-ms=25",
            )
            .await
        }
    });

    // Only trap once the begin is observably running on the plugin store.
    let seen = wait_progress(&client, &addr, "mid-b", "mid", 2).await?;
    assert!(
        seen >= 2,
        "begin should be running before the trap (saw {seen})"
    );
    let (status, _) = req(&client, &addr, "mid-a", "/boom").await?;
    assert!(
        status.is_server_error(),
        "the trapping call must fail, got {status}"
    );

    // The in-flight begin must fail promptly (5xx), not hang until its own
    // 5s duration or a timeout: its task died with the faulted incarnation.
    let (status, _) = timeout(Duration::from_secs(3), inflight).await???;
    assert!(
        status.is_server_error(),
        "an in-flight call must error when the store faults, got {status}"
    );

    // And the plugin recovers for both workloads.
    let mut recovered = false;
    for _ in 0..50 {
        if let Ok((s, _)) = req(&client, &addr, "mid-b", "/set?key=r&value=1").await
            && s.is_success()
        {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(recovered, "plugin must recover after the trap");
    Ok(())
}

/// D (deadlock regression): many concurrent re-entrant call chains must not
/// deadlock. Each `/recurse` re-enters the plugin store several times; firing a
/// burst of them concurrently would deadlock if the driver bounded in-flight
/// tasks by acquiring a permit in the serve loop (a re-entrant sub-call could
/// never be admitted while its ancestor holds a permit). All must complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_concurrent_reentrancy_no_deadlock() -> Result<()> {
    let host = "kv-reentry-burst";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    let mut handles = Vec::new();
    for _ in 0..40u32 {
        let client = client.clone();
        let host = host.to_string();
        handles.push(tokio::spawn(async move {
            // depth 5: re-enters the plugin store 5 times per chain.
            let (s, body) = req(&client, &addr, &host, "/recurse?n=5").await?;
            anyhow::ensure!(s.as_u16() == 200 && body == "5", "recurse got {s}: {body}");
            Ok::<(), anyhow::Error>(())
        }));
    }
    // A generous overall deadline; a deadlock would blow past it.
    for handle in handles {
        timeout(Duration::from_secs(30), handle).await???;
    }
    Ok(())
}

/// F (resources): a cross-store `resource`. `open` returns `own<bucket>` (a
/// proxy in the caller), `set`/`get` are `borrow<bucket>` methods routed to the
/// real resource in the plugin store, and dropping the proxy frees the real one.
#[tokio::test]
async fn test_component_plugin_resource_roundtrip_and_drop() -> Result<()> {
    let host = "kv-resource";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    // own<bucket> result + borrow<bucket> method calls round-trip a value.
    let (status, body) = req(&client, &addr, host, "/bucket?name=b1&key=k&value=hello").await?;
    assert_eq!(
        status.as_u16(),
        200,
        "bucket set/get via proxy should succeed"
    );
    assert_eq!(
        body, "hello",
        "value should round-trip through the bucket proxy"
    );

    // Each request opens then drops a bucket; dropping the proxy must route a
    // drop that frees the real resource in the plugin store (no leak).
    let baseline: u64 = {
        let (_, b) = req(&client, &addr, host, "/dropped-buckets").await?;
        b.parse().unwrap_or(0)
    };
    for _ in 0..3 {
        let (s, _) = req(&client, &addr, host, "/bucket?name=b&key=k&value=v").await?;
        assert_eq!(s.as_u16(), 200);
    }
    // Allow for async drop routing to settle.
    let mut dropped = baseline;
    for _ in 0..50 {
        let (_, b) = req(&client, &addr, host, "/dropped-buckets").await?;
        dropped = b.parse().unwrap_or(0);
        if dropped >= baseline + 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        dropped >= baseline + 3,
        "dropping each caller proxy must free the real bucket in the plugin store \
         (dropped {dropped}, expected >= {})",
        baseline + 3
    );
    Ok(())
}

/// F (resources — churn/leak): waves of concurrent open→use→drop cycles, each
/// drop making the plugin driver step out of `run_concurrent` to free the real
/// resource without breaking the other in-flight calls (preserved across the
/// step-out) — every request completes correctly, and every real is reclaimed
/// exactly once. The plugin's host-side `ResourceRegistry` holds a real per
/// `open`; a proxy drop that failed to route back would leave reals
/// accumulating in the long-lived store until restart. The guest's destructor
/// counter (`/dropped-buckets`) is the observable: a real's destructor runs
/// only when the host takes it out of the registry and drops it, so the count
/// advancing by exactly the number opened proves no real leaked (would fall
/// short) and none was freed twice (would overshoot).
///
/// The client pools no connections (`pool_max_idle_per_host(0)`): a pooled GET
/// retried on a stale connection would silently re-open an extra bucket and
/// break the exactly-once accounting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_resource_no_leak_under_churn() -> Result<()> {
    let host = "kv-res-leak";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    let (_, base) = req(&client, &addr, host, "/dropped-buckets").await?;
    let baseline: u64 = base.parse().unwrap_or(0);

    // Bounded-width churn: waves of 40 keep in-flight well under the ceiling while
    // opening (and dropping) 400 distinct reals across the bridge.
    const WAVES: u32 = 10;
    const WIDTH: u32 = 40;
    for wave in 0..WAVES {
        let mut handles = Vec::new();
        for i in 0..WIDTH {
            let client = client.clone();
            let host = host.to_string();
            handles.push(tokio::spawn(async move {
                let (s, body) = req(
                    &client,
                    &addr,
                    &host,
                    &format!("/bucket?name=w{wave}-{i}&key=k&value=w{wave}v{i}"),
                )
                .await?;
                anyhow::ensure!(
                    s.as_u16() == 200 && body == format!("w{wave}v{i}"),
                    "wave {wave} bucket {i} got {s}: {body}"
                );
                Ok::<(), anyhow::Error>(())
            }));
        }
        for handle in handles {
            timeout(Duration::from_secs(30), handle).await???;
        }
    }

    // Drops flush when the driver steps out of `run_concurrent`, so the count can
    // trail the last request; poll until it settles, then require it landed
    // exactly on the number opened.
    let target = baseline + u64::from(WAVES * WIDTH);
    let mut dropped = baseline;
    for _ in 0..100 {
        let (_, b) = req(&client, &addr, host, "/dropped-buckets").await?;
        dropped = b.parse().unwrap_or(0);
        if dropped >= target {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        dropped,
        target,
        "every opened bucket must be dropped exactly once (opened {}, dropped {})",
        target - baseline,
        dropped.saturating_sub(baseline)
    );

    // The long-lived store is still healthy after the churn.
    let (s, _) = req(&client, &addr, host, "/set?key=after&value=ok").await?;
    assert!(
        s.is_success(),
        "store must still serve after resource churn"
    );
    let (_, body) = req(&client, &addr, host, "/get?key=after").await?;
    assert_eq!(body, "ok", "store must serve correct values after churn");
    Ok(())
}

/// F (resources): every `open` returns an independent bucket — a key set in one
/// bucket is ABSENT from a freshly opened one (buckets are distinct resources,
/// not views onto one shared map), including across two genuinely distinct
/// workloads (host-header routing).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_component_plugin_resource_isolation() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("res-a")).await?;
    h.workload_start(caller_workload("res-b")).await?;
    let client = reqwest::Client::new();

    // A sets a key in its own bucket (and reads it back through the proxy).
    let (s, body) = req(&client, &addr, "res-a", "/bucket?name=x&key=only-a&value=1").await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(body, "1");

    // A fresh bucket opened by ANOTHER workload must not see that key. If the
    // plugin backed all buckets with one shared map this would return 200 "1".
    let (s, _) = req(&client, &addr, "res-b", "/bucket-get?name=x&key=only-a").await?;
    assert_eq!(
        s.as_u16(),
        404,
        "a fresh bucket in another workload must not see A's key"
    );

    // Nor does a fresh bucket in the SAME workload: isolation is per-resource,
    // not per-caller.
    let (s, _) = req(&client, &addr, "res-a", "/bucket-get?name=x&key=only-a").await?;
    assert_eq!(
        s.as_u16(),
        404,
        "a fresh bucket in the same workload must not see the earlier bucket's key"
    );
    Ok(())
}

/// E (per-caller partitioning): two workloads sharing the singleton plugin have
/// ISOLATED partitioned state, keyed by the `wasmcloud:host/identity` import,
/// while the global map stays shared. Proves the `(workload_id, component_id)`
/// threading + the identity import; concurrent interleaving is covered by
/// [`test_component_plugin_partitioning_under_concurrency`].
///
/// Runs multi-threaded so the live HTTP server serves requests in parallel with
/// the test's request loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_component_plugin_per_caller_partitioning() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("part-a")).await?;
    h.workload_start(caller_workload("part-b")).await?;
    let client = reqwest::Client::new();

    // A writes to its own partition and reads it back.
    let (s, _) = req(&client, &addr, "part-a", "/pset?key=k&value=fromA").await?;
    assert!(s.is_success());
    let (s, body) = req(&client, &addr, "part-a", "/pget?key=k").await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(body, "fromA");

    // B does NOT see A's partitioned value (isolated).
    let (s, _) = req(&client, &addr, "part-b", "/pget?key=k").await?;
    assert_eq!(s.as_u16(), 404, "B must not see A's per-caller value");

    // B writes its own value under the same key; A is unaffected.
    let (s, _) = req(&client, &addr, "part-b", "/pset?key=k&value=fromB").await?;
    assert!(s.is_success());
    let (_, body) = req(&client, &addr, "part-b", "/pget?key=k").await?;
    assert_eq!(body, "fromB");
    let (_, body) = req(&client, &addr, "part-a", "/pget?key=k").await?;
    assert_eq!(
        body, "fromA",
        "A's partition must be unaffected by B's write to the same key"
    );

    // Sanity: the GLOBAL map stays shared across the two workloads.
    let (s, _) = req(&client, &addr, "part-a", "/set?key=g&value=shared").await?;
    assert!(s.is_success());
    let (_, body) = req(&client, &addr, "part-b", "/get?key=g").await?;
    assert_eq!(body, "shared", "the global map stays shared across callers");

    Ok(())
}

/// E (per-caller partitioning under concurrency): writes from two workloads
/// interleave on the single plugin store, yet each write lands in its own caller's
/// partition and no caller can read the other's keys. This exercises the strict
/// ambient attribution path — the identity import walks its own root guest task
/// (`async_call_stack`) rather than reading a shared slot, so an identity read
/// scheduled amid another workload's call cannot cross wires. A shared-slot
/// approach fails here: a concurrent call overwrites the slot between the call
/// task setting it and the guest reading it, misattributing the write.
///
/// Writes run fully concurrently; attribution is checked only after they quiesce,
/// so the check does not depend on read-your-writes ordering holding mid-flight
/// (the cooperative store can lag a same-caller readback by a call — orthogonal to
/// identity and not what this test guards).
///
/// Runs multi-threaded so requests are served in parallel under concurrency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_partitioning_under_concurrency() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("conc-a")).await?;
    h.workload_start(caller_workload("conc-b")).await?;
    let client = reqwest::Client::new();

    // Each caller concurrently writes a disjoint set of keys, every value tagged
    // with its writer. Interleaving these two callers' calls on one store is what
    // stresses ambient identity: a misattributed write lands in the wrong
    // partition.
    const KEYS: u32 = 24;
    let mut handles = Vec::new();
    for caller in ["conc-a", "conc-b"] {
        for i in 0..KEYS {
            let client = client.clone();
            handles.push(tokio::spawn(async move {
                let (s, _) = req(
                    &client,
                    &addr,
                    caller,
                    &format!("/pset?key=k{i}&value={caller}-{i}"),
                )
                .await?;
                anyhow::ensure!(s.is_success(), "{caller} pset k{i} failed: {s}");
                Ok::<(), anyhow::Error>(())
            }));
        }
    }
    for handle in handles {
        handle.await??;
    }

    // Quiescent: each caller must see exactly its own writes under each key, and
    // never the other caller's value. Crossing on write would surface as a wrong
    // value here; crossing on read would surface as reading the other partition.
    for (caller, other) in [("conc-a", "conc-b"), ("conc-b", "conc-a")] {
        for i in 0..KEYS {
            let (s, body) = req(&client, &addr, caller, &format!("/pget?key=k{i}")).await?;
            assert_eq!(s.as_u16(), 200, "{caller} lost its own write to k{i}");
            assert_eq!(
                body,
                format!("{caller}-{i}"),
                "{caller}'s partition holds {other}'s value for k{i} — identity crossed"
            );
        }
    }
    Ok(())
}

/// C: no head-of-line blocking — while one slow capability call is in flight
/// (cooperatively awaiting a timer inside the plugin), concurrent fast calls to
/// the same store still complete. If the store serialized calls, no fast call
/// could finish until the slow one returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_no_head_of_line_blocking() -> Result<()> {
    let host = "kv-hol";
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    // Seed a key the fast calls will read.
    let (status, _) = req(&client, &addr, host, "/set?key=fast&value=quick").await?;
    assert!(status.is_success());

    // Kick off a slow call (the plugin awaits a ~2s timer, yielding the store).
    let slow = tokio::spawn({
        let client = client.clone();
        let host = host.to_string();
        async move { req(&client, &addr, &host, "/slow?millis=2000").await }
    });

    // While the slow call is in flight, fast reads must keep completing.
    let mut served_during_slow = 0u32;
    while !slow.is_finished() && served_during_slow < 5 {
        match req(&client, &addr, host, "/get?key=fast").await {
            Ok((s, body)) if s.as_u16() == 200 && body == "quick" => served_during_slow += 1,
            Ok((s, body)) => anyhow::bail!("unexpected fast /get result {s}: {body}"),
            Err(_) => break,
        }
    }

    let slow_result = timeout(Duration::from_secs(30), slow).await???;
    assert_eq!(slow_result.0.as_u16(), 200, "/slow should complete cleanly");
    // Require MORE than one: a serialized store could let exactly one fast call
    // complete at the instant the slow call resolves (it queued behind it), which
    // a `>= 1` check would wrongly accept. Several completing mid-flight proves
    // the store spawns one task per call and interleaves them.
    assert!(
        served_during_slow >= 2,
        "multiple fast calls must complete while a slow call is in flight (served \
         {served_during_slow}); a serialized store would block them until the slow call returned"
    );
    Ok(())
}

/// Poll `/progress?name={name}` (addressed to `host`) until it reaches at least
/// `target`, or time out. Returns the last value seen.
async fn wait_progress(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    host: &str,
    name: &str,
    target: u64,
) -> Result<u64> {
    let mut last = 0u64;
    for _ in 0..200 {
        let (s, body) = req(client, addr, host, &format!("/progress?name={name}")).await?;
        if s.as_u16() == 200 {
            last = body.parse().unwrap_or(0);
            if last >= target {
                return Ok(last);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(last)
}

/// Cancellation baseline: an uncancelled `begin` runs to completion — it reaches
/// every tick and `progress` reports the full count. Establishes that the
/// long-running op is well-behaved before the cancel cases truncate it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_begin_runs_to_completion() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("cbeg")).await?;
    let client = reqwest::Client::new();

    let (s, body) = req(&client, &addr, "cbeg", "/begin?name=b1&ticks=4&tick-ms=10").await?;
    assert_eq!(s.as_u16(), 200, "an uncancelled begin should complete");
    assert_eq!(body, "4", "begin reaches every tick when not cancelled");
    let (_, p) = req(&client, &addr, "cbeg", "/progress?name=b1").await?;
    assert_eq!(p, "4", "progress records the full run");
    Ok(())
}

/// Per-invocation cooperative cancellation: a same-workload caller cancels a
/// long-running `begin` mid-flight. `cancel-job` marks the job, the guest observes
/// it on its next tick and returns early — cleanly, with a partial count (a normal
/// 200, not a trap) — while the plugin store keeps serving. Only that one
/// invocation is affected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_cancel_truncates_one_invocation() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("cc")).await?;
    let client = reqwest::Client::new();

    // Long begin (~100 * 30ms ≈ 3s) on workload "cc".
    let begin = tokio::spawn({
        let client = client.clone();
        async move { req(&client, &addr, "cc", "/begin?name=foo&ticks=100&tick-ms=30").await }
    });

    // Once it is clearly running, cancel it from the SAME workload (authorized).
    let seen = wait_progress(&client, &addr, "cc", "foo", 2).await?;
    assert!(
        seen >= 2,
        "begin should be making progress before cancel (saw {seen})"
    );
    let (s, body) = req(&client, &addr, "cc", "/cancel-job?name=foo").await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(body, "true", "a same-workload caller may cancel the job");

    // The guest cooperatively returns early with a partial count — a normal
    // response, no trap.
    let (s, body) = timeout(Duration::from_secs(20), begin).await???;
    assert_eq!(
        s.as_u16(),
        200,
        "a cooperatively-cancelled begin still returns normally"
    );
    let reached: u64 = body.parse().unwrap_or(u64::MAX);
    assert!(
        (1..100).contains(&reached),
        "cancel truncated begin mid-run (reached {reached}, expected 1..100)"
    );

    // The plugin store is unharmed: it keeps serving.
    let (s, _) = req(&client, &addr, "cc", "/set?key=alive&value=yes").await?;
    assert!(
        s.is_success(),
        "the plugin store keeps serving after a cancel"
    );
    let (_, v) = req(&client, &addr, "cc", "/get?key=alive").await?;
    assert_eq!(v, "yes");
    Ok(())
}

/// Cancellation is tenant-isolated: a *different* workload cannot cancel another's
/// invocation. The cross-workload `cancel-job` returns false (unauthorized) and
/// the target `begin` runs to completion undisturbed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_component_plugin_cancel_is_tenant_isolated() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("iso-a")).await?;
    h.workload_start(caller_workload("iso-b")).await?;
    let client = reqwest::Client::new();

    // Moderate begin (~20 * 30ms ≈ 600ms) owned by workload "iso-a".
    let begin = tokio::spawn({
        let client = client.clone();
        async move {
            req(
                &client,
                &addr,
                "iso-a",
                "/begin?name=bar&ticks=20&tick-ms=30",
            )
            .await
        }
    });

    // A different workload tries to cancel it — must be refused.
    let seen = wait_progress(&client, &addr, "iso-b", "bar", 2).await?;
    assert!(
        seen >= 2,
        "begin should be running before the cancel attempt (saw {seen})"
    );
    let (s, body) = req(&client, &addr, "iso-b", "/cancel-job?name=bar").await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(
        body, "false",
        "a different workload must not be able to cancel the job"
    );

    // The invocation, undisturbed, runs to completion.
    let (s, body) = timeout(Duration::from_secs(20), begin).await???;
    assert_eq!(s.as_u16(), 200, "the uncancelled begin should complete");
    assert_eq!(
        body, "20",
        "an unauthorized cancel does not truncate the run"
    );
    Ok(())
}

/// Cancellation racing completion: cancelling AFTER the target invocation has
/// completed is refused — the job was retired with its task, so the stale job id
/// no longer authorizes anything (and cannot alias a newer job: ids are never
/// reused within an incarnation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_component_plugin_cancel_after_completion_is_refused() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin_by_host("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    h.workload_start(caller_workload("late")).await?;
    let client = reqwest::Client::new();

    // Run a short begin to completion; its job retires as the call returns.
    let (s, body) = req(
        &client,
        &addr,
        "late",
        "/begin?name=done&ticks=2&tick-ms=10",
    )
    .await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(body, "2", "the begin ran to completion");

    // The plugin still remembers the (now stale) job id under this name; the
    // cancel request must be refused, even from the owning workload.
    let (s, body) = req(&client, &addr, "late", "/cancel-job?name=done").await?;
    assert_eq!(s.as_u16(), 200);
    assert_eq!(
        body, "false",
        "cancelling a completed (retired) job must be refused"
    );
    Ok(())
}

/// E (identity): both halves of the ambient caller identity — workload id AND
/// component id — resolve end-to-end through the `wasmcloud:host/identity`
/// import while the call is in flight.
#[tokio::test]
async fn test_component_plugin_caller_identity_both_halves() -> Result<()> {
    let (addr, h) =
        start_host_with_component_plugin("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM).await?;
    let request = caller_workload("who");
    let workload_id = request.workload_id.clone();
    h.workload_start(request).await?;
    let client = reqwest::Client::new();

    let (s, body) = req(&client, &addr, "who", "/whoami").await?;
    assert_eq!(s.as_u16(), 200);
    let (seen_workload, seen_component) = body
        .split_once('|')
        .with_context(|| format!("whoami returns 'workload|component', got {body}"))?;
    assert_eq!(
        seen_workload, workload_id,
        "the plugin must see the calling workload's id"
    );
    assert!(
        !seen_component.is_empty(),
        "the plugin must see a non-empty calling component id (empty means the \
         identity lookup fell back to 'unknown caller')"
    );
    Ok(())
}

/// Supervision budget: with a budget of zero restarts, the first fault kills the
/// plugin for good — subsequent capability calls fail promptly with a clear
/// error (the plugin is not running) instead of hanging or queueing forever, and
/// stay failing (no zombie incarnation comes back).
#[tokio::test]
async fn test_component_plugin_restart_budget_exhaustion() -> Result<()> {
    let host = "kv-budget";
    let (addr, h) =
        start_host_with_component_plugin_max_restarts("127.0.0.1:0", PLUGIN_ID, KV_PLUGIN_WASM, 0)
            .await?;
    h.workload_start(caller_workload(host)).await?;
    let client = reqwest::Client::new();

    // Healthy before the fault.
    let (s, _) = req(&client, &addr, host, "/set?key=pre&value=1").await?;
    assert!(s.is_success(), "plugin serves before the fault");

    // One trap exhausts the zero-restart budget.
    let (s, _) = req(&client, &addr, host, "/boom").await?;
    assert!(s.is_server_error(), "the trapping call fails, got {s}");

    // Once the supervisor gives up it clears the capability channel, so every
    // later call errors promptly. Poll past the brief window where the dying
    // incarnation's channel still accepts sends.
    let mut dead = false;
    for _ in 0..50 {
        let (s, _) = req(&client, &addr, host, "/get?key=pre").await?;
        if s.is_server_error() {
            dead = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        dead,
        "past its restart budget the plugin must fail calls, not serve or hang"
    );

    // And it STAYS dead — no delayed restart sneaks back.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (s, _) = req(&client, &addr, host, "/get?key=pre").await?;
    assert!(
        s.is_server_error(),
        "a plugin past its restart budget must not come back, got {s}"
    );
    Ok(())
}
