//! End-to-end integration test for the NatsMessaging plugin's subscriber path.
//!
//! Asserts the happy path: subscriptions configured via host_interfaces lead
//! to a NATS SUB landing on the bus, the handler component receives the
//! delivered message, and a reply published back via consumer.publish makes
//! the round trip. Future regressions in plugin lifecycle, tracker storage,
//! or the spawn loop that silently break subscription delivery will fail this
//! test.
//!
//! Requires Docker. Gated behind `NATS_INTEGRATION_TESTS=1` so CI can opt in.

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasmcloud_messaging::NatsMessaging,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const MESSAGING_HANDLER_WASM: &[u8] = include_bytes!("wasm/messaging_handler.wasm");

const SUBSCRIPTION_SUBJECT: &str = "test.echo";

struct TestHarness {
    nats_client: async_nats::Client,
    monitoring_url: String,
    _host: Box<dyn std::any::Any + Send>,
    _container: Box<dyn std::any::Any + Send>,
}

async fn setup() -> Result<TestHarness> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    // -m 8222 enables NATS' HTTP monitoring endpoint. The monitoring view
    // is the authoritative answer to "is this subscription registered with
    // the server right now?", which is the exact invariant we want to assert
    // independently from the request/reply round trip.
    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_exposed_port(8222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-m", "8222"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start NATS container: {e}"))?;

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS host port: {e}"))?;
    let monitoring_port = container
        .get_host_port_ipv4(8222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS monitoring host port: {e}"))?;

    let nats_url = format!("nats://127.0.0.1:{port}");
    let monitoring_url = format!("http://127.0.0.1:{monitoring_port}");
    let nats_client = async_nats::connect(&nats_url)
        .await
        .context("Failed to connect to NATS")?;

    // The plugin holds its own client — match the pattern host.rs uses, which
    // is what the runtime-operator chart wires up in production.
    let plugin_client = Arc::new(
        async_nats::connect(&nats_url)
            .await
            .context("Failed to connect plugin client to NATS")?,
    );

    let engine = Engine::builder().build()?;
    // HttpServer is required by HostBuilder even though this test doesn't
    // exercise HTTP — bind to ephemeral port 0.
    let http_plugin = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let messaging_plugin = NatsMessaging::new(plugin_client);

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(messaging_plugin))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;

    let mut subscription_config = HashMap::new();
    subscription_config.insert(
        "subscriptions".to_string(),
        SUBSCRIPTION_SUBJECT.to_string(),
    );

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "messaging-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "messaging-handler".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(MESSAGING_HANDLER_WASM),
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
            host_interfaces: vec![WitInterface {
                namespace: "wasmcloud".to_string(),
                package: "messaging".to_string(),
                interfaces: ["handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.0").unwrap()),
                config: subscription_config,
                name: None,
            }],
            volumes: vec![],
        },
    };

    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    Ok(TestHarness {
        nats_client,
        monitoring_url,
        _host: Box::new(host),
        _container: Box::new(container),
    })
}

#[tokio::test]
async fn test_nats_messaging_handler_subscription_round_trip() -> Result<()> {
    if std::env::var("NATS_INTEGRATION_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping NATS integration test (set NATS_INTEGRATION_TESTS=1 to enable)");
        return Ok(());
    }

    let harness = setup().await?;

    // Give the plugin a moment to flush its SUB to the server. The handler
    // pushes its reply via the consumer interface, and request() waits for
    // it. If the plugin's subscription never landed (issue #5074), the
    // request times out.
    let payload = b"ping".to_vec();
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        harness
            .nats_client
            .request(SUBSCRIPTION_SUBJECT, payload.clone().into()),
    )
    .await
    .context("request timed out — handler did not reply, subscription likely never registered")?
    .context("nats request failed")?;

    assert_eq!(
        response.payload.as_ref(),
        payload.as_slice(),
        "handler echoed wrong payload"
    );

    Ok(())
}

/// Asserts the plugin's subscription is registered on the NATS server itself,
/// independent of the request/reply round trip. This is the strongest possible
/// in-process check for issue #5074: the bug manifested as a SUB protocol
/// message never reaching the server, so we ask the server directly via its
/// HTTP monitoring endpoint (`/connz?subs=true`) whether `test.echo` shows up.
///
/// Without `client.flush()` in `on_workload_resolved`, this assertion is
/// timing-dependent on slow runners; with the flush, it must be true by the
/// time `workload_start` returns.
#[tokio::test]
async fn test_nats_messaging_subscription_registered_on_server() -> Result<()> {
    if std::env::var("NATS_INTEGRATION_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping NATS integration test (set NATS_INTEGRATION_TESTS=1 to enable)");
        return Ok(());
    }

    let harness = setup().await?;

    // Poll briefly to absorb the small lag between connect and the server's
    // /connz view becoming consistent. Each attempt fetches the full
    // connection list and walks every subscription on every connection.
    let connz_url = format!("{}/connz?subs=true", harness.monitoring_url);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let body = reqwest::get(&connz_url)
            .await
            .context("failed to fetch /connz from NATS monitoring")?
            .text()
            .await
            .context("failed to read /connz response body")?;
        // The subject appears literally inside the JSON `"subscriptions_list"`
        // arrays; substring is sufficient and avoids dragging in a JSON dep.
        if body.contains(SUBSCRIPTION_SUBJECT) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "subscription `{SUBSCRIPTION_SUBJECT}` never appeared in NATS /connz \
                 after workload_start returned (regression of #5074?). last response:\n{body}"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
