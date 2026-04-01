//! Integration test for HTTP + NatsBlobstore round-trip
//!
//! Requires Docker. Gated behind `NATS_INTEGRATION_TESTS=1` so CI can opt in.
//!
//! Uses testcontainers to spin up a NATS server with JetStream, then validates
//! that the http-blobstore fixture component can write data through NatsBlobstore
//! and read it back correctly — exercising the TempFileOutputStream-based write path.

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
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

struct TestHarness {
    addr: SocketAddr,
    nats_client: async_nats::Client,
    // Hold references to keep them alive
    _host: Box<dyn std::any::Any + Send>,
    _container: Box<dyn std::any::Any + Send>,
}

async fn setup() -> Result<TestHarness> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

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
                        config.insert("buckets".to_string(), "my-container-real".to_string());
                        config
                    },
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    Ok(TestHarness {
        addr,
        nats_client,
        _host: Box::new(host),
        _container: Box::new(container),
    })
}

impl TestHarness {
    async fn cleanup(&self) -> Result<()> {
        let js = async_nats::jetstream::new(self.nats_client.clone());
        js.delete_object_store("my-container-real")
            .await
            .context("Failed to delete object store bucket")?;
        Ok(())
    }
}

#[tokio::test]
async fn test_nats_blobstore_roundtrip() -> Result<()> {
    if std::env::var("NATS_INTEGRATION_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping NATS integration test (set NATS_INTEGRATION_TESTS=1 to enable)");
        return Ok(());
    }

    let harness = setup().await?;

    let test_data = "Hello from NATS blobstore integration test!";
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .post(format!("http://{}/", harness.addr))
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

    harness.cleanup().await?;
    println!("NATS blobstore round-trip test passed");
    Ok(())
}

#[tokio::test]
async fn test_nats_blobstore_large_payload() -> Result<()> {
    if std::env::var("NATS_LARGE_PAYLOAD_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping large payload test (set NATS_LARGE_PAYLOAD_TESTS=1 to enable)");
        return Ok(());
    }

    let harness = setup().await?;

    // 2GB payload: 2048 chunks of 1MB each, filled with a repeating byte pattern.
    // Neither the sender nor receiver holds the full 2GB in memory.
    const CHUNK_SIZE: usize = 1024 * 1024; // 1MB
    const NUM_CHUNKS: usize = 2048; // 2GB total
    const TOTAL_SIZE: u64 = (CHUNK_SIZE * NUM_CHUNKS) as u64;

    println!("Sending {TOTAL_SIZE} bytes ({NUM_CHUNKS} x {CHUNK_SIZE}) through NATS blobstore...");
    let start = std::time::Instant::now();

    // Build a streaming body that yields 1MB chunks without allocating 2GB
    let body_stream = futures::stream::iter((0..NUM_CHUNKS).map(|i| {
        // Each chunk has a deterministic pattern: byte value = chunk_index % 251 (prime)
        let fill_byte = (i % 251) as u8;
        let chunk = vec![fill_byte; CHUNK_SIZE];
        Ok::<_, std::io::Error>(bytes::Bytes::from(chunk))
    }));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;

    let response = client
        .post(format!("http://{}/", harness.addr))
        .header("HOST", "foo")
        .body(reqwest::Body::wrap_stream(body_stream))
        .send()
        .await
        .context("Failed to send large payload")?;

    let status = response.status();
    assert!(status.is_success(), "Expected success, got {status}");

    // Stream the response back and verify size + content without holding 2GB in memory
    let mut response_bytes_read: u64 = 0;
    let mut chunk_index: usize = 0;
    let mut stream = response.bytes_stream();
    use futures::StreamExt;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("Failed to read response chunk")?;
        let chunk_len = chunk.len();

        // Verify content of each byte against expected pattern
        for &byte in chunk.iter() {
            let expected_chunk_idx = (response_bytes_read as usize / CHUNK_SIZE) % NUM_CHUNKS;
            let expected_byte = (expected_chunk_idx % 251) as u8;
            if byte != expected_byte {
                anyhow::bail!(
                    "Content mismatch at byte offset {response_bytes_read}: \
                     expected {expected_byte:#04x}, got {byte:#04x} \
                     (chunk {expected_chunk_idx})"
                );
            }
            response_bytes_read += 1;
        }

        // Progress logging every 256MB
        if response_bytes_read / (256 * 1024 * 1024)
            != (response_bytes_read - chunk_len as u64) / (256 * 1024 * 1024)
        {
            let mb = response_bytes_read / (1024 * 1024);
            println!("  verified {mb}MB / {}", TOTAL_SIZE / (1024 * 1024));
        }

        chunk_index += 1;
    }

    let elapsed = start.elapsed();
    let throughput_mbps = (TOTAL_SIZE as f64 * 2.0) / (1024.0 * 1024.0) / elapsed.as_secs_f64();

    assert_eq!(
        response_bytes_read, TOTAL_SIZE,
        "Size mismatch: expected {TOTAL_SIZE} bytes, got {response_bytes_read}"
    );

    println!(
        "Large payload test passed: {TOTAL_SIZE} bytes round-tripped in {elapsed:.1?} \
         ({throughput_mbps:.1} MB/s), {chunk_index} response chunks"
    );

    harness.cleanup().await?;
    Ok(())
}
