//! Pure-wasmtime HTTP baseline benchmark.
//!
//! Mirrors `http_invoke.rs` but replaces the wash-runtime host with a
//! hand-rolled hyper + wasmtime-wasi-http server  - the same primitives used
//! by `wasmtime serve`, without wash-runtime's routing, workload model, or
//! plugin scaffolding.
//!
//! This lets us run **the same component bytes** through a minimal wasmtime
//! pipeline and compare against `http_invoke.rs` directly. The delta between
//! the two reports is what wash-runtime's extra layers cost per request.
//!
//! Strategy choice: this baseline uses **one instance per request**, matching
//! wash-runtime's current strategy so the comparison isolates wrapper
//! overhead (routing, cloning, SharedCtx setup, ProxyPre wrapping, P3 body
//! collection, etc.) rather than the instance-reuse policy itself. See
//! `wasmtime_serve.rs` for the instance-reuse variant that uses
//! `wasmtime_wasi_http::handler::ProxyHandler`.
//!
//! Run with:
//! ```text
//! cargo bench -p wash-runtime --features wasip3 --bench wasmtime_baseline
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use common::Flavor;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto,
};
use tokio::{net::TcpListener, runtime::Runtime, task::JoinHandle};
use wasmtime::{
    Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    p2::{
        bindings::{ProxyPre, http::types::Scheme as P2Scheme},
        body::HyperOutgoingBody,
    },
};

// ---------------------------------------------------------------------------
// Store context  - the bare minimum: WasiCtx + ResourceTable + WasiHttpCtx.
// ---------------------------------------------------------------------------

struct Ctx {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
}

impl Ctx {
    fn new() -> Self {
        Self {
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl wasmtime_wasi_http::p2::WasiHttpView for Ctx {
    fn http(&mut self) -> wasmtime_wasi_http::p2::WasiHttpCtxView<'_> {
        wasmtime_wasi_http::p2::WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: Default::default(),
        }
    }
}

#[cfg(feature = "wasip3")]
impl wasmtime_wasi_http::p3::WasiHttpView for Ctx {
    fn http(&mut self) -> wasmtime_wasi_http::p3::WasiHttpCtxView<'_> {
        wasmtime_wasi_http::p3::WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: wasmtime_wasi_http::p3::default_hooks(),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine + Linker construction
// ---------------------------------------------------------------------------

fn build_engine() -> anyhow::Result<Engine> {
    let mut cfg = wasmtime::Config::default();
    // async_support is implied by current wasmtime; the explicit setter is
    // deprecated, so we rely on the default.
    let mut pool = wasmtime::PoolingAllocationConfig::default();
    pool.total_memories(100);
    pool.total_tables(100);
    pool.total_component_instances(100);
    cfg.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pool));
    #[cfg(feature = "wasip3")]
    cfg.wasm_component_model_async(true);
    Ok(Engine::new(&cfg)?)
}

/// Build a linker that can serve both P2 and P3 proxy components.
fn build_linker(engine: &Engine) -> anyhow::Result<Linker<Ctx>> {
    let mut linker: Linker<Ctx> = Linker::new(engine);

    // P2 WASI + wasi:http bindings.
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;

    // P3 WASI + wasi:http bindings (only present when the wasip3 feature is on).
    #[cfg(feature = "wasip3")]
    {
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    }

    Ok(linker)
}

// ---------------------------------------------------------------------------
// Per-request handlers  - one Store per request for apples-to-apples with wash.
// ---------------------------------------------------------------------------

/// Handle a P2 request via `wasi:http/incoming-handler`.
async fn handle_p2(
    pre: wasmtime::component::InstancePre<Ctx>,
    req: hyper::Request<Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let mut store = Store::new(pre.component().engine(), Ctx::new());
    let (sender, receiver) = tokio::sync::oneshot::channel();

    let scheme = match req.uri().scheme() {
        Some(s) if s == &hyper::http::uri::Scheme::HTTP => P2Scheme::Http,
        Some(s) if s == &hyper::http::uri::Scheme::HTTPS => P2Scheme::Https,
        Some(s) => P2Scheme::Other(s.as_str().to_string()),
        None => P2Scheme::Http,
    };

    let wasi_req = wasmtime_wasi_http::p2::WasiHttpView::http(store.data_mut())
        .new_incoming_request(scheme, req)?;
    let out = wasmtime_wasi_http::p2::WasiHttpView::http(store.data_mut())
        .new_response_outparam(sender)?;
    let proxy_pre = ProxyPre::new(pre)?;

    let task: JoinHandle<anyhow::Result<()>> = tokio::task::spawn(async move {
        let proxy = proxy_pre.instantiate_async(&mut store).await?;
        proxy
            .wasi_http_incoming_handler()
            .call_handle(&mut store, wasi_req, out)
            .await?;
        Ok(())
    });

    match receiver.await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            task.await??;
            anyhow::bail!("oneshot channel closed but no response was sent")
        }
    }
}

/// Handle a P3 request via `wasi:http/handler@0.3.x`.
#[cfg(feature = "wasip3")]
async fn handle_p3(
    pre: wasmtime::component::InstancePre<Ctx>,
    req: hyper::Request<Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as P2ErrorCode;
    use wasmtime_wasi_http::p3::bindings::ServicePre;
    use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode as P3ErrorCode;

    let mut store = Store::new(pre.component().engine(), Ctx::new());
    let service_pre = ServicePre::new(pre)?;

    let (parts, body) = req.into_parts();
    let body = body
        .map_err(|e| P3ErrorCode::InternalError(Some(e.to_string())))
        .boxed_unsync();
    let req = hyper::Request::from_parts(parts, body);
    let (wasi_req, req_io) = wasmtime_wasi_http::p3::Request::from_http(req);

    let service = service_pre.instantiate_async(&mut store).await?;

    let collected: hyper::Response<http_body_util::Collected<bytes::Bytes>> =
        store
            .run_concurrent(async move |store| {
                let handler_fut = async {
                    match service.handle(store, wasi_req).await {
                        Ok(Ok(response)) => {
                            let http_response: hyper::Response<_> =
                                store.with(|s| response.into_http(s, async { Ok(()) }))?;
                            let (parts, body) = http_response.into_parts();
                            let body = body
                                .collect()
                                .await
                                .map_err(|e| anyhow::anyhow!("collect body: {e:?}"))?;
                            Ok::<
                                hyper::Response<http_body_util::Collected<bytes::Bytes>>,
                                anyhow::Error,
                            >(hyper::Response::from_parts(parts, body))
                        }
                        Ok(Err(code)) => {
                            let body = http_body_util::Empty::<bytes::Bytes>::new()
                                .collect()
                                .await
                                .map_err(|e| anyhow::anyhow!("collect empty: {e:?}"))?;
                            Ok(hyper::Response::builder()
                                .status(500)
                                .body(body)
                                .unwrap_or_else(|_| {
                                    panic!("failed to build error response: {code:?}")
                                }))
                        }
                        Err(e) => Err(anyhow::anyhow!(e).context("P3 handler trap")),
                    }
                };
                let io_fut = async {
                    let _ = req_io.await;
                };
                let (r, _) = tokio::join!(handler_fut, io_fut);
                r
            })
            .await??;

    // Convert the collected response back to a streaming hyper body suitable
    // for the outgoing connection. `HyperOutgoingBody` uses P2's ErrorCode as
    // its error type, but our body is infallible so no mapping is needed.
    let _ = P2ErrorCode::InternalError(None); // keep the import in scope
    let (parts, body) = collected.into_parts();
    let body: HyperOutgoingBody = http_body_util::Full::new(body.to_bytes())
        .map_err(|never| match never {})
        .boxed_unsync();
    Ok(hyper::Response::from_parts(parts, body))
}

// ---------------------------------------------------------------------------
// Hyper server  - binds to 127.0.0.1:0, serves requests until dropped.
// ---------------------------------------------------------------------------

struct Server {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    _join: JoinHandle<()>,
}

impl Server {
    async fn start(flavor: Flavor) -> anyhow::Result<Self> {
        let engine = build_engine()?;
        let linker = build_linker(&engine)?;
        let component = Component::from_binary(&engine, flavor.wasm())?;
        let pre = linker.instantiate_pre(&component)?;
        let pre = Arc::new(pre);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let flavor_copy = flavor;

        let join = tokio::spawn(async move {
            loop {
                let accept = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    res = listener.accept() => res,
                };
                let (stream, _) = match accept {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                stream.set_nodelay(true).ok();
                let pre = pre.clone();
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service =
                        hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                            let pre = (*pre).clone();
                            async move {
                                let resp = match flavor_copy {
                                    Flavor::P2 => handle_p2(pre, req).await,
                                    #[cfg(feature = "wasip3")]
                                    Flavor::P3 => handle_p3(pre, req).await,
                                    #[cfg(not(feature = "wasip3"))]
                                    Flavor::P3 => unreachable!("wasip3 feature disabled"),
                                };
                                match resp {
                                    Ok(r) => Ok::<_, hyper::Error>(r),
                                    Err(e) => {
                                        tracing::error!(err = ?e, "handler error");
                                        Ok(error_response(500))
                                    }
                                }
                            }
                        });
                    let builder = auto::Builder::new(TokioExecutor::new());
                    let builder = {
                        let mut b = builder;
                        b.http1().timer(TokioTimer::new());
                        b.http2().timer(TokioTimer::new());
                        b
                    };
                    let _ = builder.serve_connection(io, service).await;
                });
            }
        });

        Ok(Server {
            addr,
            shutdown: shutdown_tx,
            _join: join,
        })
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // shutdown_rx is a oneshot  - sending () wakes the accept loop.
        // Replacing with a fresh channel so we can move out of &mut self.
        let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
        let old = std::mem::replace(&mut self.shutdown, tx);
        let _ = old.send(());
    }
}

fn error_response(status: u16) -> hyper::Response<HyperOutgoingBody> {
    let body: HyperOutgoingBody = http_body_util::Empty::<bytes::Bytes>::new()
        .map_err(|never| match never {})
        .boxed_unsync();
    hyper::Response::builder()
        .status(status)
        .body(body)
        .unwrap()
}

// ---------------------------------------------------------------------------
// Benchmark harness
// ---------------------------------------------------------------------------

struct Warm {
    server: Server,
    client: reqwest::Client,
}

async fn start_warm(flavor: Flavor) -> anyhow::Result<Warm> {
    let server = Server::start(flavor).await?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(64)
        .tcp_nodelay(true)
        .build()?;
    let warmup = client
        .get(format!("http://{}/", server.addr()))
        .send()
        .await?;
    anyhow::ensure!(
        warmup.status().is_success(),
        "warmup failed for {:?}: {}",
        flavor,
        warmup.status()
    );
    let body = warmup.text().await?;
    anyhow::ensure!(
        body == flavor.expected_body(),
        "unexpected warmup body for {:?}: {body:?}",
        flavor
    );
    Ok(Warm { server, client })
}

async fn cold_once(flavor: Flavor) -> anyhow::Result<()> {
    let warm = start_warm(flavor).await?;
    drop(warm);
    Ok(())
}

async fn hot_once(warm: &Warm) -> anyhow::Result<()> {
    let resp = warm
        .client
        .get(format!("http://{}/", warm.server.addr()))
        .send()
        .await?;
    anyhow::ensure!(resp.status().is_success(), "non-2xx: {}", resp.status());
    let _ = resp.bytes().await?;
    Ok(())
}

fn bench_cold(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("wasmtime_cold_invocation");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    for flavor in [Flavor::P2, Flavor::P3] {
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt)
                .iter(|| async move { cold_once(flavor).await.unwrap() });
        });
    }
    group.finish();
}

fn bench_hot(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("wasmtime_hot_invocation");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(10));

    for flavor in [Flavor::P2, Flavor::P3] {
        let warm = rt.block_on(start_warm(flavor)).expect("warm");
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt)
                .iter(|| async { hot_once(&warm).await.unwrap() });
        });
        drop(warm);
    }
    group.finish();
}

fn bench_throughput(c: &mut Criterion) {
    const CONCURRENCY: usize = 32;
    const BATCH: usize = 256;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut group = c.benchmark_group("wasmtime_http_throughput");
    group.throughput(Throughput::Elements(BATCH as u64));
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(15));

    for flavor in [Flavor::P2, Flavor::P3] {
        let warm = rt.block_on(start_warm(flavor)).expect("warm");
        let url = format!("http://{}/", warm.server.addr());
        let client = warm.client.clone();

        let failures = Arc::new(AtomicUsize::new(0));
        let failures_ref = failures.clone();
        group.bench_function(BenchmarkId::from_parameter(flavor.name()), |b| {
            b.to_async(&rt).iter_custom(|iters| {
                let url = url.clone();
                let client = client.clone();
                let failures = failures_ref.clone();
                async move {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(CONCURRENCY);
                        let per_worker = BATCH / CONCURRENCY;
                        for _ in 0..CONCURRENCY {
                            let client = client.clone();
                            let url = url.clone();
                            let failures = failures.clone();
                            handles.push(tokio::spawn(async move {
                                for _ in 0..per_worker {
                                    match client.get(&url).send().await {
                                        Ok(resp) if resp.status().is_success() => {
                                            let _ = resp.bytes().await;
                                        }
                                        _ => {
                                            failures.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }));
                        }
                        for h in handles {
                            h.await.ok();
                        }
                        total += start.elapsed();
                    }
                    total
                }
            });
        });
        let failed = failures.load(Ordering::Relaxed);
        if failed > 0 {
            eprintln!(
                "[wasmtime_http_throughput/{}] {failed} requests failed during bench run",
                flavor.name()
            );
        }
        drop(warm);
    }
    group.finish();
}

criterion_group!(benches, bench_cold, bench_hot, bench_throughput);
criterion_main!(benches);
