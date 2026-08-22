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
    plugin::wasmcloud_nats::WasmcloudNats,
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

const NATS_HANDLER_WASM: &[u8] = include_bytes!("wasm/nats_jetstream_handler.wasm");
/// The P3 counterpart, bound against the async `@0.2.0` package.
const NATS_ASYNC_HANDLER_WASM: &[u8] = include_bytes!("wasm/nats_async_handler_p3.wasm");

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

/// A binding on the async `@0.2.0` package, with the core handler included so
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
        version: Some(semver::Version::new(0, 2, 0)),
        config: config
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        name: None,
    }
}

fn workload_request(workload_id: &str, interface: WitInterface) -> WorkloadStartRequest {
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
                bytes: bytes::Bytes::from_static(match interface.version {
                    Some(semver::Version { minor: 2, .. }) => NATS_ASYNC_HANDLER_WASM,
                    _ => NATS_HANDLER_WASM,
                }),
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
            host_interfaces: vec![interface],
            volumes: vec![],
        },
    }
}

async fn start_host() -> Result<impl HostApi> {
    let engine = Engine::builder().build()?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(WasmcloudNats::new().with_lattice_prefixes(vec![
            "runtime.host.".to_string(),
            "runtime.operator.".to_string(),
        ])))?
        .build()?;
    host.start().await.context("failed to start host")
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            .any(|r| r == "denied-probe:subject-denied:other.subject"),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            .any(|r| r.starts_with("denied-probe:subject-denied:$JS.API")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            .any(|r| r == "denied-probe:subject-denied:other.>"),
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
                ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.broad:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.narrow:all")),
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
            .any(|r| r.as_str() == "denied-probe:subject-denied:other.thing"),
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

/// The async `@0.2.0` package delivers into a P3 component whose handler is an
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
                ("subscriptions", &format!("{STREAM}:test.orders.>:all")),
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
            ("subscriptions", &format!("{STREAM}:test.orders.>:new")),
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
