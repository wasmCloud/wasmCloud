//! Integration test for passing a guest resource handle across the WASIP3
//! dynamic linker.
//!
//! Three P3 components are linked together in one workload, and a single HTTP
//! request drives a resource handle through both linker hops:
//!
//! 1. A request hits `res-caller-p3`, the workload's HTTP entrypoint.
//! 2. `res-caller-p3` calls `res-producer-p3` (hop 1) and gets back a `token`
//!    resource, created for the string `"world"`.
//! 3. `res-caller-p3` passes that same `token` to `res-sink-p3` (hop 2) via
//!    `accept(token)`. The handle crosses the linker as a host `ResourceAny`.
//! 4. `res-sink-p3` calls `greet()` on the token (`"hello world"`) and wraps it
//!    as `"sink:hello world"`, which `res-caller-p3` returns as the body.
//!
//! What this exercises:
//!
//! - Each linker hop lowers the `token` via `engine::value::lower_with_type`,
//!   which must pass the resource by *identity* rather than re-lowering
//!   (copying) it.
//! - If identity were not preserved, `res-sink-p3` would receive a different /
//!   stale resource and its `greet()` call would fail or read the wrong token,
//!   so the body would not come back as `"sink:hello world"`.
//!
//! Asserting the exact `"sink:hello world"` body therefore proves the one
//! `token` resource survived both hops across the dynamic linker intact.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3};

const RES_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/res_caller_p3.wasm");
const RES_PRODUCER_P3_WASM: &[u8] = include_bytes!("wasm/res_producer_p3.wasm");
const RES_SINK_P3_WASM: &[u8] = include_bytes!("wasm/res_sink_p3.wasm");

fn component(name: &str, bytes: &'static [u8]) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
    }
}

#[tokio::test]
async fn test_p3_resource_handle_crosses_linker() -> Result<()> {
    let (addr, host) = start_host_with_p3("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-resource-passing".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("res-caller", RES_CALLER_P3_WASM),
                component("res-producer", RES_PRODUCER_P3_WASM),
                component("res-sink", RES_SINK_P3_WASM),
            ],
            host_interfaces: http_only_host_interfaces("p3-resource"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("resource-passing workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-resource")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    assert!(
        response.status().is_success(),
        "resource-passing handler should return 2xx, got {}",
        response.status()
    );

    // producer makes a token for "world"; sink greets it -> "hello world",
    // wrapped as "sink:hello world". Getting this back proves the same token
    // resource survived the trip across the linker into the sink.
    let body = response.text().await?;
    assert_eq!(body, "sink:hello world");

    Ok(())
}
