#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end `(implements ..)` routing of the async, native-stream
//! `wasmcloud:blobstore@0.1.0` through a **real P3 guest**, backed by NATS.
//!
//! The `blobstore-implements-p3` fixture imports the async blobstore under the
//! labels `store-a` (the `blobstore` interface) and `objects-a` (the `container`
//! interface). On each request it creates a container, streams an object body in
//! via `write-data(name, stream<u8>)`, reads it back out via
//! `get-data(..) -> stream<u8>`, and returns the bytes.
//!
//! The host binds a single [`MultiplexedAsyncBlobstore`] with a NATS provider and
//! declares the two named interfaces routed to the Dockerized NATS object store.
//! A 200 whose body echoes the written bytes proves the concurrent/stream host
//! ABI works end to end through a guest — the producer (`get-data`), the consumer
//! (`write-data`), and the implements id threading — against a real backend.
//!
//! Requires Docker (NATS JetStream); marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_blobstore::{MultiplexedAsyncBlobstore, NatsBlobProvider},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const BLOBSTORE_IMPLEMENTS_P3_WASM: &[u8] = include_bytes!("wasm/blobstore_implements_p3.wasm");

/// A named async `wasmcloud:blobstore` interface routed to the NATS backend.
/// `name` is the `(implements ..)` label the guest imports under; `iface` is the
/// blobstore sub-interface (`blobstore` or `container`).
fn nats_blob_iface(name: &str, iface: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: [iface.to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::from([
            ("backend".to_string(), "nats".to_string()),
            ("url".to_string(), url.to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

#[tokio::test]
#[ignore = "requires Docker (NATS JetStream); run with `cargo test --include-ignored`"]
async fn p3_guest_streams_blobstore_through_nats() -> Result<()> {
    // --- NATS container (JetStream enabled for the object store) ---
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_port = nats.get_host_port_ipv4(4222).await?;
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // --- host with the async multiplexed blobstore plugin + NATS provider ---
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(
            MultiplexedAsyncBlobstore::new().with_provider(Arc::new(NatsBlobProvider)),
        ))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // The guest's `store-a` (blobstore) and `objects-a` (container) implements
    // imports both route to the same NATS server (pooled by url).
    let host_interfaces = vec![
        http_incoming_handler_interface("blob-impl", None),
        nats_blob_iface("store-a", "blobstore", &nats_url),
        nats_blob_iface("objects-a", "container", &nats_url),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "blobstore-implements-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "blobstore-implements-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(BLOBSTORE_IMPLEMENTS_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs the named-imports `add_to_linker` for both `store-a` and
    // `objects-a`. `workload_start` returns Ok even on resolution failure (it
    // encodes the error in the status), so assert it actually reached Running.
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "workload should resolve: {}",
        resp.workload_status.message
    );

    // Drive the guest: it creates a container, streams an object in, reads it
    // back, and echoes the bytes. A matching body proves the stream ABI flowed
    // both directions through the real NATS object store.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "blob-impl")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "meow from a real p3 guest",
        "the guest must round-trip the object body through the NATS-backed stream ABI"
    );

    Ok(())
}
