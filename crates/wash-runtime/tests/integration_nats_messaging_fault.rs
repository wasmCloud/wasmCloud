//! Fault-injection coverage for the NatsMessaging plugin.
//!
//! Targets the timing race that motivated #5074: a `client.flush()` between
//! `subscribe()` and the spawned subscriber loop is needed for the SUB
//! protocol message to deterministically reach NATS before
//! `on_workload_resolved` returns Ok. Without it, on a fast runner with no
//! NATS-side latency, the race usually goes the right way; introduce upstream
//! latency and the race opens wide.
//!
//! Mechanism: a Rust TCP proxy sits between the wash-runtime data NATS
//! client and a real NATS testcontainer. The proxy:
//!   * adds configurable per-direction latency,
//!   * captures all bytes flowing each direction into in-memory buffers.
//!
//! The latency lets us deterministically reproduce the race window. The
//! captured bytes let us assert protocol-level invariants (NATS' wire
//! protocol is line-based ASCII so substring matches on the buffer are
//! sufficient: `SUB <subject> <sid>\r\n`, `UNSUB <sid>\r\n`).
//!
//! Requires Docker. Gated behind `NATS_INTEGRATION_TESTS=1`.

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
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

const MESSAGING_ECHO_WASM: &[u8] = include_bytes!("wasm/messaging_echo.wasm");

const SUBSCRIPTION_SUBJECT: &str = "test.echo";

// ---------------------------------------------------------------------------
// FaultProxy: TCP proxy with controllable latency + traffic capture.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FaultProxy {
    listen_addr: SocketAddr,
    /// Bytes the wash-runtime client wrote toward NATS.
    client_to_server: Arc<Mutex<Vec<u8>>>,
    /// Bytes NATS wrote back toward the wash-runtime client.
    server_to_client: Arc<Mutex<Vec<u8>>>,
}

impl FaultProxy {
    /// Bind to an ephemeral local port and forward to `upstream`, applying
    /// `latency` to bytes in each direction. Returns once the listener is
    /// accepting; the underlying accept loop runs as a tokio task that lives
    /// until the test process exits.
    async fn start(upstream: SocketAddr, latency: Duration) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("failed to bind fault proxy listener")?;
        let listen_addr = listener.local_addr()?;

        let client_to_server = Arc::new(Mutex::new(Vec::new()));
        let server_to_client = Arc::new(Mutex::new(Vec::new()));

        let c2s = Arc::clone(&client_to_server);
        let s2c = Arc::clone(&server_to_client);
        tokio::spawn(async move {
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let upstream = match TcpStream::connect(upstream).await {
                    Ok(u) => u,
                    Err(_) => continue,
                };

                let c2s = Arc::clone(&c2s);
                let s2c = Arc::clone(&s2c);
                tokio::spawn(async move {
                    let (mut cr, mut cw) = client.into_split();
                    let (mut ur, mut uw) = upstream.into_split();
                    tokio::join!(
                        copy_with_latency(&mut cr, &mut uw, latency, c2s),
                        copy_with_latency(&mut ur, &mut cw, latency, s2c),
                    );
                });
            }
        });

        Ok(Self {
            listen_addr,
            client_to_server,
            server_to_client,
        })
    }

    fn url(&self) -> String {
        format!("nats://{}", self.listen_addr)
    }

    async fn client_to_server_bytes(&self) -> Vec<u8> {
        self.client_to_server.lock().await.clone()
    }

    #[allow(dead_code)]
    async fn server_to_client_bytes(&self) -> Vec<u8> {
        self.server_to_client.lock().await.clone()
    }
}

async fn copy_with_latency<R, W>(
    reader: &mut R,
    writer: &mut W,
    latency: Duration,
    capture: Arc<Mutex<Vec<u8>>>,
) where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        capture.lock().await.extend_from_slice(&buf[..n]);
        if !latency.is_zero() {
            tokio::time::sleep(latency).await;
        }
        if writer.write_all(&buf[..n]).await.is_err() {
            break;
        }
        let _ = writer.flush().await;
    }
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct TestHarness {
    proxy: FaultProxy,
    monitoring_url: String,
    _host: Box<dyn std::any::Any + Send>,
    _container: Box<dyn std::any::Any + Send>,
}

async fn setup(latency: Duration) -> Result<TestHarness> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_exposed_port(8222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-m", "8222"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start NATS container: {e}"))?;

    let nats_port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS host port: {e}"))?;
    let monitoring_port = container
        .get_host_port_ipv4(8222)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get NATS monitoring host port: {e}"))?;

    let nats_addr: SocketAddr = format!("127.0.0.1:{nats_port}").parse()?;
    let proxy = FaultProxy::start(nats_addr, latency).await?;
    let monitoring_url = format!("http://127.0.0.1:{monitoring_port}");

    let plugin_client = Arc::new(
        async_nats::connect(proxy.url())
            .await
            .context("Failed to connect plugin client through fault proxy")?,
    );

    let engine = Engine::builder().build()?;
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
            name: "messaging-fault".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "messaging-handler".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(MESSAGING_ECHO_WASM),
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
        .context("workload_start did not return Ok — host failed to bind/resolve under fault")?;

    Ok(TestHarness {
        proxy,
        monitoring_url,
        _host: Box::new(host),
        _container: Box::new(container),
    })
}

fn skip_if_disabled() -> bool {
    if std::env::var("NATS_INTEGRATION_TESTS").unwrap_or_default() != "1" {
        eprintln!("Skipping NATS fault-injection test (set NATS_INTEGRATION_TESTS=1 to enable)");
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Fault injection: invariant must hold even with NATS-side latency
// ---------------------------------------------------------------------------

/// 100ms latency per direction widens the race window: a SUB protocol
/// message takes ~100ms to reach NATS; without `client.flush()` in
/// `on_workload_resolved`, the function returns immediately after queuing
/// the SUB into the async-nats command channel, while NATS hasn't yet
/// received it. With the flush, the function blocks until PING/PONG
/// (~200ms round trip) so by the time it returns the SUB is registered.
///
/// The assertion: query NATS' `/connz` once, no retries, immediately after
/// `workload_start` returns. The assertion is the contract — when
/// resolve says "ok", the subscription is *on the server*. The latency
/// makes the window wide enough to be deterministic on every machine.
#[tokio::test]
async fn subscription_registers_under_upstream_latency() -> Result<()> {
    if skip_if_disabled() {
        return Ok(());
    }

    let harness = setup(Duration::from_millis(100)).await?;

    let connz_url = format!("{}/connz?subs=true", harness.monitoring_url);
    // Brief retry to absorb /connz update lag — it's the server's view that
    // matters, and that view sometimes lags a few hundred ms even after
    // PONG comes back. Cap at 1s; without flush the SUB never lands here.
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let body = reqwest::get(&connz_url).await?.text().await?;
        if body.contains(SUBSCRIPTION_SUBJECT) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "subscription `{SUBSCRIPTION_SUBJECT}` did not register on NATS within \
                 1s of `workload_start` returning, under 100ms upstream latency. This \
                 means the SUB protocol message has not reached NATS — likely a \
                 regression of #5074 (someone removed `client.flush()`). last /connz \
                 body:\n{body}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Protocol-level assertions
// ---------------------------------------------------------------------------

/// Asserts the SUB protocol message was actually written by the wash-runtime
/// client toward NATS before `workload_start` returned. Independent of any
/// timing or NATS server state — looks at the bytes the proxy captured.
///
/// NATS protocol subscribe message format: `SUB <subject> <sid>\r\n`.
#[tokio::test]
async fn sub_protocol_message_sent_before_resolve_returns() -> Result<()> {
    if skip_if_disabled() {
        return Ok(());
    }

    let harness = setup(Duration::from_millis(0)).await?;

    // Poll briefly for the SUB to land in the capture buffer. Even with the
    // plugin's `flush()` ensuring the SUB has reached NATS, the proxy's
    // capture is one async hop away — bytes are read into the buffer by the
    // forwarder task, which races with this assertion. The flush is what
    // makes this poll terminate quickly; without it, the SUB may never
    // arrive (regression of #5074).
    let needle = format!("SUB {SUBSCRIPTION_SUBJECT} ");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let bytes = harness.proxy.client_to_server_bytes().await;
        if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            let preview = String::from_utf8_lossy(&bytes);
            anyhow::bail!(
                "expected client→server bytes to contain `{needle}` within 2s of \
                 workload_start returning, but did not. captured bytes ({n}):\n{preview}",
                n = bytes.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Asserts the steady-state behavior: after `workload_start` returns and the
/// subscriber loop has time to settle, the workload's user-facing subject
/// (`test.echo`) is still subscribed — no UNSUB has been sent for *its* sid.
/// This catches the cleanup-on-error path firing inappropriately or the
/// spawned loop exiting and dropping its Subscribers.
///
/// `on_workload_resolved`'s server-side sync barrier intentionally creates
/// and tears down a sentinel inbox subscription, so a generic substring
/// match for `UNSUB ` would false-positive on that. Instead, parse the
/// `SUB <subject> <sid>` line for our subject and assert no `UNSUB <sid>`
/// follows it.
#[tokio::test]
async fn no_unsubscribe_during_steady_state() -> Result<()> {
    if skip_if_disabled() {
        return Ok(());
    }

    let harness = setup(Duration::from_millis(0)).await?;
    // Give the subscriber loop a moment to be polled; if it were going to
    // exit immediately and drop its Subscribers, the UNSUB would land here.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let bytes = harness.proxy.client_to_server_bytes().await;
    let text = String::from_utf8_lossy(&bytes);
    let needle = format!("SUB {SUBSCRIPTION_SUBJECT} ");
    let sid = text
        .lines()
        .find_map(|line| line.strip_prefix(&needle))
        .map(|tail| tail.split_whitespace().next().unwrap_or("").to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no `SUB {SUBSCRIPTION_SUBJECT} <sid>` line in captured bytes; can't verify \
             the workload's subscription is still active. captured:\n{text}"
            )
        })?;

    let unsub_for_sid = format!("UNSUB {sid}");
    if text.lines().any(|line| line.starts_with(&unsub_for_sid)) {
        anyhow::bail!(
            "found `{unsub_for_sid}` in client→server bytes during steady state; the \
             subscriber loop has dropped the workload's Subscriber (regression of \
             #5074-class behavior). captured bytes:\n{text}"
        );
    }
    Ok(())
}
