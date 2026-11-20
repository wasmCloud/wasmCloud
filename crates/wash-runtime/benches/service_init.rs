//! Service metadata initialization on an engine shared by several workloads.
//! Compilation and the first service's linker setup are warmed before timing;
//! workload cloning and teardown are excluded from each measured start.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use criterion::{BatchSize, Criterion, criterion_group};

fn bench_service_initialization(c: &mut Criterion) {
    let engine = common::engine();
    let workload = common::service_workload("bench-service");
    let _warm = engine
        .initialize_workload("warmup", workload.clone())
        .expect("warm service metadata");
    let mut group = c.benchmark_group("service_initialization");
    group.bench_function("cached_engine", |b| {
        b.iter_batched(
            || workload.clone(),
            |workload| {
                engine
                    .initialize_workload("bench-service", workload)
                    .expect("service metadata")
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_service_initialization);

fn main() {
    if !std::env::args_os().any(|arg| arg == "--bench") {
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
