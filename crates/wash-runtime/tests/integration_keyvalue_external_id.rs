#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end `external-id` routing through a *real* guest.
//!
//! The `keyvalue-external-id` fixture imports `wasi:keyvalue/store` twice, under
//! the labels `users` and `catalog`, each annotated in WIT with the platform
//! name of the resource it expects. On each request it opens a bucket through
//! each import, writes a distinct value to the same key, and reads both back; it
//! answers `isolated` only if neither write is visible through the other import.
//!
//! What separates this from `integration_keyvalue_implements.rs` is what the
//! platform side says: nothing here mentions `users` or `catalog`. The two
//! bindings are keyed purely by external-id, which is the point of the attribute
//! — the operator names resources, the component names its own dependencies, and
//! neither has to learn the other's vocabulary.
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
    plugin::wasi_keyvalue::{InMemoryKeyValue, InMemoryProvider, MultiplexedKeyValue},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_EXTERNAL_ID_WASM: &[u8] = include_bytes!("wasm/keyvalue_external_id.wasm");

const KV_VERSION: &str = "0.2.0-draft";

/// A `wasi:keyvalue/store` binding keyed only by the platform resource it
/// serves. It carries no `name`, so it can only be reached by an import
/// declaring the same external-id.
fn store_for(external_id: &str) -> WitInterface {
    let mut config = HashMap::new();
    config.insert("backend".to_string(), "in-memory".to_string());
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse(KV_VERSION).unwrap()),
        config,
        name: None,
        external_id: Some(external_id.to_string()),
    }
}

fn workload(host_interfaces: Vec<WitInterface>) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-external-id".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-external-id.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_EXTERNAL_ID_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    }
}

async fn start_host(
    host_interfaces: Vec<WitInterface>,
) -> Result<(Arc<wash_runtime::host::Host>, std::net::SocketAddr, String)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    // Both keyvalue plugins, as a real `wash host` registers them: the standalone
    // one serves an unkeyed default route, the multiplexed one serves keyed
    // instances. An external-id-keyed binding must reach the multiplexed plugin
    // even though it carries no name.
    let multiplexed = MultiplexedKeyValue::new().with_provider(Arc::new(InMemoryProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(multiplexed))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // `workload_start` returns Ok even when resolution fails — the failure is in
    // the status — so callers assert on the state rather than the call.
    let resp = host
        .workload_start(workload(host_interfaces))
        .await
        .context("workload_start call failed")?;
    let state = resp.workload_status.workload_state;
    let message = resp.workload_status.message;
    if state != wash_runtime::types::WorkloadState::Running {
        return Ok((host, addr, message));
    }
    Ok((host, addr, String::new()))
}

#[tokio::test]
async fn external_id_bindings_route_to_isolated_backends() -> Result<()> {
    let (_host, addr, failure) = start_host(vec![
        http_incoming_handler_interface("kv-external-id", None),
        store_for("user-db-prod:region-a"),
        store_for("catalog-db-prod:region-a"),
    ])
    .await?;
    assert!(failure.is_empty(), "workload should resolve: {failure}");

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-external-id")
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
        "each external-id must select its own backend (no cross-resource leak)"
    );

    Ok(())
}

#[tokio::test]
async fn an_unbound_external_id_fails_the_workload_by_name() -> Result<()> {
    // Only one of the two resources is bound, and there is no default route.
    let (_host, _addr, failure) = start_host(vec![
        http_incoming_handler_interface("kv-external-id-missing", None),
        store_for("user-db-prod:region-a"),
    ])
    .await?;

    assert!(
        failure.contains("catalog-db-prod:region-a"),
        "the failure should name the unbound platform resource, got: {failure}"
    );
    Ok(())
}

#[tokio::test]
async fn an_operator_name_pin_overrides_the_declared_resource() -> Result<()> {
    // The guest's `users` import names `user-db-prod:region-a`, so that is what
    // it resolves by. A binding written against its label is the operator
    // saying otherwise — deploy-time configuration outranks what the artifact
    // declares — so the workload still binds with no entry for that resource.
    let mut pinned = store_for("unused");
    pinned.external_id = None;
    pinned.name = Some("users".to_string());

    let (_host, addr, failure) = start_host(vec![
        http_incoming_handler_interface("kv-external-id-pinned", None),
        pinned,
        store_for("catalog-db-prod:region-a"),
    ])
    .await?;
    assert!(
        failure.is_empty(),
        "a name pin should satisfy the import that names a resource: {failure}"
    );

    // The pin and the resource binding are still separate backends.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-external-id-pinned")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let body = response.text().await?;
    assert_eq!(
        body, "isolated",
        "the pinned import must keep its own backend"
    );

    Ok(())
}
