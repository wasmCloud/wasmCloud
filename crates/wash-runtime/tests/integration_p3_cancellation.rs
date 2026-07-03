//! Integration tests for cancelling an in-flight P3 streaming invocation.
//!
//! Setup (two linked P3 components):
//!   - `cancellable-producer` exports `produce`, emitting `n` numbers as a
//!     `stream<u8>`, one per second.
//!   - `cancellable-component` is the HTTP entrypoint. `GET /?id=<id>`
//!     registers the invocation with the host `cancellable-jobs/control`
//!     plugin, pulls the producer's stream across the dynamic linker, and
//!     forwards it to the response body. `GET /cancel?id=<id>` asks the host
//!     to cancel the invocation registered under `<id>`.
//!
//! Cancellation works by tripping the invocation's epoch handle: the runtime
//! then traps that invocation's store mid-stream (see `engine::linked_call`'s
//! epoch-deadline callback and `plugin::cancellable_jobs`). These tests prove
//! the observable effect end-to-end:
//!   - `..._completes_without_cancel`: left alone, the client receives all ten
//!     numbers.
//!   - `..._cancel_stops_streaming_midway`: cancelling at ~5s truncates the
//!     body — the client sees only the first few numbers, never all ten.
//!
//! Requires the `epoch-interruption` feature (the plugin + epoch machinery
//! only compile under it), so the whole module is gated on it.

#![cfg(feature = "epoch-interruption")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use futures::StreamExt;
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::{http_cancellable_host_interfaces, start_host_with_p3_cancellable};

const CANCELLABLE_PRODUCER_WASM: &[u8] = include_bytes!("wasm/cancellable_producer.wasm");
const CANCELLABLE_COMPONENT_WASM: &[u8] = include_bytes!("wasm/cancellable_component.wasm");

const EXPECTED_FULL: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];

fn cancellable_workload(host_header: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: format!("p3-cancellation-{host_header}"),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "cancellable-consumer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CANCELLABLE_COMPONENT_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 4,
                    max_invocations: 100,
                },
                Component {
                    name: "cancellable-producer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CANCELLABLE_PRODUCER_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 4,
                    max_invocations: 100,
                },
            ],
            host_interfaces: http_cancellable_host_interfaces(host_header),
            volumes: vec![],
        },
    }
}

/// Drain a streaming response body to text, Returns whatever bytes
/// arrived before the stream ended.
async fn collect_body(response: reqwest::Response) -> Result<String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        match timeout(Duration::from_secs(20), stream.next()).await {
            // Stream closed cleanly.
            Ok(None) => break,
            // A chunk arrived.
            Ok(Some(Ok(chunk))) => body.extend_from_slice(&chunk),
            Ok(Some(Err(e))) => anyhow::bail!(
                "body stream closed abrubtly ({e} instead of a clean EOF - the runtime dropped the store without finishing the HTTP response"
            ),
            Err(_) => anyhow::bail!("timed out waiting for a body chunk"),
        }
    }
    String::from_utf8(body).context("streamed body was not valid utf8")
}

fn parse_numbers(body: &str) -> Vec<&str> {
    body.lines().filter(|l| !l.is_empty()).collect()
}

/// Baseline: left alone, the streaming invocation runs to completion and the
/// client receives all ten numbers.
#[tokio::test]
async fn test_p3_streaming_invocation_completes_without_cancel() -> Result<()> {
    const HOST: &str = "p3-cancel-baseline";
    let (addr, host) = start_host_with_p3_cancellable("127.0.0.1:0").await?;
    host.workload_start(cancellable_workload(HOST))
        .await
        .context("cancellable workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(20),
        client
            .get(format!("http://{addr}/?id=baseline"))
            .header("HOST", HOST)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    assert!(
        response.status().is_success(),
        "streaming handler should return 2xx, got {}",
        response.status()
    );

    let body = collect_body(response).await?;
    assert_eq!(
        parse_numbers(&body),
        EXPECTED_FULL,
        "an un-cancelled invocation should stream all ten numbers"
    );

    Ok(())
}

/// The point of the suite: cancelling the invocation ~5s in truncates the
/// stream. The client receives some numbers but never all ten.
#[tokio::test]
async fn test_p3_cancel_stops_streaming_midway() -> Result<()> {
    const HOST: &str = "p3-cancel-midway";
    const ID: &str = "midway";
    // 5 seconds correspond to 5 expected numbers (the producer delay gap is 1s)
    const DELAY_LENGTH: u64 = 5;

    let (addr, host) = start_host_with_p3_cancellable("127.0.0.1:0").await?;
    host.workload_start(cancellable_workload(HOST))
        .await
        .context("cancellable workload should start")?;

    let client = reqwest::Client::new();

    // Fire the cancel 5s into the stream, on a separate connection. By then
    // the producer has emitted the first five numbers.
    let cancel_client = client.clone();
    let cancel_response = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(DELAY_LENGTH)).await;
        cancel_client
            .get(format!("http://{addr}/cancel?id={ID}"))
            .header("HOST", HOST)
            .send()
            .await
            .and_then(|r| r.error_for_status())
    });

    let response = timeout(
        Duration::from_secs(20),
        client
            .get(format!("http://{addr}/?id={ID}"))
            .header("HOST", HOST)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    assert!(
        response.status().is_success(),
        "streaming handler should return 2xx, got {}",
        response.status()
    );

    let body = collect_body(response).await?;
    let got = parse_numbers(&body);

    let cancel_body = cancel_response
        .await
        .context("cancel task panicked")?
        .context("cancel request failed")?
        .text()
        .await
        .context("reading cancel response failed")?;

    assert_eq!(
        cancel_body.trim(),
        "cancelled",
        "cancel should report the invocation was registered and cancelled"
    );

    assert!(
        !got.is_empty(),
        "expected the stream to deliver some numbers before cancellation, got none"
    );
    assert!(
        got.len() == DELAY_LENGTH as usize,
        "cancellation should truncate the stream, but received all {} numbers: {got:?}",
        got.len()
    );

    // Assert that the order in which the numbers came is correct.
    assert_eq!(
        got.as_slice(),
        &EXPECTED_FULL[..got.len()],
        "the truncated body should be an in-order prefix of the full stream"
    );

    Ok(())
}
