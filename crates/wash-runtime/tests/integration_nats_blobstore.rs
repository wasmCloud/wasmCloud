//! Integration test for HTTP + NatsBlobstore round-trip
//!
//! Requires Docker (NATS); marked `#[ignore]`, run with `cargo test --include-ignored`.
//!
//! Uses testcontainers to spin up a NATS server with JetStream, then validates
//! that the http-blobstore fixture component can write data through NatsBlobstore
//! and read it back correctly — exercising the TempFileOutputStream-based write path.

use anyhow::{Context, Result};
use futures::{StreamExt as _, TryStreamExt as _};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
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
        http::{DevRouter, Ingress},
    },
    plugin::wasi_blobstore::{NatsBlobstore, PLUGIN_BLOBSTORE_ID},
    plugin::{PluginBindingSet, PluginBindings, WorkloadConfigPolicy},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

const HTTP_BLOBSTORE_WASM: &[u8] = include_bytes!("wasm/http_blobstore.wasm");

struct TestHarness {
    addr: SocketAddr,
    nats_client: async_nats::Client,
    /// Where the workload stands once started, and why.
    state: WorkloadState,
    message: String,
    // Hold references to keep them alive
    _host: Box<dyn std::any::Any + Send>,
    _container: Box<dyn std::any::Any + Send>,
}

async fn setup() -> Result<TestHarness> {
    let (container, url) = start_nats(None).await?;
    let nats_client = async_nats::connect(url)
        .await
        .context("Failed to connect to NATS")?;
    let harness = start(
        container,
        nats_client,
        HashMap::new(),
        PluginBindings::new(),
    )
    .await?;
    anyhow::ensure!(
        harness.state == WorkloadState::Running,
        "the workload did not start: {}",
        harness.message
    );
    Ok(harness)
}

/// Starts NATS with JetStream, from `config` when given, returning it and its
/// URL.
async fn start_nats(
    config: Option<&str>,
) -> Result<(testcontainers::ContainerAsync<GenericImage>, String)> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let image = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"));
    let container = match config {
        Some(config) => image
            .with_copy_to("/etc/nats/test.conf", config.as_bytes().to_vec())
            .with_cmd(["-c", "/etc/nats/test.conf"]),
        None => image.with_cmd(["-js"]),
    }
    .start()
    .await
    .map_err(|e| anyhow::anyhow!("Failed to start NATS container: {e}"))?;

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS host port: {e}"))?;
    Ok((container, format!("nats://127.0.0.1:{port}")))
}

/// The fixture's workload on a host whose blobstore connects as
/// `nats_client`, its blobstore binding's config `binding` beside `buckets`.
async fn start(
    container: testcontainers::ContainerAsync<GenericImage>,
    nats_client: async_nats::Client,
    binding: HashMap<String, String>,
    bindings: PluginBindings,
) -> Result<TestHarness> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let blobstore_plugin = NatsBlobstore::new(&nats_client);

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(blobstore_plugin))?
        .with_plugin_bindings(bindings)
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
                    allowed_ip_name_lookups: Default::default(),
                    allowed_host_loopback_ports: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::new(0, 2, 2)),
                    config: {
                        let mut config = wash_runtime::wit::Config::new();
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
                    version: Some(semver::Version::parse("0.2.0-draft").map_err(|e| {
                        anyhow::anyhow!("invalid blobstore interface version: {e}")
                    })?),
                    config: {
                        let mut config = wash_runtime::wit::Config::from(binding);
                        config.insert("buckets".to_string(), "my-container-real".to_string());
                        config
                    },
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };

    let status = host
        .workload_start(req)
        .await
        .context("Failed to start workload")?
        .workload_status;

    Ok(TestHarness {
        addr,
        nats_client,
        state: status.workload_state,
        message: status.message,
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

/// Sends a request to the blobstore fixture, which answers a `/op/...` path
/// with a blobstore operation or, on any other path, writes the body as
/// `hi.txt` and reads it back.
async fn blobstore_request(
    client: &reqwest::Client,
    addr: SocketAddr,
    path: &str,
) -> Result<(u16, String)> {
    let response = timeout(
        Duration::from_secs(10),
        client
            .post(format!("http://{addr}{path}"))
            .header("HOST", "foo")
            .body("hello")
            .send(),
    )
    .await
    .context("HTTP request timed out")?
    .context("Failed to make HTTP request")?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .context("Failed to read response body")?;
    Ok((status, body))
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn test_nats_blobstore_roundtrip() -> Result<()> {
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

/// A deleted object stays absent through the plugin's `has-object`,
/// `object-info` and listing, although NATS keeps its tombstone.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn test_nats_blobstore_deleted_object_is_absent() -> Result<()> {
    let harness = setup().await?;
    let client = reqwest::Client::new();

    // The fixture writes `hi.txt` and reads it back.
    let (status, body) = blobstore_request(&client, harness.addr, "/").await?;
    assert_eq!((status, body.as_str()), (200, "hello"));

    // With the object present, the plugin reports and lists it.
    for (path, expected) in [("/op/has-object", "true"), ("/op/list", "hi.txt")] {
        let (status, body) = blobstore_request(&client, harness.addr, path).await?;
        assert_eq!(
            (status, body.as_str()),
            (200, expected),
            "{path} must see the object while it exists"
        );
    }

    let store = async_nats::jetstream::new(harness.nats_client.clone())
        .get_object_store("my-container-real")
        .await
        .context("Failed to open the object store")?;
    store
        .delete("hi.txt")
        .await
        .context("Failed to delete the object")?;

    // The premise: NATS keeps the deleted object's metadata as a tombstone,
    // which `info` returns with `deleted: true`.
    assert!(
        store
            .info("hi.txt")
            .await
            .context("Failed to read the tombstone")?
            .deleted,
        "NATS must keep a deleted object's tombstone"
    );

    let (status, body) = blobstore_request(&client, harness.addr, "/op/has-object").await?;
    assert_eq!(
        (status, body.as_str()),
        (200, "false"),
        "has-object must treat a deleted object as absent"
    );

    let (status, body) = blobstore_request(&client, harness.addr, "/op/object-info").await?;
    assert_eq!(
        status, 500,
        "object-info must treat a deleted object as absent: {body}"
    );

    let (status, body) = blobstore_request(&client, harness.addr, "/op/list").await?;
    assert_eq!(
        (status, body.as_str()),
        (200, ""),
        "a listing must omit tombstones"
    );

    harness.cleanup().await?;
    Ok(())
}

/// Two accounts, each with JetStream: the host connects as `host`, a workload's
/// binding as `node`.
const ACCOUNTS: &str = r#"
jetstream {}
accounts {
  HOST: { jetstream: enabled, users: [{ user: host, password: host }] }
  NODE: { jetstream: enabled, users: [{ user: node, password: node }] }
}
"#;

/// NATS with [`ACCOUNTS`], its URL, and a client of each account.
async fn start_accounts() -> Result<(
    testcontainers::ContainerAsync<GenericImage>,
    String,
    async_nats::Client,
    async_nats::Client,
)> {
    let (container, url) = start_nats(Some(ACCOUNTS)).await?;
    let connect = |user: &str| {
        async_nats::ConnectOptions::with_user_and_password(user.to_string(), user.to_string())
            .connect(url.clone())
    };
    let host = connect("host").await.context("Failed to connect as host")?;
    let node = connect("node").await.context("Failed to connect as node")?;
    Ok((container, url, host, node))
}

/// A binding that names credentials opens its containers on a connection of
/// its own, made as them: the container lives in their account, not the host's.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn test_nats_blobstore_credentials_choose_the_account() -> Result<()> {
    let (container, url, host, node) = start_accounts().await?;
    let binding = HashMap::from([
        ("servers".to_string(), url),
        ("username".to_string(), "node".to_string()),
        ("password".to_string(), "node".to_string()),
    ]);
    // `allow`: the workload brings its credentials, as a manifest's
    // `secretFrom` does.
    let bindings = PluginBindings::new().with_plugin(
        PluginBindingSet::new(PLUGIN_BLOBSTORE_ID)
            .with_workload_config(WorkloadConfigPolicy::Allow),
    );
    let harness = start(container, host.clone(), binding, bindings).await?;
    assert_eq!(harness.state, WorkloadState::Running, "{}", harness.message);

    let (status, body) = blobstore_request(&reqwest::Client::new(), harness.addr, "/").await?;
    assert_eq!((status, body.as_str()), (200, "hello"));

    async_nats::jetstream::new(node)
        .get_object_store("my-container-real")
        .await
        .context("the container must be in the binding's account")?;
    assert!(
        async_nats::jetstream::new(host)
            .get_object_store("my-container-real")
            .await
            .is_err(),
        "the container must not be in the host's account"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn test_nats_blobstore_mtls_identity_chooses_the_account() -> Result<()> {
    wash_runtime::init_crypto();
    let dir = tempfile::tempdir()?;
    let mut ca_params = CertificateParams::new(Vec::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate()?)?;
    let ca_path = dir.path().join("ca.crt");
    std::fs::write(&ca_path, ca.pem())?;

    let issue = |name: &str, names: Vec<String>, usage| -> Result<_> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(names)?;
        params.extended_key_usages = vec![usage];
        let cert = params.signed_by(&key, &ca)?;
        let cert_path = dir.path().join(format!("{name}.crt"));
        let key_path = dir.path().join(format!("{name}.key"));
        std::fs::write(&cert_path, cert.pem())?;
        std::fs::write(&key_path, key.serialize_pem())?;
        Ok((cert_path, key_path))
    };
    let (server_cert, server_key) = issue(
        "server",
        vec!["localhost".into(), "127.0.0.1".into()],
        ExtendedKeyUsagePurpose::ServerAuth,
    )?;
    let (host_cert, host_key) = issue(
        "host",
        vec!["host".into()],
        ExtendedKeyUsagePurpose::ClientAuth,
    )?;
    let (node_cert, node_key) = issue(
        "node",
        vec!["node".into()],
        ExtendedKeyUsagePurpose::ClientAuth,
    )?;

    let config = r#"
jetstream {}
tls {
  cert_file: "/etc/nats/server.crt"
  key_file: "/etc/nats/server.key"
  ca_file: "/etc/nats/ca.crt"
  verify_and_map: true
}
accounts {
  HOST: { jetstream: enabled, users: [{ user: host }] }
  NODE: { jetstream: enabled, users: [{ user: node }] }
}
"#;
    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_copy_to("/etc/nats/test.conf", config.as_bytes().to_vec())
        .with_copy_to("/etc/nats/ca.crt", std::fs::read(&ca_path)?)
        .with_copy_to("/etc/nats/server.crt", std::fs::read(server_cert)?)
        .with_copy_to("/etc/nats/server.key", std::fs::read(server_key)?)
        .with_cmd(["-c", "/etc/nats/test.conf"])
        .start()
        .await?;
    let port = container.get_host_port_ipv4(4222).await?;
    let url = format!("tls://127.0.0.1:{port}");
    let host = async_nats::ConnectOptions::new()
        .add_root_certificates(ca_path.clone())
        .add_client_certificate(host_cert, host_key)
        .connect(&url)
        .await
        .context("Failed to connect with the host certificate")?;
    let node = async_nats::ConnectOptions::new()
        .add_root_certificates(ca_path.clone())
        .add_client_certificate(node_cert.clone(), node_key.clone())
        .connect(&url)
        .await
        .context("Failed to connect with the binding certificate")?;

    let binding = HashMap::from([
        ("servers".into(), url),
        ("tls-ca".into(), ca_path.display().to_string()),
        ("tls-cert".into(), node_cert.display().to_string()),
        ("tls-key".into(), node_key.display().to_string()),
    ]);
    let bindings = PluginBindings::new().with_plugin(
        PluginBindingSet::new(PLUGIN_BLOBSTORE_ID)
            .with_workload_config(WorkloadConfigPolicy::Allow),
    );
    let harness = start(container, host.clone(), binding, bindings).await?;
    assert_eq!(harness.state, WorkloadState::Running, "{}", harness.message);

    let (status, body) = blobstore_request(&reqwest::Client::new(), harness.addr, "/").await?;
    assert_eq!((status, body.as_str()), (200, "hello"));

    async_nats::jetstream::new(node)
        .get_object_store("my-container-real")
        .await
        .context("the container must be in the certificate's account")?;
    let host_streams: Vec<_> = async_nats::jetstream::new(host)
        .stream_names()
        .try_collect()
        .await?;
    assert!(host_streams.is_empty(), "host streams: {host_streams:?}");
    Ok(())
}

/// Under `deny`, the default, whom a binding connects as is the operator's to
/// say: a workload naming credentials is refused.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn test_nats_blobstore_workload_credentials_refused_under_deny() -> Result<()> {
    let (container, _url, host, _node) = start_accounts().await?;
    let binding = HashMap::from([
        ("username".to_string(), "node".to_string()),
        ("password".to_string(), "from-the-manifest".to_string()),
    ]);
    let harness = start(container, host, binding, PluginBindings::new()).await?;
    assert_eq!(harness.state, WorkloadState::Error, "{}", harness.message);
    assert!(harness.message.contains("password"), "{}", harness.message);
    assert!(
        !harness.message.contains("from-the-manifest"),
        "{}",
        harness.message
    );
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
