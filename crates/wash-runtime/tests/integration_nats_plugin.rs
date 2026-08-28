//! Integration tests for the `wasmcloud:nats` host plugin.
//!
//! Requires Docker (NATS); marked `#[ignore]`, run with `cargo test --include-ignored`.
//!
//! Covers the properties the plugin exists to provide: per-workload
//! connections, subject/stream/bucket grants, auto-ack and redelivery, and
//! refusal to bind on a misconfiguration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt as _;
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    plugin::wasmcloud_nats::{NatsBindings, WasmcloudNats, WorkloadConfig},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

/// Starts a workload expected to be refused, returning the refusal message.
async fn expect_refused(host: &impl HostApi, request: WorkloadStartRequest) -> Result<String> {
    let response = host.workload_start(request).await?;
    anyhow::ensure!(
        response.workload_status.workload_state == WorkloadState::Error,
        "expected the workload to be refused, got {:?}",
        response.workload_status
    );
    Ok(response.workload_status.message)
}

/// A P3 guest exporting the JetStream and core handlers.
const NATS_HANDLER_WASM: &[u8] = include_bytes!("wasm/nats_async_handler_p3.wasm");
/// Imports `wasmcloud:nats/core@0.1.0` twice, under `hub` and `leaf`.
const NATS_BRIDGE_WASM: &[u8] = include_bytes!("wasm/nats_implements_p3.wasm");

const STREAM: &str = "TESTS";
const COUNTS_BUCKET: &str = "test-counts";

struct Harness {
    nats_url: String,
    client: async_nats::Client,
    js: async_nats::jetstream::Context,
    _container: ContainerAsync<GenericImage>,
}

async fn start_nats() -> Result<Harness> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start NATS container: {e}"))?;

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("failed to get NATS port: {e}"))?;
    let nats_url = format!("nats://127.0.0.1:{port}");

    // The log-scraping wait strategy returns before the listener is accepting
    // on some Docker backends, so poll the port rather than trusting it.
    let client = connect_with_retry(&nats_url, Duration::from_secs(30)).await?;
    let js = async_nats::jetstream::new(client.clone());

    js.create_stream(async_nats::jetstream::stream::Config {
        name: STREAM.to_string(),
        subjects: vec![
            "test.orders.>".to_string(),
            "test.results".to_string(),
            "other.>".to_string(),
        ],
        ..Default::default()
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to create stream: {e}"))?;

    js.create_key_value(async_nats::jetstream::kv::Config {
        bucket: COUNTS_BUCKET.to_string(),
        history: 10,
        ..Default::default()
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to create kv bucket: {e}"))?;

    Ok(Harness {
        nats_url,
        client,
        js,
        _container: container,
    })
}

/// Connects once the server is actually accepting, or gives up at `budget`.
async fn connect_with_retry(url: &str, budget: Duration) -> Result<async_nats::Client> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut last: Option<async_nats::ConnectError> = None;
    while tokio::time::Instant::now() < deadline {
        match async_nats::connect(url).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    match last {
        Some(e) => Err(anyhow::anyhow!("NATS never became reachable at {url}: {e}")),
        None => Err(anyhow::anyhow!("NATS never became reachable at {url}")),
    }
}

/// Builds the interface binding a workload is deployed with.
fn nats_interface(config: &[(&str, &str)]) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "nats".to_string(),
        interfaces: [
            "types".to_string(),
            "jetstream".to_string(),
            "kv".to_string(),
            "jetstream-handler".to_string(),
        ]
        .into_iter()
        .collect(),
        version: Some(semver::Version::new(0, 1, 0)),
        config: config
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        name: None,
    }
}

/// A binding with the core handler included so
/// the P3 fixture's second export is served too.
fn nats_async_interface(config: &[(&str, &str)]) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "nats".to_string(),
        interfaces: [
            "types".to_string(),
            "core".to_string(),
            "jetstream".to_string(),
            "kv".to_string(),
            "jetstream-handler".to_string(),
            "core-handler".to_string(),
        ]
        .into_iter()
        .collect(),
        version: Some(semver::Version::new(0, 1, 0)),
        config: config
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        name: None,
    }
}

fn workload_request(workload_id: &str, interface: WitInterface) -> WorkloadStartRequest {
    workload_request_with_pool(workload_id, interface, 1)
}

/// A workload whose component declares `pool_size` explicitly, so a test can
/// compare pooled and ephemeral delivery. `0` is the ephemeral default.
fn workload_request_with_pool(
    workload_id: &str,
    interface: WitInterface,
    pool_size: i32,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: format!("nats-{workload_id}"),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "nats-handler".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(NATS_HANDLER_WASM),
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
                pool_size,
                max_invocations: 100,
                max_concurrency: 4,
            }],
            host_interfaces: vec![interface],
            volumes: vec![],
        },
    }
}

async fn start_host() -> Result<impl HostApi> {
    start_host_with(NatsBindings::new()).await
}

/// A host that declares its own bindings, the way `wash host` does.
async fn start_host_with(defaults: NatsBindings) -> Result<impl HostApi> {
    let engine = Engine::builder().build()?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(
            WasmcloudNats::new()
                .with_bindings(defaults)
                .with_lattice_prefixes(vec![
                    "runtime.host.".to_string(),
                    "runtime.operator.".to_string(),
                ]),
        ))?
        .build()?;
    host.start().await.context("failed to start host")
}

/// The config map an operator writes for one declared binding.
fn declared(config: &[(&str, &str)]) -> HashMap<String, String> {
    config
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Collects result lines the fixture publishes, up to `count` or the deadline.
async fn collect_results(client: &async_nats::Client, count: usize) -> Result<Vec<String>> {
    let mut sub = client.subscribe("test.results").await?;
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while out.len() < count {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, sub.next()).await {
            Ok(Some(msg)) => out.push(String::from_utf8_lossy(&msg.payload).to_string()),
            Ok(None) | Err(_) => break,
        }
    }
    Ok(out)
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn delivers_and_auto_acks() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-deliver",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start workload")?;

    // Give the subscription time to attach before publishing.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 2).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.new", "order-1".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results.iter().any(|r| r.starts_with("handled:order-1")),
        "expected the handler to process order-1, got {results:?}"
    );
    assert!(
        results.iter().any(|r| r == "count:order-1:1"),
        "expected first delivery to be counted once, got {results:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn handler_error_naks_and_redelivers() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-redeliver",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            ("ack-mode", "auto"),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 3).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.bad", "fail:once".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    // An `Err` from the handler naks under auto, so the same body is counted
    // more than once rather than stalling until ack-wait expires.
    let redeliveries = results
        .iter()
        .filter(|r| r.starts_with("count:fail:once"))
        .count();
    assert!(
        redeliveries >= 2,
        "expected a nak to redeliver, got {results:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn subject_outside_grant_is_denied() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-denied",
        nats_interface(&[
            ("servers", &h.nats_url),
            // Only test.> is granted, so test.results works but other.> does not.
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 2).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.probe", "denied:other.subject".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results
            .iter()
            .any(|r| r == "denied-probe:denied:not-granted:subject:other.subject"),
        "expected the ungranted subject to be refused by name, got {results:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn reserved_subjects_are_denied_even_with_a_broad_grant() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-reserved",
        nats_interface(&[
            ("servers", &h.nats_url),
            // A grant of `>` still must not reach the JetStream API directly.
            ("subject-allow", ">"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 2).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.probe", "denied:$JS.API.STREAM.LIST".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results
            .iter()
            .any(|r| r.starts_with("denied-probe:denied:reserved:subject:$JS.API")),
        "a `>` grant must not reach $JS.API, got {results:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn wildcard_publish_cannot_widen_a_grant() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    // A grant of `other.*` must not let a publish to `other.>` through: the
    // request would otherwise be matched by the grant's own wildcard.
    host.workload_start(workload_request(
        "wl-wildcard",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>,other.*"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 2).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.probe", "denied:other.>".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results
            .iter()
            .any(|r| r == "denied-probe:denied:wildcard-not-allowed:subject:other.>"),
        "a wildcard publish must not be matched against the grant, got {results:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn missing_servers_fails_the_deployment() -> Result<()> {
    let _h = start_nats().await?;
    let host = start_host().await?;

    let message = expect_refused(
        &host,
        workload_request(
            "wl-noservers",
            nats_interface(&[("subject-allow", "test.>")]),
        ),
    )
    .await?;

    assert!(
        message.contains("servers"),
        "expected the refusal to name the missing key, got {message}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn subscription_outside_grant_fails_the_deployment() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    // Subscribing to a stream that was never granted must fail at deploy, not
    // start a workload that silently receives nothing.
    let message = expect_refused(
        &host,
        workload_request(
            "wl-badsub",
            nats_interface(&[
                ("servers", &h.nats_url),
                ("subject-allow", "test.>"),
                ("stream-allow", "SOMETHING-ELSE"),
                (
                    "jetstream-subscriptions",
                    &format!("{STREAM}:test.orders.>:all"),
                ),
            ]),
        ),
    )
    .await?;

    assert!(
        message.contains("stream grant"),
        "expected the refusal to explain the refused stream, got {message}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn conflicting_credentials_fail_the_deployment() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    let message = expect_refused(
        &host,
        workload_request(
            "wl-badcreds",
            nats_interface(&[
                ("servers", &h.nats_url),
                ("subject-allow", "test.>"),
                ("token", "t"),
                ("username", "u"),
                ("password", "p"),
            ]),
        ),
    )
    .await?;

    assert!(
        message.contains("conflicting NATS credentials"),
        "expected a credential conflict refusal, got {message}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn each_workload_is_held_to_its_own_grant() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    // Same servers, same everything except the grant. If connections or policy
    // were shared across workloads, the narrow one would inherit the broad
    // one's rights.
    host.workload_start(workload_request(
        "wl-broad",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>,other.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.broad:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start wl-broad")?;

    host.workload_start(workload_request(
        "wl-narrow",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.narrow:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start wl-narrow")?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 4).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    for subject in ["test.orders.broad", "test.orders.narrow"] {
        h.js.publish(subject, "denied:other.thing".into())
            .await
            .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
            .await
            .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;
    }

    let results = results.await??;
    let probes: Vec<&String> = results
        .iter()
        .filter(|r| r.starts_with("denied-probe:"))
        .collect();

    assert!(
        probes.iter().any(|r| r.as_str() == "denied-probe:allowed"),
        "the broad grant should reach other.thing, got {results:?}"
    );
    assert!(
        probes
            .iter()
            .any(|r| r.as_str() == "denied-probe:denied:not-granted:subject:other.thing"),
        "the narrow grant must not inherit the broad one, got {results:?}"
    );

    for id in ["wl-broad", "wl-narrow"] {
        host.workload_stop(wash_runtime::types::WorkloadStopRequest {
            workload_id: id.to_string(),
        })
        .await
        .with_context(|| format!("failed to stop {id}"))?;
    }
    Ok(())
}

/// The package delivers into a P3 component whose handler is an
/// `async fn` awaiting imported NATS calls. This is what `@0.1.0` cannot do: a
/// sync-signature export cannot be lifted with the async canonical ABI.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn async_p3_handler_delivers_and_auto_acks() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-async-deliver",
        nats_async_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start the async workload")?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 2).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.new", "async-order-1".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results
            .iter()
            .any(|r| r.starts_with("handled:async-order-1")),
        "expected the async handler to process async-order-1, got {results:?}"
    );
    assert!(
        results.iter().any(|r| r == "count:async-order-1:1"),
        "expected the async KV round trip to count one delivery, got {results:?}"
    );
    Ok(())
}

/// A denial reaches the async guest as the structured `denied` variant, naming
/// both the reason and the kind of name refused.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn async_p3_denial_carries_reason_and_target() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-async-denied",
        nats_async_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start the async workload")?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    let results = tokio::spawn({
        let client = h.client.clone();
        async move { collect_results(&client, 1).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    h.js.publish("test.orders.new", "denied:other.subject".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    let results = results.await??;
    assert!(
        results
            .iter()
            .any(|r| r == "denied-probe:denied:not-granted:subject:other.subject"),
        "expected a structured denial naming the subject, got {results:?}"
    );
    Ok(())
}

/// A JetStream subscription whose filter subject sits outside `subject-allow`
/// is refused at bind: a stream grant alone would deliver whatever that stream
/// captures.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn jetstream_filter_outside_the_subject_grant_is_refused() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    let message = expect_refused(
        &host,
        workload_request(
            "wl-filter-denied",
            nats_interface(&[
                ("servers", &h.nats_url),
                ("subject-allow", "test.payments.>"),
                ("stream-allow", STREAM),
                (
                    "jetstream-subscriptions",
                    &format!("{STREAM}:test.orders.>:all"),
                ),
            ]),
        ),
    )
    .await?;
    assert!(
        message.contains("subject grant"),
        "expected the filter subject to be refused, got {message}"
    );
    Ok(())
}

/// A JetStream push subscription rebuilds when its delivery path dies.
///
/// Deleting the consumer and its stream is what a server restart does to an
/// ephemeral consumer: the client stays subscribed to a deliver subject
/// nothing publishes to any more. Before the rebuild path the subscription
/// parked forever — no error, no log, no recovery.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn jetstream_subscription_rebuilds_when_delivery_dies() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-rebuild",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:new"),
            ),
        ]),
    ))
    .await
    .context("failed to start workload")?;

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Establish a delivery position first: a subscription that never consumed
    // anything has nothing to resume from, and `deliver-policy: new` still
    // means new.
    let mut sub = h.client.subscribe("test.results").await?;
    h.js.publish("test.orders.new", "before-the-outage".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;
    timeout(Duration::from_secs(20), sub.next())
        .await
        .map_err(|_| anyhow::anyhow!("the first message was never handled"))?;

    // Take the stream out from under the live subscription, then put it back
    // as out-of-band provisioning would.
    h.js.delete_stream(STREAM)
        .await
        .map_err(|e| anyhow::anyhow!("failed to delete stream: {e}"))?;
    h.js.create_stream(async_nats::jetstream::stream::Config {
        name: STREAM.to_string(),
        subjects: vec![
            "test.orders.>".to_string(),
            "test.results".to_string(),
            "other.>".to_string(),
        ],
        ..Default::default()
    })
    .await
    .map_err(|e| anyhow::anyhow!("failed to recreate stream: {e}"))?;

    // Published while there is no consumer at all. Under the configured
    // `deliver-policy: new` a rebuilt consumer would never see this, so the
    // rebuild has to resume from what the stream holds instead.
    h.js.publish("test.orders.new", "during-the-outage".into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;

    // Recovery waits out the idle-heartbeat timeout, so wait for the delivery
    // rather than guessing a single sleep.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut seen = None;
    while tokio::time::Instant::now() < deadline && seen.is_none() {
        if let Ok(Some(msg)) = timeout(Duration::from_secs(5), sub.next()).await {
            let body = String::from_utf8_lossy(&msg.payload).to_string();
            if body.starts_with("handled:during-the-outage") {
                seen = Some(body);
            }
        }
    }

    assert!(
        seen.is_some(),
        "expected the message published during the outage to be delivered once \
         the subscription rebuilt"
    );
    Ok(())
}

/// Reads result lines until one starts with `prefix`, or the budget runs out.
async fn wait_for_result(
    sub: &mut async_nats::Subscriber,
    prefix: &str,
    budget: Duration,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match timeout(remaining, sub.next()).await {
            Ok(Some(msg)) => {
                let body = String::from_utf8_lossy(&msg.payload).to_string();
                if body.starts_with(prefix) {
                    return Some(body);
                }
            }
            Ok(None) | Err(_) => return None,
        }
    }
}

/// The delivery number the fixture reported in a `count:<body>:<n>` line.
fn reported_count(line: &str) -> u32 {
    line.rsplit(':')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Takes the highest `count:<body>:<n>` already queued on the subscription,
/// leaving nothing buffered that a later wait could mistake for fresh traffic.
async fn drain_counts(sub: &mut async_nats::Subscriber, prefix: &str, highest: u32) -> u32 {
    let mut highest = highest;
    while let Ok(Some(msg)) = timeout(Duration::from_millis(250), sub.next()).await {
        let body = String::from_utf8_lossy(&msg.payload).to_string();
        if body.starts_with(prefix) {
            highest = highest.max(reported_count(&body));
        }
    }
    highest
}

/// Publishes to the stream and waits for the publish ack.
async fn publish_to_stream(h: &Harness, subject: &str, body: &str) -> Result<()> {
    h.js.publish(subject.to_string(), body.to_owned().into())
        .await
        .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
        .await
        .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_nakd_message_survives_a_consumer_rebuild() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-nak-rebuild",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            ("ack-mode", "auto"),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:new"),
            ),
        ]),
    ))
    .await
    .context("failed to start workload")?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut sub = h.client.subscribe("test.results").await?;

    // The fixture fails on every delivery of this body, so it is nak'd and
    // never settled: the subscription has to keep resuming at it.
    publish_to_stream(&h, "test.orders.bad", "fail:always").await?;
    let first = wait_for_result(&mut sub, "count:fail:always:", Duration::from_secs(20))
        .await
        .context("the failing message was never delivered")?;

    // A message the handler does settle, so the delivery position moves past
    // the nak'd one — without that there is nothing for a rebuild to skip.
    publish_to_stream(&h, "test.orders.new", "after-the-nak").await?;
    wait_for_result(&mut sub, "handled:after-the-nak", Duration::from_secs(20))
        .await
        .context("the second message was never handled")?;

    // Drop the consumer the way a server restart would: an ephemeral push
    // consumer takes its ack state — including the pending redelivery of the
    // nak'd message — with it, so only the host's own in-flight tracking can
    // bring that message back.
    // Baseline on everything already delivered, buffered redeliveries included:
    // counted from a stale baseline, a backlog on a slow machine would satisfy
    // the assertion without a rebuild ever happening.
    let before = drain_counts(&mut sub, "count:fail:always:", reported_count(&first)).await;

    let stream =
        h.js.get_stream(STREAM)
            .await
            .map_err(|e| anyhow::anyhow!("failed to get stream: {e}"))?;
    let mut names = stream.consumer_names();
    let mut doomed = Vec::new();
    while let Some(name) = names.next().await {
        doomed.push(name.map_err(|e| anyhow::anyhow!("failed to list consumers: {e}"))?);
    }
    anyhow::ensure!(!doomed.is_empty(), "expected the subscription's consumer");
    for name in doomed {
        stream
            .delete_consumer(&name)
            .await
            .map_err(|e| anyhow::anyhow!("failed to delete consumer '{name}': {e}"))?;
    }

    // Recovery waits out the idle-heartbeat timeout, and the redelivery backoff
    // grows with the delivery count, so give it room. Two further deliveries,
    // not one, so a redelivery already in flight when the consumer went away
    // cannot pass for a rebuild that resumed correctly.
    let target = before + 2;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    let mut highest = before;
    while tokio::time::Instant::now() < deadline && highest < target {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match wait_for_result(&mut sub, "count:fail:always:", remaining).await {
            Some(line) => highest = highest.max(reported_count(&line)),
            None => break,
        }
    }

    assert!(
        highest >= target,
        "expected the nak'd message to come round again after the rebuild \
         (saw delivery {highest}, wanted {target}); a sequence released before \
         the server settled it is skipped by the rebuilt consumer"
    );
    Ok(())
}

/// A NATS container with nothing provisioned on it: the multi-cluster tests
/// only need core pub/sub, and a second JetStream-enabled server would just be
/// slower to start.
async fn start_bare_nats() -> Result<(String, async_nats::Client, ContainerAsync<GenericImage>)> {
    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start NATS container: {e}"))?;
    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("failed to get NATS port: {e}"))?;
    let url = format!("nats://127.0.0.1:{port}");
    let client = connect_with_retry(&url, Duration::from_secs(30)).await?;
    Ok((url, client, container))
}

/// A binding of the async package under an `(implements ..)` name.
fn named_nats_interface(name: &str, interfaces: &[&str], config: &[(&str, &str)]) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "nats".to_string(),
        interfaces: interfaces.iter().map(|i| (*i).to_string()).collect(),
        version: Some(semver::Version::new(0, 1, 0)),
        config: config
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        name: Some(name.to_string()),
    }
}

/// The bridge component: `wasmcloud:nats/core` imported twice, under `hub` and
/// `leaf`, plus the `core-handler` export the leaf subscription dispatches into.
fn bridge_workload_request(
    workload_id: &str,
    interfaces: Vec<WitInterface>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: format!("nats-{workload_id}"),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "nats-bridge".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(NATS_BRIDGE_WASM),
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
                max_concurrency: 4,
            }],
            host_interfaces: interfaces,
            volumes: vec![],
        },
    }
}

/// Waits for one message on `subject`, or gives up at `budget`.
async fn await_one(client: &async_nats::Client, subject: &str, budget: Duration) -> Option<String> {
    let mut sub = client.subscribe(subject.to_string()).await.ok()?;
    match timeout(budget, sub.next()).await {
        Ok(Some(msg)) => Some(String::from_utf8_lossy(&msg.payload).to_string()),
        _ => None,
    }
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn two_named_bindings_reach_two_clusters() -> Result<()> {
    let (hub_url, hub_client, _hub) = start_bare_nats().await?;
    let (leaf_url, leaf_client, _leaf) = start_bare_nats().await?;
    let host = start_host().await?;

    // One component, two `wasmcloud:nats` bindings: `hub` publishes to one
    // server, `leaf` subscribes on and publishes to the other.
    host.workload_start(bridge_workload_request(
        "wl-bridge",
        vec![
            named_nats_interface(
                "hub",
                &["types", "core"],
                &[("servers", &hub_url), ("subject-allow", "bridge.>")],
            ),
            named_nats_interface(
                "leaf",
                &["types", "core", "core-handler"],
                &[
                    ("servers", &leaf_url),
                    ("subject-allow", "bridge.>,trigger.go"),
                    ("core-subscriptions", "trigger.go"),
                ],
            ),
        ],
    ))
    .await
    .context("failed to start the bridge workload")?;

    // Subscribe before triggering, then re-trigger on a timer: core NATS drops
    // a message published before the workload's subscription has attached, and
    // how long that takes depends on what else the machine is doing.
    let mut hub_sub = hub_client.subscribe("bridge.hub".to_string()).await?;
    let mut leaf_sub = leaf_client.subscribe("bridge.leaf".to_string()).await?;
    let triggering = tokio::spawn({
        let client = leaf_client.clone();
        async move {
            loop {
                let _ = client
                    .publish("trigger.go".to_string(), "over-the-leafnode".into())
                    .await;
                let _ = client.flush().await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    let budget = Duration::from_secs(45);
    let hub_seen = timeout(budget, hub_sub.next())
        .await
        .ok()
        .flatten()
        .map(|msg| String::from_utf8_lossy(&msg.payload).to_string());
    let leaf_seen = timeout(budget, leaf_sub.next())
        .await
        .ok()
        .flatten()
        .map(|msg| String::from_utf8_lossy(&msg.payload).to_string());
    triggering.abort();

    assert_eq!(
        hub_seen.as_deref(),
        Some("hub:over-the-leafnode"),
        "the `hub` label should publish to the hub server"
    );
    assert_eq!(
        leaf_seen.as_deref(),
        Some("leaf:over-the-leafnode"),
        "the `leaf` label should publish to the leaf server"
    );

    // Each label really is its own cluster: neither subject exists on the other
    // server, so a single shared connection would have failed one of the two
    // assertions above.
    assert!(
        await_one(&hub_client, "bridge.leaf", Duration::from_secs(1))
            .await
            .is_none(),
        "the leaf publish must not appear on the hub server"
    );
    Ok(())
}

/// A burst that stays inside `subscription-capacity` must not lose anything.
///
/// The delivery loop cannot both wait for a handler permit and keep reading the
/// subscription, so it does not try: a reader task drains the client's channel
/// into a host-side backlog this loop takes from. That is a change to the path
/// every core delivery travels, and the property it must not break is this one
/// — under capacity, with the handler pool the bottleneck, nothing is shed.
///
/// The burst is deliberately wider than `max-in-flight`, so most of it is
/// sitting in the backlog rather than in a handler when the next message
/// arrives. A run that shed anything would come up short, and the count is
/// exact rather than a threshold because that is the whole claim.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_burst_under_capacity_loses_nothing() -> Result<()> {
    const BURST: usize = 100;

    let (url, client, _nats) = start_bare_nats().await?;
    let host = start_host().await?;

    // Both labels point at the one server: this test is about the subscription
    // path, not about which cluster a publish lands in.
    host.workload_start(bridge_workload_request(
        "wl-burst",
        vec![
            named_nats_interface(
                "hub",
                &["types", "core"],
                &[("servers", &url), ("subject-allow", "bridge.>")],
            ),
            named_nats_interface(
                "leaf",
                &["types", "core", "core-handler"],
                &[
                    ("servers", &url),
                    ("subject-allow", "bridge.>,trigger.go"),
                    ("core-subscriptions", "trigger.go"),
                ],
            ),
        ],
    ))
    .await
    .context("failed to start the burst workload")?;

    let mut leaf = client.subscribe("bridge.leaf".to_string()).await?;

    // Core NATS drops what is published before a subscription attaches, so the
    // burst cannot start until one round trip has actually completed. Retry
    // until the workload answers, then take that answer off the wire.
    let mut attached = false;
    for _ in 0..45 {
        client
            .publish("trigger.go".to_string(), "warmup".into())
            .await?;
        client.flush().await?;
        if timeout(Duration::from_secs(1), leaf.next()).await.is_ok() {
            attached = true;
            break;
        }
    }
    assert!(attached, "the workload never attached its subscription");

    // Warmups already in flight would otherwise be counted as burst messages.
    while timeout(Duration::from_millis(500), leaf.next())
        .await
        .is_ok()
    {}

    for n in 0..BURST {
        client
            .publish("trigger.go".to_string(), format!("burst-{n}").into())
            .await?;
    }
    client.flush().await?;

    let mut seen = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while seen < BURST {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, leaf.next()).await {
            Ok(Some(_)) => seen += 1,
            Ok(None) | Err(_) => break,
        }
    }

    assert_eq!(
        seen, BURST,
        "a {BURST}-message burst is well inside the default 1024-message \
         subscription capacity, so every one of them should have been delivered"
    );
    Ok(())
}

/// Drives the async fixture's pull probe and returns the line it reports.
async fn pull_probe(client: &async_nats::Client, spec: &str) -> Result<String> {
    let mut results = client.subscribe("test.results".to_string()).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    client
        .publish("probe.pull".to_string(), format!("pull:{spec}").into())
        .await?;
    client.flush().await?;
    wait_for_result(&mut results, "pull:", Duration::from_secs(20))
        .await
        .context("the fixture never reported a pull outcome")
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_fetch_over_the_consumer_limit_is_refused_not_reported_empty() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    // A consumer that refuses any pull asking for more than 5 messages. The
    // server answers those with `409 Exceeded MaxRequestBatch`, which used to
    // reach the guest as `no-messages` — indistinguishable from an idle
    // consumer, and unfixable from the guest side.
    let stream =
        h.js.get_stream(STREAM)
            .await
            .map_err(|e| anyhow::anyhow!("failed to get stream: {e}"))?;
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("limited".to_string()),
            filter_subject: "test.orders.>".to_string(),
            max_batch: 5,
            max_bytes: 4096,
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to create consumer: {e}"))?;

    for n in 0..8 {
        h.js.publish("test.orders.pull".to_string(), format!("order-{n}").into())
            .await
            .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
            .await
            .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;
    }

    host.workload_start(workload_request(
        "wl-pull-limits",
        nats_async_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>,probe.>"),
            ("stream-allow", STREAM),
            ("bucket-allow", COUNTS_BUCKET),
            ("core-subscriptions", "probe.pull"),
        ]),
    ))
    .await
    .context("failed to start workload")?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Over `max-request-batch`: a typed refusal, carrying the server's wording.
    let refused = pull_probe(&h.client, &format!("{STREAM}:limited:10")).await?;
    assert!(
        refused.starts_with("pull:err:limit-exceeded:"),
        "an over-limit fetch should be refused, got {refused:?}"
    );
    assert!(
        refused.contains("MaxRequestBatch"),
        "the refusal should carry the server's reason, got {refused:?}"
    );
    // And `info` reports the limits the guest needs to size the next request.
    assert!(
        refused.contains("max-request-batch=5") && refused.contains("max-request-max-bytes=4096"),
        "consumer-info should report both request limits, got {refused:?}"
    );

    // Within the limits: messages come back, and the batch says why it ended.
    let ok = pull_probe(&h.client, &format!("{STREAM}:limited:5")).await?;
    assert!(
        ok.starts_with("pull:ok:5:batch-filled:"),
        "a fetch inside the limits should fill the batch, got {ok:?}"
    );

    // Byte-bounded: the server closes the batch early, and that is now visible
    // rather than looking like a drained consumer.
    let capped = pull_probe(&h.client, &format!("{STREAM}:limited:5:100")).await?;
    assert!(
        capped.starts_with("pull:ok:") && capped.contains(":byte-limit:"),
        "a byte-capped batch should report `byte-limit`, got {capped:?}"
    );
    Ok(())
}

/// The shape this plugin is meant to be operated in: the host declares what
/// each binding is — where it points and what it may reach — and the workload's
/// manifest only names the binding and says which subject it wants delivered.
/// Nothing in the manifest could have widened its own reach.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn declared_bindings_serve_a_manifest_that_only_asks() -> Result<()> {
    let (hub_url, hub_client, _hub) = start_bare_nats().await?;
    let (leaf_url, leaf_client, _leaf) = start_bare_nats().await?;

    let host = start_host_with(
        NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_binding(
                "hub",
                declared(&[("servers", &hub_url), ("subject-allow", "bridge.>")]),
            )
            .with_binding(
                "leaf",
                declared(&[
                    ("servers", &leaf_url),
                    ("subject-allow", "bridge.>,trigger.go"),
                ]),
            ),
    )
    .await?;

    host.workload_start(bridge_workload_request(
        "wl-declared-bridge",
        vec![
            named_nats_interface("hub", &["types", "core"], &[]),
            named_nats_interface(
                "leaf",
                &["types", "core", "core-handler"],
                // All the manifest still says: which subject this handler wants.
                &[("core-subscriptions", "trigger.go")],
            ),
        ],
    ))
    .await
    .context("failed to start the bridge workload")?;

    let mut hub_sub = hub_client.subscribe("bridge.hub".to_string()).await?;
    let mut leaf_sub = leaf_client.subscribe("bridge.leaf".to_string()).await?;
    let triggering = tokio::spawn({
        let client = leaf_client.clone();
        async move {
            loop {
                let _ = client
                    .publish("trigger.go".to_string(), "over-the-leafnode".into())
                    .await;
                let _ = client.flush().await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    let budget = Duration::from_secs(45);
    let hub_seen = timeout(budget, hub_sub.next())
        .await
        .ok()
        .flatten()
        .map(|msg| String::from_utf8_lossy(&msg.payload).to_string());
    let leaf_seen = timeout(budget, leaf_sub.next())
        .await
        .ok()
        .flatten()
        .map(|msg| String::from_utf8_lossy(&msg.payload).to_string());
    triggering.abort();

    assert_eq!(
        hub_seen.as_deref(),
        Some("hub:over-the-leafnode"),
        "the host's `hub` declaration should carry the workload to the hub server"
    );
    assert_eq!(
        leaf_seen.as_deref(),
        Some("leaf:over-the-leafnode"),
        "and its `leaf` declaration to the leaf server"
    );
    Ok(())
}

/// A manifest cannot hand itself a grant the operator withheld. The refusal is
/// at deploy, naming the key, rather than a workload that starts and then has
/// every call denied one at a time.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_manifest_cannot_widen_a_declared_grant() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host_with(
        NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec![h.nats_url.clone()])
            .with_base(declared(&[("subject-allow", "test.orders.>")])),
    )
    .await?;

    let message = expect_refused(
        &host,
        workload_request(
            "wl-selfgrant",
            nats_interface(&[("subject-allow", "test.>"), ("stream-allow", STREAM)]),
        ),
    )
    .await?;

    assert!(
        message.contains("`subject-allow`") && message.contains("`stream-allow`"),
        "expected the refusal to name the keys the host does not accept, got {message}"
    );
    Ok(())
}

/// Asking for a binding the host does not serve is a deployment error, not a
/// workload started against the right cluster with an empty grant.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn asking_for_an_undeclared_binding_fails_the_deployment() -> Result<()> {
    let (hub_url, _hub_client, _hub) = start_bare_nats().await?;
    let host = start_host_with(
        NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_binding("hub", declared(&[("servers", &hub_url)])),
    )
    .await?;

    let message = expect_refused(
        &host,
        bridge_workload_request(
            "wl-undeclared",
            vec![
                named_nats_interface("hub", &["types", "core"], &[]),
                named_nats_interface("leaf", &["types", "core", "core-handler"], &[]),
            ],
        ),
    )
    .await?;

    assert!(
        message.contains("leaf"),
        "expected the refusal to name the binding asked for, got {message}"
    );
    Ok(())
}

/// Drives the fixture's `warm:` marker over JetStream and returns what the
/// guest reported for each run, in order.
///
/// JetStream rather than core, deliberately: a core trigger published before
/// the workload's SUB has reached the server is dropped silently, which made
/// the first delivery a race against machine load. A JetStream publish is
/// stored and delivered whenever the consumer attaches, so the trigger cannot
/// be lost — and the delivery counter the test asserts on stays exact.
async fn warm_probe(h: &Harness, runs: &[&str]) -> Result<Vec<String>> {
    let mut results = h.client.subscribe("test.results".to_string()).await?;
    let mut out = Vec::new();
    for run in runs {
        h.js.publish("test.orders.warm".to_string(), format!("warm:{run}").into())
            .await
            .map_err(|e| anyhow::anyhow!("publish failed: {e}"))?
            .await
            .map_err(|e| anyhow::anyhow!("publish ack failed: {e}"))?;
        let got = timeout(Duration::from_secs(15), results.next())
            .await
            .context("no result before the deadline")?
            .context("results subscription ended")?;
        out.push(String::from_utf8_lossy(&got.payload).to_string());
        // The guest publishes its result from *inside* the handler, so the
        // result can arrive here before the host has parked the instance.
        // Give the park a moment, or the next delivery races it for a cold
        // store and the counter this test asserts on goes nondeterministic.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(out)
}

/// `poolSize` now applies to NATS deliveries: a component that opted in serves
/// consecutive messages from one instance, so what it built in linear memory
/// survives between them. The fixture counts deliveries in a `static`; only a
/// reused instance can ever report 2.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn a_pooled_component_keeps_state_across_deliveries() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request(
        "wl-warm",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
    ))
    .await
    .context("failed to start the pooled workload")?;

    let got = warm_probe(&h, &["a", "b", "c"]).await?;
    assert_eq!(
        got,
        vec!["warm:a:1", "warm:b:2", "warm:c:3"],
        "each delivery should have found the previous delivery's linear memory"
    );
    Ok(())
}

/// The control for the test above: a component that did not opt in keeps
/// nothing, exactly as before pooling existed on this path.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn an_ephemeral_component_starts_fresh_every_delivery() -> Result<()> {
    let h = start_nats().await?;
    let host = start_host().await?;

    host.workload_start(workload_request_with_pool(
        "wl-cold",
        nats_interface(&[
            ("servers", &h.nats_url),
            ("subject-allow", "test.>"),
            ("stream-allow", STREAM),
            (
                "jetstream-subscriptions",
                &format!("{STREAM}:test.orders.>:all"),
            ),
        ]),
        0,
    ))
    .await
    .context("failed to start the ephemeral workload")?;

    let got = warm_probe(&h, &["a", "b"]).await?;
    assert_eq!(
        got,
        vec!["warm:a:1", "warm:b:1"],
        "an ephemeral component must never see a previous delivery's memory"
    );
    Ok(())
}
