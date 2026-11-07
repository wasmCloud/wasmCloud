//! Wall-clock cost of minting runtime UUIDs with [`wash_runtime::ids::now_v7`].
//!
//! - `ids_tight_loop/<threads>`: every thread mints IDs back to back.
//!   Throughput is IDs per second across all threads.
//! - `ids/per_millisecond`: each iteration waits, untimed, for the wall clock
//!   to reach a new millisecond, then times one ID. This is a thread minting
//!   only a few IDs per millisecond, the common case on a host, where
//!   anything a generator renews per millisecond lands on every call.
//!
//! Run with:
//! ```text
//! cargo bench -p wash-runtime --bench ids
//! ```

use std::hint::black_box;
use std::num::NonZero;
use std::sync::Barrier;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use wash_runtime::ids::now_v7;

/// IDs each thread mints per criterion iteration of the tight loop.
const PER_THREAD: u64 = 10_000;

fn tight_loop(c: &mut Criterion) {
    let cores = std::thread::available_parallelism().map_or(1, NonZero::get);
    let mut thread_counts = vec![1, 2, 4, cores];
    thread_counts.sort_unstable();
    thread_counts.dedup();

    let mut group = c.benchmark_group("ids_tight_loop");
    for threads in thread_counts {
        group.throughput(Throughput::Elements(threads as u64 * PER_THREAD));
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            &threads,
            |b, &threads| {
                b.iter_custom(|iters| {
                    let barrier = Barrier::new(threads + 1);
                    let start = std::thread::scope(|s| {
                        for _ in 0..threads {
                            s.spawn(|| {
                                barrier.wait();
                                for _ in 0..iters * PER_THREAD {
                                    black_box(now_v7());
                                }
                            });
                        }
                        barrier.wait();
                        Instant::now()
                    });
                    start.elapsed()
                });
            },
        );
    }
    group.finish();
}

fn per_millisecond(c: &mut Criterion) {
    let mut group = c.benchmark_group("ids");
    // Every iteration spends about a millisecond waiting for the clock.
    group.measurement_time(Duration::from_secs(10));
    group.bench_function("per_millisecond", |b| {
        b.iter_custom(|iters| {
            let mut timed = Duration::ZERO;
            for _ in 0..iters {
                wait_for_next_millisecond();
                let start = Instant::now();
                black_box(now_v7());
                timed += start.elapsed();
            }
            timed
        });
    });
    group.finish();
}

fn wait_for_next_millisecond() {
    let now = unix_millis();
    while unix_millis() == now {
        std::hint::spin_loop();
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

criterion_group!(benches, tight_loop, per_millisecond);
criterion_main!(benches);
