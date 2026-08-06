//! Integration test for the pooled outbound HTTP transport, driven end to end
//! through a real guest.
//!
//! Unlike the other egress tests, this one installs no stub handler: the host
//! runs the default [`DefaultOutgoingHandler`], so a guest's `wasi:http`
//! outgoing request travels the same path it does in `wash host` and `wash
//! dev` — through the per-workload connection pool — and lands on a real
//! server that counts accepted TCP connections.
//!
//! That connection count is the whole point. wasmtime's per-request transport
//! opens a fresh connection for every outgoing request, which under sustained
//! traffic exhausts ephemeral ports and surfaces to guests as a misleading
//! `DNS error: rcode="address not available"`. Counting connections server-side
//! is the only way to tell reuse from a coincidentally-passing request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, Ingress},
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitInterface,
};

const HTTP_EGRESS_POOL_WASM: &[u8] = include_bytes!("wasm/http_egress_pool.wasm");

/// Keep-alive HTTP/1.1 server that counts accepted connections and answers
/// every request with `200 ok`.
async fn spawn_counting_server() -> Result<(std::net::SocketAddr, Arc<AtomicUsize>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let conns = Arc::new(AtomicUsize::new(0));
    let conns_clone = conns.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(_) => return,
            };
            conns_clone.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let mut pending = Vec::new();
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    let Some(chunk) = buf.get(..n) else { return };
                    pending.extend_from_slice(chunk);
                    // One response per request head; the fixture sends GETs.
                    while let Some(pos) = pending.windows(4).position(|w| w == b"\r\n\r\n") {
                        pending.drain(..pos + 4);
                        if stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });
    Ok((addr, conns))
}

/// Start a host whose ingress uses the *default* outgoing handler — the
/// pooled transport under test.
async fn start_host() -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let bound_addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((bound_addr, host))
}

fn egress_workload(workload_id: &str, allowed_host: &str) -> WorkloadStartRequest {
    egress_workload_with(workload_id, allowed_host, 1, 1)
}

fn egress_workload_with(
    workload_id: &str,
    allowed_host: &str,
    pool_size: i32,
    max_concurrency: i32,
) -> WorkloadStartRequest {
    let allowed: wash_runtime::host::allowed_hosts::AllowedHost = allowed_host
        .parse()
        .expect("test gave an invalid allowed_hosts entry");
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-egress-pool".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-egress-pool.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_EGRESS_POOL_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: vec![allowed].into(),
                    allowed_ip_name_lookups: Default::default(),
                },
                pool_size,
                max_invocations: 1000,
                max_concurrency,
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: {
                    let mut config = HashMap::new();
                    config.insert("host".to_string(), "test".to_string());
                    config
                },
                name: None,
            }],
            volumes: vec![],
        },
    }
}

/// Drive `count` inbound requests, each of which makes one outbound request
/// through the host's egress transport. Returns nothing; asserts every hop
/// succeeded so a connection count can't look good because requests failed.
async fn drive_requests(
    ingress: std::net::SocketAddr,
    upstream: std::net::SocketAddr,
    count: usize,
) -> Result<()> {
    let client = reqwest::Client::new();
    for i in 0..count {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{ingress}/fetch?target={upstream}"))
                .header("HOST", "test")
                .send(),
        )
        .await
        .with_context(|| format!("request {i} timed out"))?
        .with_context(|| format!("request {i} failed"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::ensure!(
            status == 200,
            "request {i}: guest returned {status} (body: {body})"
        );
        anyhow::ensure!(
            body.contains("upstream 200"),
            "request {i}: guest did not see a 200 from upstream (body: {body})"
        );
    }
    Ok(())
}

/// A guest making repeated outgoing requests to one authority must reuse a
/// handful of connections, not open one per request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pooled_egress_reuses_connections_across_guest_requests() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (upstream, conns) = spawn_counting_server().await?;
    let (ingress, host) = start_host().await?;
    host.workload_start(egress_workload(
        &uuid::Uuid::new_v4().to_string(),
        &upstream.to_string(),
    ))
    .await
    .context("failed to start workload")?;

    const REQUESTS: usize = 25;
    drive_requests(ingress, upstream, REQUESTS).await?;

    let opened = conns.load(Ordering::SeqCst);
    assert!(
        opened < REQUESTS,
        "the pooled transport must reuse connections, but the upstream saw \
         {opened} connections for {REQUESTS} requests (a per-request transport \
         opens one each)"
    );
    // A single-instance guest issues these serially, so one connection carries
    // them all; allow a little slack for a reconnect.
    assert!(
        opened <= 3,
        "expected a handful of reused connections, but the upstream saw {opened}"
    );
    Ok(())
}

/// Stopping a workload must release its pooled connections rather than leave
/// them open until the pool's idle timeout, and a workload started afterwards
/// must not inherit them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_workload_drops_its_pooled_connections() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (upstream, conns) = spawn_counting_server().await?;
    let (ingress, host) = start_host().await?;

    let first = uuid::Uuid::new_v4().to_string();
    host.workload_start(egress_workload(&first, &upstream.to_string()))
        .await
        .context("failed to start the first workload")?;
    drive_requests(ingress, upstream, 5).await?;
    let after_first = conns.load(Ordering::SeqCst);
    assert!(
        after_first <= 3,
        "the first workload should have reused its connection, saw {after_first}"
    );

    host.workload_stop(WorkloadStopRequest {
        workload_id: first.clone(),
    })
    .await
    .context("failed to stop the first workload")?;

    let second = uuid::Uuid::new_v4().to_string();
    host.workload_start(egress_workload(&second, &upstream.to_string()))
        .await
        .context("failed to start the second workload")?;
    drive_requests(ingress, upstream, 5).await?;

    let total = conns.load(Ordering::SeqCst);
    assert!(
        total > after_first,
        "the second workload must open its own connection rather than inherit \
         the stopped workload's ({total} total connections, {after_first} before)"
    );
    assert!(
        total <= 6,
        "each workload should still reuse within its own pool, but the upstream \
         saw {total} connections for 10 requests"
    );
    Ok(())
}

/// Drive `count` inbound requests concurrently and return how many succeeded,
/// so a test can assert on connection counts without a slow request making
/// the burst look smaller than it was.
async fn drive_concurrent_requests(
    ingress: std::net::SocketAddr,
    upstream: std::net::SocketAddr,
    count: usize,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(count)
        .build()?;
    let requests = (0..count).map(|i| {
        let client = client.clone();
        async move {
            let response = timeout(
                Duration::from_secs(30),
                client
                    .get(format!("http://{ingress}/fetch?target={upstream}"))
                    .header("HOST", "test")
                    .send(),
            )
            .await
            .with_context(|| format!("request {i} timed out"))?
            .with_context(|| format!("request {i} failed"))?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::ensure!(
                status == 200 && body.contains("upstream 200"),
                "request {i}: guest returned {status} (body: {body})"
            );
            Ok::<_, anyhow::Error>(())
        }
    });
    for result in futures::future::join_all(requests).await {
        result?;
    }
    Ok(())
}

/// A component declaring instance concurrency bursts far more outbound
/// requests at once than one running a call at a time, and the pool has to be
/// sized for that burst: a cap below it closes every connection handed back to
/// a full pool, which is the connection churn this pool exists to remove.
///
/// Two rounds against the same upstream: the second must ride on connections
/// the first left in the pool, so the count barely moves. With an idle cap
/// below the burst, round two reopens most of what it needs.
///
/// The fixture is a wasip2 component, and `invoke_component_handler` only
/// serves wasip3 exports from warm instances, so the concurrency here comes
/// from the burst of inbound requests rather than from instances sharing one
/// store. What the component *declares* is what sizes the pool either way,
/// which is the property under test; a wasip3 fixture would additionally
/// exercise the instances themselves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_instances_reuse_connections_across_bursts() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (upstream, conns) = spawn_counting_server().await?;
    let (ingress, host) = start_host().await?;

    // 4 warm instances × 12 concurrent calls each: a burst far past the
    // 32-connection floor a component running one call at a time would get.
    host.workload_start(egress_workload_with(
        &uuid::Uuid::new_v4().to_string(),
        &upstream.to_string(),
        4,
        12,
    ))
    .await
    .context("failed to start workload")?;

    const BURST: usize = 48;
    drive_concurrent_requests(ingress, upstream, BURST).await?;
    let after_first = conns.load(Ordering::SeqCst);

    drive_concurrent_requests(ingress, upstream, BURST).await?;
    let after_second = conns.load(Ordering::SeqCst);

    // Measured: with the cap sized off the declared concurrency the second
    // burst reopens nothing; pinned at the 32 floor it reopens exactly the 16
    // connections the burst runs past that floor. The allowance sits between
    // the two, leaving room for a connection the upstream happens to close.
    let reopened = after_second - after_first;
    assert!(
        reopened <= BURST / 8,
        "the second burst must ride on the first burst's pooled connections, \
         but it opened {reopened} more on top of {after_first} — the idle cap \
         is below the burst, so connections are being closed and reopened"
    );
    Ok(())
}
