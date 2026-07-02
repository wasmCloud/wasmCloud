#![cfg(feature = "wasm_component_model_implements")]
//! A **plain** (unlabeled) async `wasmcloud:blobstore@0.1.0` import, served by a
//! default backend through a **real P3 guest** — no `(implements ..)` label.
//!
//! The `blobstore-default-p3` fixture imports `wasmcloud:blobstore/blobstore`
//! plainly. On each request it creates a container, streams an object body in via
//! `write-data(name, stream<u8>)`, reads it back via `get-data(..) -> stream<u8>`,
//! and returns the bytes.
//!
//! The host binds a single [`MultiplexedAsyncBlobstore`] with the in-memory
//! provider and declares an **unnamed** blobstore interface (the default route).
//! A 200 whose body echoes the written bytes proves that a component gets a
//! working backend from a plain import — no label needed — exercising the
//! standard (non-`(implements)`) binding to the workload's default backend.
//!
//! Unlike the NATS-backed blobstore integration tests, this uses the in-memory
//! backend, so it needs no Docker and is not `#[ignore]`d.
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
    plugin::wasi_blobstore::{InMemoryProvider, MultiplexedAsyncBlobstore},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const BLOBSTORE_DEFAULT_P3_WASM: &[u8] = include_bytes!("wasm/blobstore_default_p3.wasm");

/// An UNNAMED async `wasmcloud:blobstore` interface — no `(implements ..)` label,
/// no backend override, so it is the workload's default route and falls back to
/// the in-memory backend.
fn default_blob_iface() -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: [
            "blobstore".to_string(),
            "container".to_string(),
            "types".to_string(),
        ]
        .into_iter()
        .collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

#[tokio::test]
async fn p3_guest_plain_blobstore_uses_default_backend() -> Result<()> {
    // --- host with the async multiplexed blobstore plugin + in-memory provider ---
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(
            MultiplexedAsyncBlobstore::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // The guest imports blobstore *plainly* (no label); the unnamed interface is
    // the default route, which the plugin binds via the standard host interface.
    let host_interfaces = vec![
        http_incoming_handler_interface("blob-default", None),
        default_blob_iface(),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "blobstore-default-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "blobstore-default-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(BLOBSTORE_DEFAULT_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs the *standard* `blobstore::add_to_linker` (not the named one)
    // for the plain import. `workload_start` returns Ok even on resolution
    // failure (it encodes the error in the status), so assert it reached Running.
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "a plain (unlabeled) blobstore import should resolve to a default backend: {}",
        resp.workload_status.message
    );

    // Drive the guest: create a container, stream an object in, read it back, and
    // echo the bytes. A matching body proves the plain import reached a working
    // default backend through the stream ABI — with no label.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "blob-default")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "woof from a plain p3 guest",
        "the guest must round-trip the object body through the default (unlabeled) backend"
    );

    Ok(())
}
