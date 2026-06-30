#![cfg(feature = "wasm_component_model_implements")]
//! Coexistence of the standalone, sync-multiplexed, and **async-multiplexed**
//! keyvalue plugins on a single host.
//!
//! The production host (see `wash`'s `host`/`dev`) registers all three on one
//! builder:
//!   * [`InMemoryKeyValue`] — standalone `wasi:keyvalue`, serves unnamed imports;
//!   * [`MultiplexedKeyValue`] — `(implements ..)` `wasi:keyvalue`, serves named;
//!   * [`MultiplexedAsyncKeyValue`] — `(implements ..)` async `wasmcloud:keyvalue`.
//!
//! Existing tests cover the standalone + sync-multiplexed pair. This adds the
//! async plugin to the mix and runs two workloads on the one host:
//!   * a sync `wasi:keyvalue` guest (`keyvalue-counter`) — unnamed (→ standalone)
//!     plus a named interface (→ sync multiplexed);
//!   * the async `wasmcloud:keyvalue` guest (`keyvalue-implements-p3`) —
//!     labeled `store` plus standalone `atomics`/`cas`/`batch`.
//!
//! Both reaching `Running` is the coexistence proof: the three plugins'
//! `add_to_linker` registrations don't conflict on one host. The async guest is
//! then driven over HTTP (store/cas/atomics/batch → `ok`) to confirm the async
//! path works alongside the sync plugins. (The `DevRouter` is single-component,
//! so it can't HTTP-route both workloads; the sync counter's standalone routing
//! is covered by `integration_keyvalue_coexistence`.) In-memory, so no Docker.
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
    plugin::wasi_keyvalue::{
        InMemoryKeyValue, InMemoryProvider, MultiplexedAsyncKeyValue, MultiplexedKeyValue,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_COUNTER_WASM: &[u8] = include_bytes!("wasm/keyvalue_counter.wasm");
const KEYVALUE_IMPLEMENTS_P3_WASM: &[u8] = include_bytes!("wasm/keyvalue_implements_p3.wasm");

/// A `wasi:keyvalue@0.2.0-draft` interface (store+atomics). `name = None` is the
/// unnamed import (→ standalone); a name routes to the sync multiplexed plugin.
fn wasi_kv_iface(name: Option<&str>) -> WitInterface {
    let mut config = HashMap::new();
    if name.is_some() {
        config.insert("backend".to_string(), "in-memory".to_string());
    }
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config,
        name: name.map(String::from),
    }
}

/// The labeled async `wasmcloud:keyvalue/store` import (→ async multiplexed).
fn wasmcloud_kv_store(name: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::from([("backend".to_string(), "in-memory".to_string())]),
        name: Some(name.to_string()),
    }
}

fn workload(
    name: &str,
    component: &str,
    wasm: &'static [u8],
    host_interfaces: Vec<WitInterface>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: component.to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    }
}

#[tokio::test]
async fn standalone_sync_and_async_keyvalue_coexist() -> Result<()> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(
            MultiplexedKeyValue::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .with_plugin(Arc::new(
            MultiplexedAsyncKeyValue::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // Sync wasi:keyvalue workload: unnamed (→ standalone) + named (→ sync mux).
    let sync = workload(
        "kv-sync",
        "keyvalue-counter.wasm",
        KEYVALUE_COUNTER_WASM,
        vec![
            http_incoming_handler_interface("kv-sync", None),
            wasi_kv_iface(None),
            wasi_kv_iface(Some("cache")),
        ],
    );
    let resp = host
        .workload_start(sync)
        .await
        .context("sync keyvalue workload_start failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "standalone + sync-multiplexed keyvalue should run: {}",
        resp.workload_status.message
    );

    // Async wasmcloud:keyvalue workload on the SAME host.
    let async_ = workload(
        "kv-async",
        "keyvalue-implements-p3.wasm",
        KEYVALUE_IMPLEMENTS_P3_WASM,
        vec![
            http_incoming_handler_interface("kv-async", None),
            wasmcloud_kv_store("kv"),
        ],
    );
    let resp = host
        .workload_start(async_)
        .await
        .context("async keyvalue workload_start failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "async-multiplexed keyvalue should run alongside the sync plugins: {}",
        resp.workload_status.message
    );

    // Both workloads reaching `Running` above is the coexistence proof: the
    // standalone, sync-multiplexed, and async-multiplexed plugins all ran their
    // `add_to_linker` on this one host without conflict. The sync counter's
    // standalone routing is covered by `integration_keyvalue_coexistence`; here
    // the `DevRouter` is single-component (it can't route two HTTP workloads by
    // HOST header), so we drive the async guest — last-started, so it is the one
    // served — to confirm the async path works end-to-end alongside the sync
    // plugins.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-async")
            .send(),
    )
    .await
    .context("async request timed out")?
    .context("async request failed")?;
    let status = response.status();
    let body = response.text().await?;
    assert!(
        status.is_success(),
        "async expected 200, got {status}: {body}"
    );
    assert_eq!(
        body, "ok",
        "async wasmcloud:keyvalue guest (store/cas/atomics/batch) must work alongside the sync plugins"
    );

    Ok(())
}
