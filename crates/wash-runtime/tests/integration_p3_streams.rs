//! Integration tests for WASIP3 streaming.
//!
//! `test_p3_cross_component_stream_to_http`: a P3 HTTP handler
//! (`stream-consumer-p3`) imports a `produce` function from a second P3
//! component (`stream-producer-p3`) and forwards the returned `stream<u8>`
//! to its response body. This exercises:
//!   - the stream handle crossing the dynamic linker boundary intact
//!     (`engine::value::lower_with_type` resource-identity passthrough),
//!   - auto-linking every component in a workload
//!     (`ResolvedWorkload::component_ids_except`),
//!   - streaming a P3 response body straight through to hyper
//!     (`host::http_p3::handle_component_request_p3`).
//!
//! `test_p3_incoming_handler_streams_incrementally`: a single paced handler
//! (`stream-pacer-p3`) proves that the incoming-handler body actually
//! streams — its chunks arrive spread over the time the guest took to produce
//! them — rather than being buffered to completion before the response is sent.
//!
//! `test_p3_cross_component_stream_streams_incrementally`: the cross-component
//! analog of the pacer test. The producer paces its bytes, and the test times
//! their arrival at the client to prove the `stream<u8>` stays concurrent as
//! it crosses the linker (the bytes arrive spread over time) rather than being
//! buffered at the linker boundary. The pacer test alone can't show
//! this — it never crosses the linker — and the byte-for-byte test above can't
//! either, since a buffer-then-forward implementation would satisfy it too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::time::timeout;

use wash_runtime::{
    host::HostApi,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
};

mod common;
use common::streaming::time_arrivals;
use common::{http_only_host_interfaces, start_host_with_p3_http_handler};

const STREAM_PRODUCER_P3_WASM: &[u8] = include_bytes!("wasm/stream_producer_p3.wasm");
const STREAM_CONSUMER_P3_WASM: &[u8] = include_bytes!("wasm/stream_consumer_p3.wasm");
const STREAM_PACER_P3_WASM: &[u8] = include_bytes!("wasm/stream_pacer_p3.wasm");

#[tokio::test]
async fn test_p3_cross_component_stream_to_http() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    // The consumer exports wasi:http/handler and is the workload's incoming
    // entrypoint; the producer is linked in and supplies the byte stream.
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-cross-component-stream".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "stream-consumer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_CONSUMER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 1,
                    ..Default::default()
                },
                Component {
                    name: "stream-producer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_PRODUCER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 1,
                    ..Default::default()
                },
            ],
            host_interfaces: http_only_host_interfaces("p3-stream"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("cross-component stream workload should start")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-stream")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    assert!(
        response.status().is_success(),
        "cross-component stream handler should return 2xx, got {}",
        response.status()
    );

    // Producer emits `n = 16` bytes 'a'..'p'; the consumer forwards them
    // verbatim, so the streamed-through body must reassemble exactly.
    let body = response.text().await?;
    assert_eq!(
        body, "abcdefghijklmnop",
        "streamed body should match the producer's output byte-for-byte"
    );

    Ok(())
}

/// Proves the P3 incoming-handler path streams the response body through to
/// hyper rather than buffering it.
///
/// `stream-pacer-p3` emits 10 chunks 100ms apart. We time arrivals from
/// *before* the request is sent:
///   - Streaming: the response headers come back immediately and each chunk
///     follows its tick, so the arrivals span ~0.9s.
///   - Buffering (the old `body.collect().await` path): the handler can't
///     return a response until the whole body is produced, so every chunk
///     arrives in one burst at the end.
///
/// The spread between first and last arrival tells those apart, and a slow
/// host — which delays the whole body alike — cannot turn one into the other.
/// Multi-threaded on purpose: on a current-thread runtime the guest's work
/// blocks this task's reads, and pacing the host performed would be recorded
/// as chunks arriving together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_p3_incoming_handler_streams_incrementally() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-stream-pacer".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "stream-pacer".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(STREAM_PACER_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: http_only_host_interfaces("p3-pacer"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("pacer workload should start")?;

    // Time from before the request: under buffering the response itself is
    // withheld until the body is complete, so `send()` blocks for the full
    // ~0.9s and the first chunk can't arrive early.
    let start = Instant::now();
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-pacer")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    assert!(
        response.status().is_success(),
        "pacer handler should return 2xx, got {}",
        response.status()
    );

    let arrivals = time_arrivals(response, start).await?;

    // Ten chunks a tick apart span ~0.9s streamed, milliseconds buffered.
    arrivals.assert_streamed("the paced response body", Duration::from_millis(400));

    let expected: String = (0..10).map(|i| format!("chunk-{i}\n")).collect();
    assert_eq!(
        arrivals.text()?,
        expected,
        "streamed body should reassemble to all paced chunks in order"
    );

    Ok(())
}

/// Proves a stream that *crosses the dynamic linker* stays concurrent.
///
/// `stream-producer-p3` paces its bytes one per tick on a background task and
/// returns the reader immediately; `stream-consumer-p3` (a *separate*
/// component, linked in at runtime) drains that reader and forwards each byte
/// to its HTTP response body. We time arrivals at the client from *before* the
/// request:
///   - Concurrent/streamed: each byte propagates across the linker and out to
///     the client on its own tick, so the arrivals span ~0.75s.
///   - Buffered at the boundary: if the linker collected the producer's
///     `stream<u8>` to completion before handing it over (or the consumer
///     buffered it), the bytes reach the client in one burst at the end.
///
/// As in the pacer test, the spread between arrivals is what tells those
/// apart, and the runtime is multi-threaded so the guest's work does not block
/// this task's reads.
///
/// This is the cross-component counterpart to
/// `test_p3_incoming_handler_streams_incrementally`, which never crosses the
/// linker, and a sharper check than `test_p3_cross_component_stream_to_http`,
/// which only asserts the final bytes and so passes even on a buffered path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_p3_cross_component_stream_streams_incrementally() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    // Same two-component workload as the byte-for-byte test; here we measure
    // *when* the producer's bytes arrive, not just that they all do.
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "p3-cross-component-stream-paced".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "stream-consumer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_CONSUMER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 1,
                    ..Default::default()
                },
                Component {
                    name: "stream-producer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_PRODUCER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                    max_concurrency: 1,
                    ..Default::default()
                },
            ],
            host_interfaces: http_only_host_interfaces("p3-stream-paced"),
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("paced cross-component stream workload should start")?;

    let start = Instant::now();
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "p3-stream-paced")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    assert!(
        response.status().is_success(),
        "paced cross-component stream handler should return 2xx, got {}",
        response.status()
    );

    let arrivals = time_arrivals(response, start).await?;

    // 16 bytes at a 50ms tick span ~0.75s crossing the linker one at a time.
    arrivals.assert_streamed("the cross-component stream", Duration::from_millis(300));

    // And it must still reassemble to the producer's output byte-for-byte.
    assert_eq!(
        arrivals.text()?,
        "abcdefghijklmnop",
        "streamed body should reassemble to the producer's output"
    );

    Ok(())
}
