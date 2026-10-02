//! Lends a guest resource (`borrow<token>`) across the P3 dynamic linker (`call_concurrent`)
//! through two sibling components. One HTTP request drives a single `token`
//! through every hop:
//!
//! ```text
//! request -> caller --Token::new--> producer                 (producer owns the token)
//!            caller --borrow--> middleware --borrow--> consumer --greet()--> producer
//!            caller --greet()--> producer
//! ```
//!
//! Every borrow that crosses the linker opens a slot in wasmtime's host table
//! that must close before the import call returns, or wasmtime traps the caller
//! with "borrow handles still remain at the end of the call". The response body
//! proves it was one resource throughout: both greetings read `hello world` and
//! the producer counted exactly two `greet` calls.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const BORROW_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/borrow_caller_p3.wasm");
const BORROW_MIDDLEWARE_P3_WASM: &[u8] = include_bytes!("wasm/borrow_middleware_p3.wasm");
const BORROW_CONSUMER_P3_WASM: &[u8] = include_bytes!("wasm/borrow_consumer_p3.wasm");
const BORROW_PRODUCER_P3_WASM: &[u8] = include_bytes!("wasm/borrow_producer_p3.wasm");

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
async fn test_p3_borrow_survives_two_linker_hops() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-borrow-passing".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("borrow-caller", BORROW_CALLER_P3_WASM),
                component("borrow-middleware", BORROW_MIDDLEWARE_P3_WASM),
                component("borrow-consumer", BORROW_CONSUMER_P3_WASM),
                component("borrow-producer", BORROW_PRODUCER_P3_WASM),
            ],
            host_interfaces: http_only_host_interfaces("p3-borrow"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("borrow-passing workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-borrow")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(
        status.is_success(),
        "borrow-passing handler should return 2xx, got {status} (body: {body:?}); \
         a 5xx here with an empty body is the caller trapping on return from an \
         import call because a lowered borrow was never released"
    );

    // middleware wraps consumer, consumer greets through the producer; the caller then
    // greets once more itself, so the producer saw two greet calls on the one
    // token instance.
    assert_eq!(body, "middleware:consumer:hello world|hello world|greets=2");

    Ok(())
}
