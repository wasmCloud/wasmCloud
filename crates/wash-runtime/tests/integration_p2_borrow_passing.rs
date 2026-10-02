//! Lends a guest resource (`borrow<token>`) across the P2 dynamic linker
//! through two sibling components. One HTTP request drives a single `token`
//! through every hop:
//!
//! ```text
//! request -> caller --Token::new--> producer                 (producer owns the token)
//!            caller --borrow--> middleware --borrow--> consumer --greet()--> producer
//!            caller --{record, tuple, option}--> middleware --same--> consumer --greet()x3--> producer
//!            caller --tuple<string, own>--> middleware --same--> consumer --greet()--> producer
//!            caller --greet()--> producer
//! ```
//!
//! Every borrow that crosses the linker opens a slot in wasmtime's host table
//! that must close before the import call returns, or wasmtime traps the caller
//! with "borrow handles still remain at the end of the call". A handle nested in
//! a compound param must cross by identity too. The response body proves it was
//! one resource throughout: every greeting reads `hello world` and the producer
//! counted exactly five `greet` calls.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_only_host_interfaces, start_host_with_dev_router};

const BORROW_CALLER_P2_WASM: &[u8] = include_bytes!("wasm/borrow_caller_p2.wasm");
const BORROW_MIDDLEWARE_P2_WASM: &[u8] = include_bytes!("wasm/borrow_middleware_p2.wasm");
const BORROW_CONSUMER_P2_WASM: &[u8] = include_bytes!("wasm/borrow_consumer_p2.wasm");
const BORROW_PRODUCER_P2_WASM: &[u8] = include_bytes!("wasm/borrow_producer_p2.wasm");

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
async fn test_p2_borrow_survives_two_linker_hops() -> Result<()> {
    let (addr, host) = start_host_with_dev_router("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p2-borrow-passing".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("borrow-caller", BORROW_CALLER_P2_WASM),
                component("borrow-middleware", BORROW_MIDDLEWARE_P2_WASM),
                component("borrow-consumer", BORROW_CONSUMER_P2_WASM),
                component("borrow-producer", BORROW_PRODUCER_P2_WASM),
            ],
            host_interfaces: http_only_host_interfaces("p2-borrow"),
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
            .header("HOST", "p2-borrow")
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
         a 5xx here with an empty body is the caller trapping because a nested \
         handle did not cross by identity or a lowered borrow was never released"
    );

    // middleware wraps consumer, consumer greets through the producer: once for
    // the plain borrow, three times for the borrows nested in a record, a tuple
    // and an option, and once on a second token it took ownership of inside a
    // tuple. The caller then greets once more itself, so the producer saw five
    // greet calls on the first token.
    assert_eq!(
        body,
        "middleware:consumer:hello world\
         |middleware:consumer:nested:pair:hello world,hello world,hello world\
         |middleware:consumer:adopted:hello owned\
         |hello world|greets=5"
    );

    Ok(())
}
