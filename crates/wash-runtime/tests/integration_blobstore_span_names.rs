//! Integration test asserting that `wasi:blobstore` host calls emit OTEL spans
//! named per the `<namespace>.<package>.<fn>` convention.
//!
//! Exercises the same http-blobstore component as `integration_http_blobstore`,
//! but installs a custom `tracing::Layer` that records every span name created
//! during the run. After the request round-trips, the recorded set is checked
//! against the names the host plugin should have produced.

use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::timeout;
use tracing::{Subscriber, span::Attributes};
use tracing_subscriber::{Layer, layer::Context as LayerContext, prelude::*, registry::Registry};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_blobstore::InMemoryBlobstore,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_BLOBSTORE_WASM: &[u8] = include_bytes!("wasm/http_blobstore.wasm");

/// Layer that appends each new span's name to a shared `Vec`. Pure recorder —
/// does not filter, so it sees every span regardless of subscriber-level level
/// filters added by other layers.
#[derive(Clone, Default)]
struct SpanNameRecorder {
    names: Arc<Mutex<Vec<String>>>,
}

impl<S: Subscriber> Layer<S> for SpanNameRecorder {
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &tracing::Id, _ctx: LayerContext<'_, S>) {
        self.names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(attrs.metadata().name().to_string());
    }
}

#[tokio::test]
async fn wasi_blobstore_handlers_emit_namespaced_spans() -> Result<()> {
    let recorder = SpanNameRecorder::default();
    let names = recorder.names.clone();

    // Install a global subscriber for this test binary. Only one test in this
    // file, so no contention with set_global_default.
    Registry::default()
        .with(recorder)
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::from_default_env()),
        )
        .try_init()
        .context("failed to install tracing subscriber")?;

    let engine = Engine::builder().build()?;
    let http_plugin = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_plugin.addr();
    let blobstore_plugin = InMemoryBlobstore::new(None);

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(blobstore_plugin))?
        .build()?;
    let host = host.start().await.context("Failed to start host")?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "span-name-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-blobstore-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_BLOBSTORE_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: Default::default(),
                    allow_ip_name_lookup: false,
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::new(0, 2, 2)),
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "foo".to_string());
                        config
                    },
                    name: None,
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "blobstore".to_string(),
                    interfaces: [
                        "blobstore".to_string(),
                        "container".to_string(),
                        "types".to_string(),
                    ]
                    .into_iter()
                    .collect(),
                    version: Some(
                        semver::Version::parse("0.2.0-draft").expect("valid semver version"),
                    ),
                    config: HashMap::new(),
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let test_data = "span-naming-payload";
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/"))
            .header("HOST", "foo")
            .body(test_data)
            .send(),
    )
    .await
    .context("HTTP request timed out")??;
    assert!(response.status().is_success());
    let body = response.text().await?;
    assert_eq!(body.trim(), test_data);

    // The http-blobstore component (see tests/fixtures/http-blobstore/src/lib.rs)
    // exercises each of these host functions on every request. If any of these
    // names goes missing, downstream Tempo / Grafana filters that key off the
    // `wasi.blobstore.*` prefix lose visibility.
    let captured = names.lock().unwrap().clone();
    let expected = [
        "wasi.blobstore.create_container",
        "wasi.blobstore.new_outgoing_value",
        "wasi.blobstore.outgoing_value_write_body",
        "wasi.blobstore.write_data",
        "wasi.blobstore.finish",
        "wasi.blobstore.get_data",
        "wasi.blobstore.incoming_value_consume_async",
    ];
    for name in expected {
        assert!(
            captured.iter().any(|n| n == name),
            "expected span {name} was not emitted; captured wasi.blobstore.* spans: {:?}",
            captured
                .iter()
                .filter(|n| n.starts_with("wasi.blobstore."))
                .collect::<Vec<_>>()
        );
    }

    // Catch drift: every wasi.blobstore.* span the run produces must match the
    // <ns>.<pkg>.<fn> shape. Three dot-separated segments, all lowercase
    // ASCII / digits / underscores.
    for n in captured.iter().filter(|n| n.starts_with("wasi.blobstore.")) {
        let parts: Vec<&str> = n.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "span name `{n}` is not <namespace>.<package>.<fn>"
        );
        assert!(
            parts[2]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "span name `{n}` has an unexpected fn segment"
        );
    }

    Ok(())
}
