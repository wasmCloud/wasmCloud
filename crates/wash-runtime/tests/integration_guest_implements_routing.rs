#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end `(implements ..)` routing between *guests*, with no host plugin
//! anywhere in the path.
//!
//! `integration_keyvalue_implements.rs` proves a labelled import routes to the
//! right **host** backend. This proves the other half: two sibling components
//! in one workload export the same interface
//! (`example:feeds/reader@0.1.0`), and a third imports it **twice** under
//! the component-model labels `feed-a` and `feed-b`, which are the manifest
//! names of those two siblings. Each label must link to the sibling it names.
//!
//! Each sibling's `source` returns its own component name, so the caller's
//! response body (`"<a>|<b>"`) says exactly which component each labelled
//! import reached. Anything other than `feed-a|feed-b` is a mis-route; a
//! workload that never reaches Running means the labelled import was dropped
//! and wasmtime found no implementation in the linker.
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
        http::{DevRouter, Ingress},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::http_incoming_handler_interface;

const CALLEE_A_WASM: &[u8] = include_bytes!("wasm/feeds_callee_a.wasm");
const CALLEE_B_WASM: &[u8] = include_bytes!("wasm/feeds_callee_b.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/feeds_caller.wasm");

fn component(name: &str, bytes: &'static [u8]) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn labelled_imports_route_to_the_sibling_they_name() -> Result<()> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    // Only HTTP ingress is host-provided. `example:feeds/reader` has no
    // plugin at all, so if the two labelled imports do not link to the sibling
    // components there is nothing else for them to fall back to.
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "feeds".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                // The component NAMES here are what the caller's `(implements ..)`
                // labels name.
                component("feed-a", CALLEE_A_WASM),
                component("feed-b", CALLEE_B_WASM),
                component("caller", CALLER_WASM),
            ],
            host_interfaces: vec![http_incoming_handler_interface("feeds", None)],
            volumes: vec![],
        },
    };

    // `workload_start` returns Ok even when resolution fails (the failure is in
    // the status), so assert the workload actually reached Running.
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

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "feeds")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "feed-a|feed-b",
        "each labelled import must reach the sibling component its label names"
    );

    Ok(())
}
