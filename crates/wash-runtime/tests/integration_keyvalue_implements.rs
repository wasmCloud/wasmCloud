#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end `(implements ..)` named-import routing through a *real* guest.
//!
//! The `keyvalue-implements` fixture imports `wasi:keyvalue/store` **twice**
//! under the component-model labels `team-a` and `team-b` (the WIT `implements`
//! clause — `import team-a: wasi:keyvalue/store@…;`). On each request it opens a
//! bucket through each label, writes a distinct value to the same key, and reads
//! both back; it answers `isolated` only if neither write is visible through the
//! other label.
//!
//! The host binds a single [`MultiplexedKeyValue`] plugin. The workload declares
//! the two named interfaces `team-a` and `team-b`, each routed to an isolated
//! in-memory backend. The plugin's named-imports `add_to_linker` resolves each
//! label to its own backend, so the two stores must stay disjoint — proving the
//! implements id threads through `open` *and* the bucket resource methods
//! (`set`/`get`) to the correct backend.
//!
//! Unlike `integration_keyvalue_coexistence.rs` (where the named side is
//! declared host-side because the guest imported unnamed), here the *guest
//! itself* emits the `(implements ..)` imports.
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
    plugin::wasi_keyvalue::{InMemoryProvider, MultiplexedKeyValue},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_IMPLEMENTS_WASM: &[u8] = include_bytes!("wasm/keyvalue_implements.wasm");

const KV_VERSION: &str = "0.2.0-draft";

/// A named `wasi:keyvalue/store` interface routed to an in-memory backend. The
/// `name` becomes the implements label the guest imports under (`team-a` /
/// `team-b`); `resolve` matches it against the component's import label.
fn named_store(name: &str) -> WitInterface {
    let mut config = HashMap::new();
    config.insert("backend".to_string(), "in-memory".to_string());
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse(KV_VERSION).unwrap()),
        config,
        name: Some(name.to_string()),
    }
}

#[tokio::test]
async fn implements_imports_route_to_isolated_backends() -> Result<()> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    // One multiplexed plugin serves both labeled imports; each named interface
    // defaults to its own isolated in-memory backend via `InMemoryProvider`.
    let multiplexed = MultiplexedKeyValue::new().with_provider(Arc::new(InMemoryProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(multiplexed))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // The two named interfaces map to the guest's `team-a` / `team-b` implements
    // imports; each routes to a distinct in-memory backend.
    let host_interfaces = vec![
        http_incoming_handler_interface("kv-implements", None),
        named_store("team-a"),
        named_store("team-b"),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-implements".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-implements.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_IMPLEMENTS_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs the named-imports `add_to_linker`, resolving `team-a`/`team-b`
    // against the declared interfaces. Success proves both labeled imports of the
    // same external interface bound on one linker. `workload_start` returns Ok
    // even on resolution failure (it encodes the error in the status), so assert
    // the workload actually reached Running.
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        wash_runtime::types::WorkloadState::Running,
        "workload should resolve: {}",
        resp.workload_status.message
    );

    // The guest writes `from-a` via team-a and `from-b` via team-b to the same
    // key, then reads both back. `isolated` means neither write leaked across the
    // label boundary — i.e. the implements id routed `open` and the bucket
    // resource methods to the correct backend.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-implements")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "isolated",
        "each implements import must route to its own backend (no cross-label leak)"
    );

    Ok(())
}
