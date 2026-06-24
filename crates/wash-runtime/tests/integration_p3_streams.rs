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
//! streams (first byte arrives long before the last) rather than being
//! buffered to completion before the response is sent.
//!
//! `test_p3_cross_component_stream_streams_incrementally`: the cross-component
//! analog of the pacer test. The producer paces its bytes, and the test times
//! their arrival at the client to prove the `stream<u8>` stays concurrent as
//! it crosses the linker (first chunk arrives well before the last) rather
//! than being buffered at the linker boundary. The pacer test alone can't show
//! this — it never crosses the linker — and the byte-for-byte test above can't
//! either, since a buffer-then-forward implementation would satisfy it too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use futures::StreamExt;
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
use common::{http_only_host_interfaces, start_host_with_p3};

const STREAM_PRODUCER_P3_WASM: &[u8] = include_bytes!("wasm/stream_producer_p3.wasm");
const STREAM_CONSUMER_P3_WASM: &[u8] = include_bytes!("wasm/stream_consumer_p3.wasm");
const STREAM_PACER_P3_WASM: &[u8] = include_bytes!("wasm/stream_pacer_p3.wasm");

#[tokio::test]
async fn test_p3_cross_component_stream_to_http() -> Result<()> {
    let (addr, host) = start_host_with_p3("127.0.0.1:0").await?;

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
                },
                Component {
                    name: "stream-producer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_PRODUCER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
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
///   - Streaming: the response headers come back immediately, so the first
///     chunk lands within ~one tick while the last lands ~0.9s later.
///   - Buffering (the old `body.collect().await` path): the handler can't
///     return a response until the whole body is produced, so the first and
///     last chunks arrive together near the ~0.9s mark.
///
/// Asserting `first < last / 2` fails on the buffered path and passes on the
/// streaming path. Margins are deliberately loose to stay CI-stable.
#[tokio::test]
async fn test_p3_incoming_handler_streams_incrementally() -> Result<()> {
    let (addr, host) = start_host_with_p3("127.0.0.1:0").await?;

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

    let mut stream = response.bytes_stream();
    let mut first_at: Option<Duration> = None;
    let mut last_at = Duration::ZERO;
    let mut body = Vec::new();
    while let Some(chunk) = timeout(Duration::from_secs(10), stream.next())
        .await
        .context("body chunk timed out")?
        .transpose()?
    {
        if chunk.is_empty() {
            continue;
        }
        let now = start.elapsed();
        first_at.get_or_insert(now);
        last_at = now;
        body.extend_from_slice(&chunk);
    }

    let first_at = first_at.context("response body was empty")?;

    // Sanity: the producer really did pace its output over time. Both the
    // streaming and buffering paths satisfy this (the producer takes ~0.9s
    // either way); it just guards against a degenerate instant body.
    assert!(
        last_at >= Duration::from_millis(400),
        "expected paced output to span >=400ms, last chunk at {last_at:?}"
    );

    // The streaming assertion: the first chunk must arrive well before the
    // last. Under the old buffered path first ≈ last and this fails.
    assert!(
        first_at < last_at / 2,
        "response body was buffered, not streamed: first chunk at {first_at:?}, \
         last at {last_at:?} (first should be < last/2)"
    );

    let expected: String = (0..10).map(|i| format!("chunk-{i}\n")).collect();
    assert_eq!(
        String::from_utf8(body).context("body not utf8")?,
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
///   - Concurrent/streamed: the producer's first byte propagates across the
///     linker and out to the client within ~one tick, while the last lands
///     ~0.75s later.
///   - Buffered at the boundary: if the linker collected the producer's
///     `stream<u8>` to completion before handing it over (or the consumer
///     buffered it), first ≈ last and the response would only start near the
///     end.
///
/// This is the cross-component counterpart to
/// `test_p3_incoming_handler_streams_incrementally`, which never crosses the
/// linker, and a sharper check than `test_p3_cross_component_stream_to_http`,
/// which only asserts the final bytes and so passes even on a buffered path.
#[tokio::test]
async fn test_p3_cross_component_stream_streams_incrementally() -> Result<()> {
    let (addr, host) = start_host_with_p3("127.0.0.1:0").await?;

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
                },
                Component {
                    name: "stream-producer".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(STREAM_PRODUCER_P3_WASM),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
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

    let mut stream = response.bytes_stream();
    let mut first_at: Option<Duration> = None;
    let mut last_at = Duration::ZERO;
    let mut body = Vec::new();
    while let Some(chunk) = timeout(Duration::from_secs(10), stream.next())
        .await
        .context("body chunk timed out")?
        .transpose()?
    {
        if chunk.is_empty() {
            continue;
        }
        let now = start.elapsed();
        first_at.get_or_insert(now);
        last_at = now;
        body.extend_from_slice(&chunk);
    }

    let first_at = first_at.context("response body was empty")?;

    // Sanity: the producer really did pace its bytes across the linker (16
    // bytes at a 50ms tick ≈ 0.75s). Loose bound to stay CI-stable.
    assert!(
        last_at >= Duration::from_millis(300),
        "expected paced output to span >=300ms, last chunk at {last_at:?}"
    );

    // The concurrency assertion: the producer's bytes reach the client
    // incrementally through the linker, not all bunched at the end. Under a
    // buffered path first ≈ last and this fails.
    assert!(
        first_at < last_at / 2,
        "cross-component stream was buffered, not streamed: first chunk at \
         {first_at:?}, last at {last_at:?} (first should be < last/2)"
    );

    // And it must still reassemble to the producer's output byte-for-byte.
    assert_eq!(
        String::from_utf8(body).context("body not utf8")?,
        "abcdefghijklmnop",
        "streamed body should reassemble to the producer's output"
    );

    Ok(())
}
