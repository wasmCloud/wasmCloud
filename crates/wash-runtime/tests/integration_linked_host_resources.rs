//! Hands host resources to a linked component over sync calls.
//!
//! A sync linked call is served from a store of the callee's own, and a host
//! resource is an entry in one store's table. So the object behind each handle
//! is moved from the caller's table to the callee's, and back again when it
//! was only lent. Both fixtures are P2, where every call is sync.
//!
//! The streams here are the request's own body and the response's, so each
//! test reads what a client sent or sees what it was sent back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_dev_router};

const CALLER_WASM: &[u8] = include_bytes!("wasm/lend_caller_p2.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/lend_callee_p2.wasm");

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

/// Start the caller and the callee as one workload, post `body` to the
/// caller's `route`, and return what it answers.
async fn post(route: &str, body: &'static str) -> Result<String> {
    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;
    let request = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "linked-host-resources".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("lend-caller", CALLER_WASM),
                component("lend-callee", CALLEE_WASM),
            ],
            host_interfaces: http_only_host_interfaces("lend"),
            volumes: vec![],
        },
    };
    // A workload that cannot be linked still answers `Ok`, with its state.
    let started = host.workload_start(request).await?.workload_status;
    anyhow::ensure!(
        started.workload_state == WorkloadState::Running,
        "the workload should start: {}",
        started.message
    );

    let response = timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .post(format!("http://{addr}{route}"))
            .header("HOST", "lend")
            .header("x-probe", "from-the-client")
            .body(body)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let answer = response.text().await?;
    anyhow::ensure!(status.is_success(), "{route} answered {status}: {answer:?}");
    Ok(answer)
}

/// The request body's stream is lent to the callee, read by the caller again,
/// given to the callee and handed back, then given away for good: each reads
/// on from where the last stopped. The response body's stream is lent, written
/// to by the caller, then given away, and the client receives all three writes
/// in order.
#[tokio::test]
async fn streams_are_lent_and_given_to_a_linked_component() -> Result<()> {
    let answer = post("/streams", "hello world!").await?;
    assert_eq!(answer, "hello| |wor|ld!|caller|callee");
    Ok(())
}

/// Header fields are lent to the callee, once and then as both arguments of
/// one call, and still read by the caller after. Then the whole request is
/// given away and the callee reads its body.
#[tokio::test]
async fn a_request_is_given_to_a_linked_component() -> Result<()> {
    let answer = post("/request", "the body").await?;
    assert_eq!(
        answer,
        "from-the-client|from-the-client+from-the-client|from-the-client|POST /request the body"
    );
    Ok(())
}
