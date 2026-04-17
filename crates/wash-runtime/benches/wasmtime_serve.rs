//! "wasmtime serve"–equivalent HTTP benchmark.
//!
//! Uses `wasmtime_wasi_http::handler::ProxyHandler`  - the same dispatch path
//! `wasmtime serve` uses  - to get instance reuse and concurrent reuse. The
//! defaults mirror wasmtime serve:
//!
//!   * P2: `max_instance_reuse_count = 1` (no reuse; fresh instance per request)
//!   * P3: `max_instance_reuse_count = 128`, `max_instance_concurrent_reuse_count = 16`
//!
//! Compare against `wasmtime_baseline.rs` (no-reuse, per-request instantiation)
//! and `http_invoke.rs` (wash-runtime). The three benches together give a
//! decomposition of where the 5× gap against `wasmtime serve` lives:
//!
//!   `http_invoke`           =  wasmtime + wash-runtime wrappers
//!   `wasmtime_baseline`     =  wasmtime, per-request instance (our strategy)
//!   `wasmtime_serve`        =  wasmtime + ProxyHandler (wasmtime serve's strategy)
//!
//! Run with:
//! ```text
//! cargo bench -p wash-runtime --features wasip3 --bench wasmtime_serve
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

use anyhow::Context as _;
use common::Flavor;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto,
};
use tokio::{net::TcpListener, runtime::Runtime, sync::oneshot, task::JoinHandle};
use wasmtime::{
    Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    handler::{HandlerState, Proxy, ProxyHandler, ProxyPre as HandlerProxyPre, StoreBundle},
    p2::body::HyperOutgoingBody,
};

// Defaults lifted from wasmtime-cli/src/commands/serve.rs.
const DEFAULT_WASIP3_MAX_INSTANCE_REUSE_COUNT: usize = 128;
const DEFAULT_WASIP2_MAX_INSTANCE_REUSE_COUNT: usize = 1;
const DEFAULT_WASIP3_MAX_INSTANCE_CONCURRENT_REUSE_COUNT: usize = 16;

// ---------------------------------------------------------------------------
// Store context
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
// Engine + Linker
// ---------------------------------------------------------------------------

fn max_instance_reuse(flavor: Flavor) -> usize {
    match flavor {
        Flavor::P2 => DEFAULT_WASIP2_MAX_INSTANCE_REUSE_COUNT,
        Flavor::P3 => DEFAULT_WASIP3_MAX_INSTANCE_REUSE_COUNT,
    }
}

fn max_concurrent_reuse(flavor: Flavor) -> usize {
    match flavor {
        Flavor::P2 => 1,
        Flavor::P3 => DEFAULT_WASIP3_MAX_INSTANCE_CONCURRENT_REUSE_COUNT,
    }
}

fn build_engine() -> anyhow::Result<Engine> {
    let mut cfg = wasmtime::Config::default();
    let mut pool = wasmtime::PoolingAllocationConfig::default();
    pool.total_memories(100);
    pool.total_tables(100);
    pool.total_component_instances(100);
    cfg.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pool));
    #[cfg(feature = "wasip3")]
    cfg.wasm_component_model_async(true);
    Ok(Engine::new(&cfg)?)
}

fn build_linker(engine: &Engine) -> anyhow::Result<Linker<Ctx>> {
    let mut linker: Linker<Ctx> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    #[cfg(feature = "wasip3")]
    {
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    }
    Ok(linker)
}

// ---------------------------------------------------------------------------
// HandlerState  - tells ProxyHandler how to produce Stores and bound reuse
// ---------------------------------------------------------------------------

struct State {
    engine: Engine,
    max_instance_reuse_count: usize,
    max_instance_concurrent_reuse_count: usize,
}

impl HandlerState for State {
    type StoreData = Ctx;

    fn new_store(&self, _req_id: Option<u64>) -> wasmtime::Result<StoreBundle<Ctx>> {
        let store = Store::new(&self.engine, Ctx::new());
        Ok(StoreBundle {
            store,
            write_profile: Box::new(|_| ()),
        })
    }

    fn request_timeout(&self) -> Duration {
        Duration::MAX
    }

    fn idle_instance_timeout(&self) -> Duration {
        // Short enough that idle workers don't linger across bench groups,
        // long enough that it never fires mid-run.
        Duration::from_secs(60)
    }

    fn max_instance_reuse_count(&self) -> usize {
        self.max_instance_reuse_count
    }

    fn max_instance_concurrent_reuse_count(&self) -> usize {
        self.max_instance_concurrent_reuse_count
    }

    fn handle_worker_error(&self, error: wasmtime::Error) {
        eprintln!("[wasmtime_serve bench] worker error: {error:?}");
    }
}

// ---------------------------------------------------------------------------
// Request dispatch  - mirrors wasmtime-cli/src/commands/serve.rs::handle_request
// ---------------------------------------------------------------------------

type P2Response = Result<
    hyper::Response<HyperOutgoingBody>,
    wasmtime_wasi_http::p2::bindings::http::types::ErrorCode,
>;
type P3Response =
    hyper::Response<http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, wasmtime::Error>>;

enum Sender {
    P2(oneshot::Sender<P2Response>),
    P3(oneshot::Sender<P3Response>),
}

enum Receiver {
    P2(oneshot::Receiver<P2Response>),
    P3(oneshot::Receiver<P3Response>),
}

async fn handle_request(
    handler: ProxyHandler<State>,
    req: hyper::Request<Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let req_id = handler.next_req_id();

    let (tx, rx) = match handler.instance_pre() {
        HandlerProxyPre::P2(_) => {
            let (tx, rx) = oneshot::channel();
            (Sender::P2(tx), Receiver::P2(rx))
        }
        HandlerProxyPre::P3(_) => {
            let (tx, rx) = oneshot::channel();
            (Sender::P3(tx), Receiver::P3(rx))
        }
    };

    handler.spawn(
        if handler.state().max_instance_reuse_count() == 1 {
            Some(req_id)
        } else {
            None
        },
        Box::new(move |accessor, proxy| {
            Box::pin(async move {
                let result: wasmtime::Result<()> = match proxy {
                    Proxy::P2(proxy) => {
                        let Sender::P2(tx) = tx else { unreachable!() };
                        let setup: wasmtime::Result<_> = accessor.with(move |mut store| {
                            let req = wasmtime_wasi_http::p2::WasiHttpView::http(store.data_mut())
                                .new_incoming_request(
                                    wasmtime_wasi_http::p2::bindings::http::types::Scheme::Http,
                                    req,
                                )?;
                            let out = wasmtime_wasi_http::p2::WasiHttpView::http(store.data_mut())
                                .new_response_outparam(tx)?;
                            wasmtime::error::Ok((req, out))
                        });
                        let (req, out) = match setup {
                            Ok(v) => v,
                            Err(e) => return eprintln!("[{req_id}] setup: {e:?}"),
                        };
                        proxy
                            .wasi_http_incoming_handler()
                            .call_handle(accessor, req, out)
                            .await
                    }
                    Proxy::P3(proxy) => {
                        let Sender::P3(tx) = tx else { unreachable!() };
                        use wasmtime_wasi_http::p3::bindings::http::types::{ErrorCode, Request};

                        let (parts, body) = req.into_parts();
                        let body = body.map_err(ErrorCode::from_hyper_request_error);
                        let req = hyper::Request::from_parts(parts, body);
                        let (request, request_io_result) = Request::from_http(req);

                        let res = match proxy.handle(accessor, request).await {
                            Ok(Ok(r)) => r,
                            Ok(Err(code)) => {
                                return eprintln!("[{req_id}] handler err: {code:?}");
                            }
                            Err(e) => return eprintln!("[{req_id}] trap: {e:?}"),
                        };

                        let res =
                            accessor.with(|mut store| res.into_http(&mut store, request_io_result));
                        let res = match res {
                            Ok(r) => r,
                            Err(e) => return eprintln!("[{req_id}] into_http: {e:?}"),
                        };

                        let res = res.map(|body| body.map_err(|e| e.into()).boxed_unsync());
                        let _ = tx.send(res);
                        Ok(())
                    }
                };
                if let Err(e) = result {
                    eprintln!("[{req_id}] :: {e:?}");
                }
            })
        }),
    );

    match rx {
        Receiver::P2(rx) => {
            let resp = rx
                .await
                .context("guest never invoked response-outparam::set")?;
            Ok(resp.map_err(wasmtime::Error::from)?)
        }
        Receiver::P3(rx) => {
            let resp = rx.await?;
            let (parts, body) = resp.into_parts();
            // Collect and re-wrap so the outer response body type matches the
            // P2 path. For the minimal fixtures this is cheap (tiny payload).
            let body = body
                .collect()
                .await
                .map_err(|e| anyhow::anyhow!("collect p3 body: {e:?}"))?
                .to_bytes();
            let body: HyperOutgoingBody = http_body_util::Full::new(body)
                .map_err(|never| match never {})
                .boxed_unsync();
            Ok(hyper::Response::from_parts(parts, body))
        }
    }
}

// ---------------------------------------------------------------------------
// Hyper server  - binds, accepts, dispatches through ProxyHandler.
// ---------------------------------------------------------------------------

struct Server {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    _join: JoinHandle<()>,
}

impl Server {
    async fn start(flavor: Flavor) -> anyhow::Result<Self> {
        let engine = build_engine()?;
        let linker = build_linker(&engine)?;
        let component = Component::from_binary(&engine, flavor.wasm())?;
        let instance_pre = linker.instantiate_pre(&component)?;

        // Wrap InstancePre in the right handler::ProxyPre variant.
        let handler_pre = match flavor {
            Flavor::P2 => HandlerProxyPre::P2(
                wasmtime_wasi_http::handler::p2::bindings::ProxyPre::new(instance_pre)?,
            ),
            #[cfg(feature = "wasip3")]
            Flavor::P3 => HandlerProxyPre::P3(wasmtime_wasi_http::p3::bindings::ServicePre::new(
                instance_pre,
            )?),
            #[cfg(not(feature = "wasip3"))]
            Flavor::P3 => anyhow::bail!("wasip3 feature disabled"),
        };

        let handler = ProxyHandler::new(
            State {
                engine,
                max_instance_reuse_count: max_instance_reuse(flavor),
                max_instance_concurrent_reuse_count: max_concurrent_reuse(flavor),
            },
            handler_pre,
        );

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

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
                let handler = handler.clone();
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service =
                        hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
                            let handler = handler.clone();
                            async move {
                                match handle_request(handler, req).await {
                                    Ok(r) => Ok::<_, hyper::Error>(r),
                                    Err(e) => {
                                        tracing::error!(err = ?e, "handler error");
                                        Ok(error_response(500))
                                    }
                                }
                            }
                        });
                    let builder = {
                        let mut b = auto::Builder::new(TokioExecutor::new());
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
        let (tx, _rx) = oneshot::channel::<()>();
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
    let mut group = c.benchmark_group("serve_cold_invocation");
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
    let mut group = c.benchmark_group("serve_hot_invocation");
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

    let mut group = c.benchmark_group("serve_http_throughput");
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
                "[serve_http_throughput/{}] {failed} requests failed during bench run",
                flavor.name()
            );
        }
        drop(warm);
    }
    group.finish();
}

criterion_group!(benches, bench_cold, bench_hot, bench_throughput);
criterion_main!(benches);
