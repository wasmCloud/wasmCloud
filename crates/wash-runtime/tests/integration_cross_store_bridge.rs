//! Integration tests for the cross-store host bridge: a long-lived p3 service
//! (HTTP ingress + cli/run) calling a stateless `wasmcloud:bridge/ops` backend
//! that runs in a separate store, instantiated fresh per call.
//!
//! Covers the test-plan P0 capabilities:
//!   #2 async inter-component linking (`func_new_concurrent`) — `/add`
//!   #3 statelessness / ephemeral instances                  — `/bump`
//!   #5 stream relocation (both directions, element type, nested) — the rest
//!
//! Each endpoint returns `{"result":N,"expected":M}`; the test asserts the
//! backend's computed result reached the service through the bridge unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const BRIDGE_SERVICE_WASM: &[u8] = include_bytes!("wasm/bridge_service.wasm");
const BRIDGE_BACKEND_WASM: &[u8] = include_bytes!("wasm/bridge_backend.wasm");

fn bridge_workload(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(BRIDGE_SERVICE_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
            }),
            // The backend is a stateless component linked to the service by its
            // `wasmcloud:bridge/ops` export; the host instantiates it fresh per
            // call in its own store.
            components: vec![Component {
                name: "bridge-backend".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(BRIDGE_BACKEND_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 1000,
            }],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// GET `path` and return the parsed `(result, expected)` pair from the JSON body.
async fn call(
    client: &reqwest::Client,
    addr: &std::net::SocketAddr,
    host: &str,
    path: &str,
) -> Result<(u64, u64)> {
    let resp = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", host)
            .send(),
    )
    .await
    .context("request timed out")??;
    anyhow::ensure!(
        resp.status().is_success(),
        "{path} should succeed, got {}",
        resp.status()
    );
    let body = resp.text().await?;
    let field = |name: &str| -> u64 {
        let key = format!("\"{name}\":");
        let start = body.find(&key).expect("field present") + key.len();
        let rest = &body[start..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().expect("numeric field")
    };
    Ok((field("result"), field("expected")))
}

/// #2: a handle-free `async func` import is linked via `func_new_concurrent` and
/// called across the bridge, returning the backend's computed result.
#[tokio::test]
async fn test_bridge_async_call_primitives() -> Result<()> {
    let host = "bridge-add";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    let (result, expected) = call(&client, &addr, host, "/add").await?;
    assert_eq!(
        result, expected,
        "ops.add(40,2) should reach the service as 42"
    );
    assert_eq!(result, 42);
    Ok(())
}

/// A `future<u64>` returned by the backend is relocated across the store
/// boundary (result direction) and resolves to the backend's value in the
/// service store — the ephemeral store is kept alive until the future resolves.
#[tokio::test]
async fn test_bridge_future_relocation() -> Result<()> {
    let host = "bridge-future";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    let (result, expected) = call(&client, &addr, host, "/delayed").await?;
    assert_eq!(
        result, expected,
        "backend future<u64> should reach the service"
    );
    assert_eq!(result, 12345);
    Ok(())
}

/// The service stays responsive after a client cancels a request mid-flight
/// (drops the connection while a streaming backend call is in progress).
/// Exercises the service_http disconnect path — the response `oneshot` failing
/// and the frame-forward loop breaking — without wedging the shared service
/// instance or leaking it into later requests.
#[tokio::test]
async fn test_service_survives_client_cancellation() -> Result<()> {
    let host = "bridge-cancel";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;

    // Fire a streaming request, then abort it before it can complete.
    let cancelled = tokio::spawn(async move {
        let client = reqwest::Client::new();
        client
            .get(format!("http://{addr}/consume-large"))
            .header("HOST", host)
            .send()
            .await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    cancelled.abort();
    let _ = cancelled.await;

    // The service must still serve subsequent requests.
    let client = reqwest::Client::new();
    let (result, _) = call(&client, &addr, host, "/add").await?;
    assert_eq!(
        result, 42,
        "service stays responsive after a client cancels a request mid-flight"
    );
    Ok(())
}

/// #3: every call instantiates a fresh backend in its own store, so the
/// backend's process-global counter never persists — `bump()` is always 1, even
/// across repeated calls, while the service instance itself stays long-lived.
#[tokio::test]
async fn test_bridge_backend_is_stateless() -> Result<()> {
    let host = "bridge-bump";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    for _ in 0..4 {
        let (result, _) = call(&client, &addr, host, "/bump").await?;
        assert_eq!(
            result, 1,
            "a fresh stateless backend instance handles each call, so bump() is always 1"
        );
    }
    Ok(())
}

/// #5: stream relocation across the store boundary in every shape — service ->
/// component (`/consume`), component -> service (`/produce`), a non-`u8` element
/// type (`/sum`), and a stream nested inside a variant payload (`/relay`).
#[tokio::test]
async fn test_bridge_stream_relocation() -> Result<()> {
    let host = "bridge-stream";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    for path in ["/consume", "/produce", "/sum", "/relay"] {
        let (result, expected) = call(&client, &addr, host, path).await?;
        assert_eq!(
            result, expected,
            "{path}: stream should relocate across the store boundary intact (got {result}, want {expected})"
        );
    }
    Ok(())
}

/// #3: while a backend call busy-spins (monopolizing its own ephemeral store),
/// the long-lived service keeps serving other HTTP requests. If the backend
/// shared the service's store, the spin would freeze the service's HTTP ingress
/// and no concurrent request could complete until the spin finished — this test
/// would then observe zero requests served during the block.
///
/// Needs a multi-threaded runtime: a single worker thread would be consumed by
/// the spin regardless of the store split, masking the property under test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_bridge_isolation_under_blocking_backend() -> Result<()> {
    let host = "bridge-block";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    // Kick off a long blocking backend call in the background.
    let block = tokio::spawn({
        let client = client.clone();
        let host = host.to_string();
        async move { call(&client, &addr, &host, "/block").await }
    });

    // While that call spins in the backend's store, the service must remain
    // responsive. Count `/add` requests that complete before the block returns.
    let mut served_during_block = 0u32;
    while !block.is_finished() && served_during_block < 5 {
        match call(&client, &addr, host, "/add").await {
            Ok((42, _)) => served_during_block += 1,
            Ok((other, _)) => anyhow::bail!("unexpected /add result {other}"),
            Err(_) => break,
        }
    }

    let block_result = timeout(Duration::from_secs(30), block).await???;
    assert_eq!(block_result.0, 1, "/block should complete cleanly");
    assert!(
        served_during_block >= 1,
        "the service must serve requests while a backend call blocks its own \
         store (served {served_during_block}); a shared store would freeze ingress"
    );
    Ok(())
}

/// #5 backpressure: ~4 MiB streamed through the bounded relocation channel (only
/// a handful of chunks of capacity) must complete with the exact byte count —
/// proving the stream is pumped incrementally with the channel back-pressuring
/// the writer, not buffered whole.
#[tokio::test]
async fn test_bridge_stream_backpressure_large() -> Result<()> {
    let host = "bridge-large";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    let (result, expected) = call(&client, &addr, host, "/consume-large").await?;
    assert_eq!(expected, 4 * 1024 * 1024, "fixture streams 4 MiB");
    assert_eq!(
        result, expected,
        "every byte of the large stream must arrive"
    );
    Ok(())
}

/// #5 early close: the service reads only the first chunk of a large
/// component-produced stream and drops its reader. The backend's writer side
/// must unwind cleanly — the request completes (no trap) having delivered a
/// partial, non-empty prefix.
#[tokio::test]
async fn test_bridge_reader_drops_early() -> Result<()> {
    let host = "bridge-drop";
    let (addr, h) = start_host_with_p3_http_handler("127.0.0.1:0").await?;
    h.workload_start(bridge_workload(host)).await?;
    let client = reqwest::Client::new();

    // A 200 with a parseable body already proves no trap on early reader drop.
    let (read, full) = call(&client, &addr, host, "/produce-drop-early").await?;
    assert!(
        read > 0,
        "should have read a non-empty prefix before dropping"
    );
    assert!(
        read < full,
        "should have stopped early (read {read} of {full}), not drained the whole stream"
    );
    Ok(())
}
