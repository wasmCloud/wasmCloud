#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end ABA test for async `wasmcloud:keyvalue` compare-and-swap through a
//! **real P3 guest**, routed via `(implements ..)`.
//!
//! The `keyvalue-implements-p3` fixture opens a bucket through the labeled `kv`
//! (`store`) import, then `set k=A`, `current(k) -> v1`, `set k=B`, `set k=A`,
//! and asserts (via `store`/`cas`/`atomics`/`batch`, all on that one bucket):
//!
//! - `swap(k, C, require_version=v1)` is `stale` — the value returned to
//!   identical bytes (A → B → A), so a content-hash version would wrongly
//!   succeed; the backend's monotonic version makes it stale (ABA detected).
//! - a swap pinned to the *current* version succeeds — so the stale result is a
//!   real precondition miss, not an always-stale backend.
//! - a swap with empty `cas-options` is rejected with `invalid-argument`.
//! - `atomics.increment` and `batch` get/set/delete-many round-trip correctly.
//!
//! The guest answers `ok` only when all hold.
//!
//! This exercises the full async/concurrent host ABI through a guest — the
//! `store.open` label routing, the shared `types.bucket` resource, and the
//! standalone `cas` `current`/`swap` host methods calling the backend's atomic
//! compare-and-set — against the in-memory backend, so it needs no Docker.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_keyvalue::{InMemoryProvider, MultiplexedAsyncKeyValue},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_IMPLEMENTS_P3_WASM: &[u8] = include_bytes!("wasm/keyvalue_implements_p3.wasm");

/// The labeled `store` import (`kv`), routed to an in-memory backend. `cas` is
/// bound standalone and needs no declaration — it operates on the bucket the
/// guest opens through this label.
fn kv_store_iface(name: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::from([("backend".to_string(), "in-memory".to_string())]),
        name: Some(name.to_string()),
    }
}

#[tokio::test]
async fn p3_guest_cas_swap_detects_aba() -> Result<()> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(
            MultiplexedAsyncKeyValue::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    let host_interfaces = vec![
        http_incoming_handler_interface("kv-impl", None),
        kv_store_iface("kv"),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-implements-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-implements-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_IMPLEMENTS_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs `add_to_linker` for the named `store` plus standalone
    // `types`/`cas`. Assert the workload actually reached Running (a labeled
    // store + cas guest is exactly the case the WIT restructure enables).
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "workload should resolve (labeled store + standalone cas): {}",
        resp.workload_status.message
    );

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-impl")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "ok",
        "guest must observe: ABA swap stale, current-version swap succeeds, empty cas-options rejected"
    );

    Ok(())
}
