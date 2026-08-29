//! A messaging-triggered component honours the instance limits it declares.
//!
//! `poolSize`, `maxConcurrency` and `maxInvocations` mean the same thing here
//! as they do for inbound HTTP: deliveries run on warm instances, several at a
//! time on one instance when the component asks for it, and an instance retires
//! at its invocation budget. Every one of them defaults to the conservative
//! value, so a limit that fails to reach the pool looks exactly like a limit
//! that reached it and was honoured — which is what these pin down.
//!
//! These are the messaging counterparts of `integration_instance_concurrency`.
//! The `messaging-sleeper-p3` fixture is that suite's `http-sleeper` with the
//! trigger swapped: its `wasmcloud:messaging/handler@0.3.0` parks on the clock
//! for the milliseconds the body names, and keeps `msg_peak` (the most
//! deliveries this instance had in flight at once) and `msg_served` (how many
//! it has handled) in its own linear memory. Both are reported over its HTTP
//! handler, which is what makes them readable: a probe is a call on the same
//! pool, so it lands on the instance the deliveries ran on — and a *fresh*
//! instance reports zeroes, which is how reuse shows.
//!
//! Every case publishes, waits out the delivery, and only then probes. Polling
//! while deliveries are in flight would be answered by whatever store the pool
//! could spare rather than by the instance under test, so the counters would
//! depend on timing rather than on the limits.
//!
//! Only `@0.3.0` is covered because only `@0.3.0` can be pooled: its
//! `handle-message` is an `async func` taking an `Accessor`, so deliveries
//! overlap on one instance. The sync `@0.2.0` export holds `&mut Store` for the
//! length of its call and keeps its store per message.
//!
//! The in-memory backend is used, so these need no Docker and run in the
//! default CI leg.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::wasmcloud_messaging::InMemoryMessaging;
use wash_runtime::plugin::{
    wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig, wasi_keyvalue::InMemoryKeyValue,
    wasi_logging::TracingLogger,
};
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_only_host_interfaces, json_u64_field};

const MSG_SLEEPER_WASM: &[u8] = include_bytes!("wasm/messaging_sleeper_p3.wasm");

/// How long each delivery parks, in the body the fixture parses. Long enough
/// that a burst is still in flight when the last of it arrives, short enough
/// not to pad the suite.
const DELIVERY_MILLIS: u64 = 300;

/// What [`Harness::settle`] waits: several times a delivery, plus room for the
/// fixture's one-off per-instance setup. Deliveries are spawned rather than
/// awaited by the publisher, and there is nothing else to synchronise on.
const SETTLE: Duration = Duration::from_millis(2_500);

/// What the fixture reports about the instance that answered the probe.
#[derive(Debug, Clone, Copy)]
struct Counters {
    /// Deliveries this instance had in flight at once, at its highest.
    msg_peak: u64,
    /// Deliveries this instance has handled.
    msg_served: u64,
    /// HTTP requests this instance has served, this probe included.
    served: u64,
}

struct Harness<H: HostApi> {
    _host: Arc<H>,
    addr: std::net::SocketAddr,
    messaging: Arc<InMemoryMessaging>,
    workload_id: String,
    host_header: &'static str,
    subject: &'static str,
    client: reqwest::Client,
}

/// Start the sleeper as a *component* (not a service) subscribed to `subject`,
/// with the instance limits under test.
async fn start_msg_sleeper(
    host_header: &'static str,
    subject: &'static str,
    pool_size: i32,
    max_concurrency: i32,
    max_invocations: i32,
) -> Result<Harness<impl HostApi>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let messaging = Arc::new(InMemoryMessaging::new());
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .with_plugin(messaging.clone())?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // Both host interfaces: messaging delivers the work, HTTP is how the
    // per-instance counters are read back.
    let mut host_interfaces = http_only_host_interfaces(host_header);
    host_interfaces.push(WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "messaging".to_string(),
        interfaces: ["handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::new(0, 3, 0)),
        config: HashMap::from([("subscriptions".to_string(), subject.to_string())]),
        name: None,
    });

    let workload_id = uuid::Uuid::new_v4().to_string();
    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host_header.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "sleeper".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(MSG_SLEEPER_WASM),
                local_resources: LocalResources {
                    config: HashMap::from([("subscriptions".to_string(), subject.to_string())]),
                    ..Default::default()
                },
                pool_size,
                max_invocations,
                max_concurrency,
                ..Default::default()
            }],
            host_interfaces,
            volumes: vec![],
        },
    })
    .await
    .context("sleeper workload should start")?;

    Ok(Harness {
        _host: host,
        addr,
        messaging,
        workload_id,
        host_header,
        subject,
        client: reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(20))
            .build()?,
    })
}

impl<H: HostApi> Harness<H> {
    async fn publish(&self) -> Result<()> {
        self.messaging
            .publish(
                &self.workload_id,
                self.subject,
                DELIVERY_MILLIS.to_string().into_bytes(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("publish failed: {e}"))
    }

    /// Publish `n` messages back to back, so their deliveries overlap.
    async fn publish_burst(&self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.publish().await?;
        }
        Ok(())
    }

    /// Wait for every delivery in flight to finish, so the next probe is
    /// answered by an idle instance rather than by a store the pool spared.
    async fn settle(&self) {
        tokio::time::sleep(SETTLE).await;
    }

    /// One HTTP request, reporting what the instance that served it has done.
    ///
    /// The probe is itself a call on the pool: it takes a warm instance when
    /// one is free, and counts against `max_invocations` like any other call.
    async fn probe(&self) -> Result<Counters> {
        let resp = self
            .client
            .get(format!("http://{}/", self.addr))
            .header("HOST", self.host_header)
            .send()
            .await?;
        anyhow::ensure!(resp.status().is_success(), "status {}", resp.status());
        let body = resp.text().await?;
        Ok(Counters {
            msg_peak: json_u64_field(&body, "msg_peak"),
            msg_served: json_u64_field(&body, "msg_served"),
            served: json_u64_field(&body, "served"),
        })
    }
}

/// The default. One warm instance, `max_concurrency` unset: it takes one
/// delivery at a time and the rest of the burst is served from stores of their
/// own, so no instance ever sees two at once.
#[tokio::test]
async fn without_max_concurrency_deliveries_do_not_overlap_on_an_instance() -> Result<()> {
    let h = start_msg_sleeper("msg-conc-off", "conc.off", 1, 1, 0).await?;

    h.publish_burst(4).await?;
    h.settle().await;

    let seen = h.probe().await?;
    assert_eq!(
        seen.msg_peak, 1,
        "an instance must serve one delivery at a time unless the component asked \
         for more, saw {seen:?}"
    );
    Ok(())
}

/// The opt-in. One warm instance with `max_concurrency: 8` takes all four
/// deliveries at once — all of them on the instance that had already paid its
/// setup, rather than three of them on fresh stores paying it again.
#[tokio::test]
async fn max_concurrency_overlaps_deliveries_on_one_instance() -> Result<()> {
    let h = start_msg_sleeper("msg-conc-on", "conc.on", 1, 8, 0).await?;

    h.publish_burst(4).await?;
    h.settle().await;

    let seen = h.probe().await?;
    assert_eq!(
        seen.msg_peak, 4,
        "all four deliveries should have been in flight on the one instance at \
         once, saw {seen:?}"
    );
    assert_eq!(
        seen.msg_served, 4,
        "and all four should have been served by it, saw {seen:?}"
    );
    Ok(())
}

/// Concurrency composes with `pool_size` rather than replacing it: two
/// instances at two deliveries each cover the burst, and neither exceeds its
/// own limit.
#[tokio::test]
async fn concurrency_is_bounded_per_instance() -> Result<()> {
    let h = start_msg_sleeper("msg-conc-bounded", "conc.bounded", 2, 2, 0).await?;

    h.publish_burst(4).await?;
    h.settle().await;

    let seen = h.probe().await?;
    assert!(
        (1..=2).contains(&seen.msg_peak),
        "no instance may exceed max_concurrency of 2, saw {seen:?}"
    );
    Ok(())
}

/// The property the configuration claimed and did not have: with `poolSize`
/// set, a second message reaches the instance the first one warmed, so
/// everything the guest built in linear memory is still there.
///
/// Published one at a time on purpose. A burst beyond `max_concurrency` falls
/// through to stores of its own, which is the saturation rule rather than the
/// reuse this is about.
#[tokio::test]
async fn pool_size_keeps_guest_state_between_messages() -> Result<()> {
    let h = start_msg_sleeper("msg-warm", "warm.state", 1, 1, 0).await?;

    h.publish().await?;
    h.settle().await;
    let first = h.probe().await?;
    assert_eq!(
        first.msg_served, 1,
        "the first message should have been handled, saw {first:?}"
    );

    h.publish().await?;
    h.settle().await;
    let second = h.probe().await?;

    assert_eq!(
        second.msg_served, 2,
        "the second message must reach the instance the first one warmed; a count \
         stuck at 1 is a fresh instance per message, saw {second:?}"
    );
    Ok(())
}

/// A delivery counts against `max_invocations` like any other call, and the
/// instance is retired once the budget is spent.
///
/// Read through the *HTTP* counter, because a retired instance's own counters
/// can never be read again — it stops admitting, so the probe that would ask it
/// is served by its replacement. A replacement counting its requests from one
/// is the observable consequence, and the control is what gives it meaning:
/// with no budget the same three calls all land on one instance.
#[tokio::test]
async fn a_delivery_counts_against_max_invocations() -> Result<()> {
    // Control: no budget, so the instance the first probe warms serves the
    // delivery too and is still there for the second probe.
    let unlimited = start_msg_sleeper("msg-budget-off", "budget.off", 1, 1, 0).await?;
    let warmed = unlimited.probe().await?;
    assert_eq!(
        warmed.served, 1,
        "the first probe should have warmed an instance, saw {warmed:?}"
    );
    unlimited.publish().await?;
    unlimited.settle().await;
    let after = unlimited.probe().await?;
    assert_eq!(
        (after.served, after.msg_served),
        (2, 1),
        "without a budget both probes and the delivery should be one instance's \
         work, saw {after:?}"
    );

    // Two calls apiece. The probe and the delivery spend the budget between
    // them, so the instance retires and the next probe gets a replacement.
    let limited = start_msg_sleeper("msg-budget-on", "budget.on", 1, 1, 2).await?;
    let warmed = limited.probe().await?;
    assert_eq!(
        warmed.served, 1,
        "the first probe should have warmed an instance, saw {warmed:?}"
    );
    limited.publish().await?;
    limited.settle().await;
    let after = limited.probe().await?;

    assert_eq!(
        (after.served, after.msg_served),
        (1, 0),
        "the delivery should have spent the instance's last invocation, leaving \
         this probe to a replacement counting from one, saw {after:?}"
    );
    Ok(())
}
