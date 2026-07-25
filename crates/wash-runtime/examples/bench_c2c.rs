//! bench_c2c: quantify the per-call overhead of the ephemeral plain-value
//! component-to-component linked-call path (cosmos-data#228).
//!
//! Starts one host (DevRouter, standard test plugin set) with two workloads:
//!   - host "base": http_handler_p3.wasm            (no linked call)
//!   - host "c2c":  ephemeral_caller_p3.wasm + ephemeral_callee_p3.wasm
//!     (the caller's HTTP handler awaits one plain-value async linked call,
//!     `wasmcloud:ephemeral-test/compute.run(21) -> 43`, dispatched via
//!     invoke_ephemeral_plain)
//!
//! A custom tracing Layer timestamps the existing trace events emitted by
//! wash_runtime::engine::linked_call:
//!   "invoking ephemeral dynamic export"  -> fires AFTER new_ephemeral_store()
//!   "invoked ephemeral dynamic export"   -> fires after the call returns
//! so per request we can attribute:
//!   pre   = t(invoking) - t(request sent)   [HTTP + caller inst + new_ephemeral_store]
//!   call  = t(invoked)  - t(invoking)       [spawn + callee instantiate_async + call]
//!   post  = t(response) - t(invoked)        [caller finishes + HTTP response]
//! and (c2c e2e - base e2e) approximates the full linked-call adder.
//!
//! Usage: bench_c2c [--n N] [--warmup W] [--pooling] [--timeline K]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use std::collections::HashMap;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context as LayerContext, SubscriberExt};

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DynamicRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::{
    wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig, wasi_keyvalue::InMemoryKeyValue,
    wasi_logging::TracingLogger,
};
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

const CALLER_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/wasm/ephemeral_caller_p3.wasm"
));
const CALLEE_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/wasm/ephemeral_callee_p3.wasm"
));
const BASE_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/wasm/http_handler_p3.wasm"
));

/// One recorded tracing event: monotonic timestamp + target + message text.
#[derive(Clone)]
struct Rec {
    at: Instant,
    target: String,
    msg: String,
}

#[derive(Clone, Default)]
struct EventTape {
    inner: Arc<Mutex<Vec<Rec>>>,
}

impl EventTape {
    fn drain(&self) -> Vec<Rec> {
        match self.inner.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => Vec::new(),
        }
    }
}

struct MsgVisitor {
    msg: Option<String>,
}

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.msg = Some(format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for EventTape {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        let target = event.metadata().target();
        if !target.starts_with("wash_runtime") {
            return;
        }
        let mut v = MsgVisitor { msg: None };
        event.record(&mut v);
        let Some(msg) = v.msg else { return };
        if let Ok(mut tape) = self.inner.lock() {
            tape.push(Rec {
                at: Instant::now(),
                target: target.to_string(),
                msg,
            });
        }
    }
}

fn http_interface(host: &str) -> Vec<WitInterface> {
    vec![WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: None,
        config: HashMap::from([("host".to_string(), host.to_string())]),
        name: None,
    }]
}

fn component(name: &str, wasm: &'static [u8]) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(wasm),
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
    }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn stats(label: &str, mut xs: Vec<f64>) {
    xs.sort_by(|a, b| a.total_cmp(b));
    let n = xs.len();
    let mean = xs.iter().sum::<f64>() / n as f64;
    println!(
        "{label:<34} n={n:<4} mean={mean:8.3}ms  p50={:8.3}ms  p90={:8.3}ms  p99={:8.3}ms  min={:8.3}ms  max={:8.3}ms",
        pct(&xs, 0.50),
        pct(&xs, 0.90),
        pct(&xs, 0.99),
        xs.first().copied().unwrap_or(f64::NAN),
        xs.last().copied().unwrap_or(f64::NAN),
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut n: usize = 300;
    let mut warmup: usize = 50;
    let mut pooling = false;
    let mut timeline: usize = 3;
    let mut callee_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--n" => n = args.next().context("--n")?.parse()?,
            "--warmup" => warmup = args.next().context("--warmup")?.parse()?,
            "--pooling" => pooling = true,
            "--timeline" => timeline = args.next().context("--timeline")?.parse()?,
            "--callee" => callee_path = Some(args.next().context("--callee")?),
            other => anyhow::bail!("unknown arg {other}"),
        }
    }
    let callee_bytes = match &callee_path {
        Some(p) => bytes::Bytes::from(std::fs::read(p).context("reading --callee")?),
        None => bytes::Bytes::from_static(CALLEE_WASM),
    };

    let tape = EventTape::default();
    let subscriber = tracing_subscriber::registry().with(tape.clone());
    tracing::subscriber::set_global_default(subscriber)?;

    let engine = if pooling {
        Engine::builder().with_pooling_allocator(true).build()?
    } else {
        Engine::builder().build()?
    };
    // DynamicRouter routes by Host header; DevRouter would send everything to
    // the LAST resolved workload, corrupting the baseline.
    let http_server = HttpServer::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;
    let host = host.start().await?;

    // Baseline workload: single p3 HTTP handler, no linked call.
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: "base".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![component("base", BASE_WASM)],
            host_interfaces: http_interface("base"),
            volumes: vec![],
        },
    })
    .await
    .context("starting baseline workload")?;

    // C2C workload: caller + callee via the ephemeral plain-value path.
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "bench".to_string(),
            name: "c2c".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("ephemeral-caller", CALLER_WASM),
                Component {
                    name: "ephemeral-callee".to_string(),
                    digest: None,
                    bytes: callee_bytes,
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: http_interface("c2c"),
            volumes: vec![],
        },
    })
    .await
    .context("starting c2c workload")?;

    let client = reqwest::Client::new();

    // ---- Baseline first (warm + measure) so the big-callee instantiations
    // cannot perturb it via memory churn. ----
    for _ in 0..warmup {
        let r = client
            .get(format!("http://{addr}/"))
            .header("HOST", "base")
            .send()
            .await?;
        anyhow::ensure!(r.status().is_success());
        let body = r.bytes().await?;
        anyhow::ensure!(
            &body[..] != b"43",
            "base route unexpectedly served the c2c caller"
        );
    }
    let mut base_e2e = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let r = client
            .get(format!("http://{addr}/"))
            .header("HOST", "base")
            .send()
            .await?;
        let _ = r.bytes().await?;
        base_e2e.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    tape.drain();

    // Warmup c2c route.
    for _ in 0..warmup {
        let r = client
            .get(format!("http://{addr}/"))
            .header("HOST", "c2c")
            .send()
            .await?;
        anyhow::ensure!(r.status().is_success());
        let body = r.bytes().await?;
        anyhow::ensure!(&body[..] == b"43", "unexpected body {body:?}");
    }
    tape.drain();

    // ---- C2C: one ephemeral plain-value linked call per request ----
    let mut c2c_e2e = Vec::with_capacity(n);
    let mut pre = Vec::new(); // request sent -> "invoking" (store built)
    let mut call = Vec::new(); // "invoking" -> "invoked"
    let mut post = Vec::new(); // "invoked" -> response done
    let mut timelines_shown = 0usize;
    for i in 0..n {
        let t0 = Instant::now();
        let r = client
            .get(format!("http://{addr}/"))
            .header("HOST", "c2c")
            .send()
            .await?;
        let body = r.bytes().await?;
        let t3 = Instant::now();
        anyhow::ensure!(&body[..] == b"43");
        c2c_e2e.push((t3 - t0).as_secs_f64() * 1e3);

        let evs = tape.drain();
        let t1 = evs
            .iter()
            .find(|e| e.msg.contains("invoking ephemeral dynamic export"))
            .map(|e| e.at);
        let t2 = evs
            .iter()
            .find(|e| e.msg.contains("invoked ephemeral dynamic export"))
            .map(|e| e.at);
        if let (Some(t1), Some(t2)) = (t1, t2) {
            pre.push((t1 - t0).as_secs_f64() * 1e3);
            call.push((t2 - t1).as_secs_f64() * 1e3);
            post.push(t3.saturating_duration_since(t2).as_secs_f64() * 1e3);
        }
        if i >= n / 2 && timelines_shown < timeline {
            timelines_shown += 1;
            println!("--- request timeline #{timelines_shown} (ms since request sent) ---");
            for e in &evs {
                println!(
                    "  {:8.3}  {:<55} {}",
                    e.at.saturating_duration_since(t0).as_secs_f64() * 1e3,
                    e.msg.chars().take(55).collect::<String>(),
                    e.target
                );
            }
        }
    }

    println!();
    println!(
        "config: n={n} warmup={warmup} pooling={pooling} wasmtime=47.0.1 build=release addr={addr}"
    );
    stats("base e2e (no linked call)", base_e2e.clone());
    stats("c2c  e2e (1 linked call)", c2c_e2e.clone());
    stats("  pre  (send -> store built)", pre);
    stats("  call (store built -> returned)", call);
    stats("  post (returned -> resp done)", post);

    let mut b = base_e2e;
    let mut c = c2c_e2e;
    b.sort_by(|a, x| a.total_cmp(x));
    c.sort_by(|a, x| a.total_cmp(x));
    println!(
        "linked-call adder (p50 c2c - p50 base): {:.3}ms",
        pct(&c, 0.5) - pct(&b, 0.5)
    );

    // Keep host alive until measurements done; drop cleanly.
    drop(host);
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}
