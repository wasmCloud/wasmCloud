//! Loop-and-time microbenchmarks for the perf investigation (M3).
//!
//! Run with:
//!   cargo test --release -p wash-runtime perf_ -- --ignored --nocapture --test-threads=1
//!
//! Component under test defaults to the http-hello-world template build;
//! override with WASH_BENCH_WASM=/path/to/component.wasm.

use std::sync::Arc;
use std::time::{Duration, Instant};

use wasmtime::AsContextMut;
use wasmtime::component::Val;

use super::ctx::SharedCtx;
use super::linked_call::{bench_ctx_template, new_store_from_templates};
use super::value;
use super::{Engine, add_wasi_to_linker};
use crate::host::http::NullServer;

fn stats(label: &str, mut xs: Vec<Duration>) {
    xs.sort();
    let n = xs.len();
    let mean = xs.iter().sum::<Duration>() / n as u32;
    let p50 = xs[n / 2];
    let p90 = xs[n * 9 / 10];
    println!(
        "PERF {label}: n={n} mean={:.2}us p50={:.2}us p90={:.2}us",
        mean.as_secs_f64() * 1e6,
        p50.as_secs_f64() * 1e6,
        p90.as_secs_f64() * 1e6,
    );
}

fn bench_engine() -> Engine {
    Engine::builder()
        .with_pooling_allocator(true)
        .build()
        .expect("engine")
}

fn wasm_path() -> String {
    std::env::var("WASH_BENCH_WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../templates/http-hello-world/target/wasm32-wasip2/release/hello_world.wasm"
        )
        .to_string()
    })
}

fn wasi_linker(engine: &Engine) -> wasmtime::component::Linker<SharedCtx> {
    let mut linker = wasmtime::component::Linker::<SharedCtx>::new(engine.inner());
    add_wasi_to_linker(&mut linker).expect("add wasi");
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker).expect("add wasi:http p2");
    linker
}

/// H7: cost of deep-cloning the fully-populated WASI linker (what
/// `WorkloadMetadata::clone` pays per request in `new_store`).
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn perf_linker_clone() {
    let engine = bench_engine();
    let linker = wasi_linker(&engine);
    let mut out = Vec::with_capacity(200);
    for _ in 0..200 {
        let t = Instant::now();
        let c = linker.clone();
        out.push(t.elapsed());
        drop(c);
    }
    stats("linker_clone", out);
}

/// H3a: store creation alone (ctx templates -> Store), no instantiation.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn perf_store_create() {
    let engine = bench_engine();
    let tmpl = bench_ctx_template("bench-component", "bench-workload");
    let handler: Arc<dyn crate::host::http::HostHandler> = Arc::new(NullServer::default());
    let mut out = Vec::with_capacity(300);
    for _ in 0..300 {
        let t = Instant::now();
        let store = new_store_from_templates(engine.inner(), handler.clone(), &tmpl, &[], &[], false)
            .await
            .expect("store");
        out.push(t.elapsed());
        drop(store);
    }
    stats("store_create", out);
}

/// H3b: fresh store + instantiate per iteration (the per-request FaaS cost),
/// vs instantiate into one long-lived store.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn perf_instantiate() {
    let engine = bench_engine();
    let bytes = std::fs::read(wasm_path()).expect("read component (build the template first)");
    let component =
        wasmtime::component::Component::from_binary(engine.inner(), &bytes).expect("compile");
    let linker = wasi_linker(&engine);
    let pre = linker.instantiate_pre(&component).expect("pre");
    let tmpl = bench_ctx_template("bench-component", "bench-workload");
    let handler: Arc<dyn crate::host::http::HostHandler> = Arc::new(NullServer::default());

    // (a) fresh store + instantiate per iter
    let mut fresh = Vec::with_capacity(300);
    for _ in 0..300 {
        let t = Instant::now();
        let mut store =
            new_store_from_templates(engine.inner(), handler.clone(), &tmpl, &[], &[], false)
                .await
                .expect("store");
        let _inst = pre.instantiate_async(&mut store).await.expect("instantiate");
        fresh.push(t.elapsed());
        drop(store);
    }
    stats("fresh_store_plus_instantiate", fresh);

    // (b) reuse one store, instantiate per iter
    let mut store = new_store_from_templates(engine.inner(), handler.clone(), &tmpl, &[], &[], false)
        .await
        .expect("store");
    let mut reuse = Vec::with_capacity(300);
    for _ in 0..300 {
        let t = Instant::now();
        let _inst = pre.instantiate_async(&mut store).await.expect("instantiate");
        reuse.push(t.elapsed());
    }
    stats("instantiate_into_reused_store", reuse);
}

/// H1: dynamic Val lower/lift deep-copy cost for representative payloads
/// (what every shared-store c2c call pays per param/result), vs plain
/// Val::clone as the floor.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn perf_lower_lift() {
    let engine = bench_engine();
    let tmpl = bench_ctx_template("bench-component", "bench-workload");
    let handler: Arc<dyn crate::host::http::HostHandler> = Arc::new(NullServer::default());
    let mut store = new_store_from_templates(engine.inner(), handler.clone(), &tmpl, &[], &[], false)
        .await
        .expect("store");
    let mut cx = store.as_context_mut();

    let small = Val::String("hello world".to_string());
    let record_1k = Val::Record(
        (0..16)
            .map(|i| (format!("field{i}"), Val::String("x".repeat(64))))
            .collect(),
    );
    let list_64k = Val::List(vec![Val::U8(0xAB); 65536]);

    for (label, v) in [
        ("small_string", &small),
        ("record_1k", &record_1k),
        ("list_64k_u8", &list_64k),
    ] {
        let mut lower_t = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let t = Instant::now();
            let lowered = value::lower(&mut cx, v).expect("lower");
            lower_t.push(t.elapsed());
            drop(lowered);
        }
        stats(&format!("lower_{label}"), lower_t);

        let mut clone_t = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let t = Instant::now();
            let c = v.clone();
            clone_t.push(t.elapsed());
            drop(c);
        }
        stats(&format!("valclone_{label}"), clone_t);

        let mut lift_t = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let vv = v.clone();
            let t = Instant::now();
            let lifted = value::lift(&mut cx, vv).expect("lift");
            lift_t.push(t.elapsed());
            drop(lifted);
        }
        stats(&format!("lift_{label}"), lift_t);
    }
}
