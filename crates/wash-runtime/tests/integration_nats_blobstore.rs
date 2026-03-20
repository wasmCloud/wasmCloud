//! Integration test for HTTP + NatsBlobstore round-trip
//!
//! Requires Docker. Gated behind `NATS_INTEGRATION_TESTS=1` so CI can opt in.
//!
//! Uses testcontainers to spin up a NATS server with JetStream, then validates
//! that the http-blobstore fixture component can write data through NatsBlobstore
//! and read it back correctly — exercising the TempFileOutputStream-based write path.

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
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
    plugin::wasi_blobstore::NatsBlobstore,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_BLOBSTORE_WASM: &[u8] = include_bytes!("wasm/http_blobstore.wasm");

#[tokio::test]
async fn test_nats_blobstore_roundtrip() -> Result<()> {
    if std::env::var("NATS_INTEGRATION_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping NATS integration test (set NATS_INTEGRATION_TESTS=1 to enable)");
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    // Start NATS with JetStream via testcontainers
    let container = GenericImage::new("nats", "2-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start NATS container: {e}"))?;

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS host port: {e}"))?;

    let nats_client = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .context("Failed to connect to NATS")?;

    // Create engine and plugins
    let engine = Engine::builder().build()?;
    let http_plugin = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_plugin.addr();
    let blobstore_plugin = NatsBlobstore::new(&nats_client);

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(blobstore_plugin))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;

    // Start workload — the fixture component uses container "my-container-real"
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "nats-blobstore-workload".to_string(),
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
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
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
                    version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
                    config: {
                        let mut config = HashMap::new();
                        // The fixture component uses "my-container-real"
                        config.insert("buckets".to_string(), "my-container-real".to_string());
                        config
                    },
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };

    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start workload")?;
    println!(
        "Started workload: {:?}",
        workload_response.workload_status.workload_id
    );

    // POST data through the component → NatsBlobstore → read back
    let test_data = "Hello from NATS blobstore integration test!";
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .post(format!("http://{addr}/"))
            .header("HOST", "foo")
            .body(test_data)
            .send(),
    )
    .await
    .context("HTTP request timed out")?
    .context("Failed to make HTTP request")?;

    let status = response.status();
    let response_text = response
        .text()
        .await
        .context("Failed to read response body")?;

    assert!(status.is_success(), "Expected success, got {status}");
    assert_eq!(
        response_text.trim(),
        test_data,
        "Round-trip data mismatch: expected {test_data:?}, got {response_text:?}"
    );

    // Clean up the object store bucket
    let js = async_nats::jetstream::new(nats_client);
    js.delete_object_store("my-container-real")
        .await
        .context("Failed to delete object store bucket")?;

    println!("NATS blobstore round-trip test passed");
    Ok(())
}
