#![cfg(feature = "wasm_component_model_implements")]
//! A **plain** (unlabeled) async `wasmcloud:keyvalue@0.1.0` `store` import, served
//! by a default backend through a **real P3 guest** — no `(implements ..)` label.
//!
//! The `keyvalue-default-p3` fixture opens a bucket via `wasmcloud:keyvalue/store`
//! plainly. On each request it sets a key, increments a counter through the
//! standalone `atomics` import, reads the value back, and returns it.
//!
//! The host binds a single [`MultiplexedAsyncKeyValue`] with the in-memory
//! provider and declares an **unnamed** keyvalue interface (the default route). A
//! 200 whose body echoes the written value proves that a component gets a working
//! backend from a plain `store` import — no label needed — exercising the standard
//! (non-`(implements)`) binding to the workload's default backend.
//!
//! Like the blobstore-default counterpart, this uses the in-memory backend, so it
//! needs no external services and is not `#[ignore]`d.
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

const KEYVALUE_DEFAULT_P3_WASM: &[u8] = include_bytes!("wasm/keyvalue_default_p3.wasm");

/// An UNNAMED async `wasmcloud:keyvalue` interface — no `(implements ..)` label,
/// no backend override, so it is the workload's default route and falls back to
/// the in-memory backend.
fn default_kv_iface() -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

#[tokio::test]
async fn p3_guest_plain_keyvalue_uses_default_backend() -> Result<()> {
    // --- host with the async multiplexed keyvalue plugin + in-memory provider ---
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

    // The guest imports `store` *plainly* (no label); the unnamed interface is the
    // default route, which the plugin binds via the standard host interface.
    let host_interfaces = vec![
        http_incoming_handler_interface("kv-default", None),
        default_kv_iface(),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-default-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-default-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_DEFAULT_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs the *standard* `store::add_to_linker` (not the named one) for
    // the plain import. `workload_start` returns Ok even on resolution failure (it
    // encodes the error in the status), so assert it reached Running.
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "a plain (unlabeled) keyvalue import should resolve to a default backend: {}",
        resp.workload_status.message
    );

    // Drive the guest: open a bucket, set a key, increment a counter, read the
    // value back, and echo it. A matching body proves the plain import reached a
    // working default backend — with no label.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-default")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "woof from a plain p3 keyvalue guest",
        "the guest must round-trip the value through the default (unlabeled) backend"
    );

    Ok(())
}
