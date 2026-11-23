//! Cost of dropping a store's loopback lease on a network that holds many ports.
//! Run with `cargo bench -p wash-runtime --bench loopback_lease`.

use std::hint::black_box;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, PoisonError};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tokio::sync::Semaphore;
use wash_runtime::sockets::LoopbackLease;
use wash_runtime::sockets::loopback::{Network, Owner};

/// Long-lived stores holding the active ports.
const LONG_LIVED: usize = 64;
/// Active ports in the churn benches.
const CHURN_ACTIVE: u16 = 10_000;
/// Store lives per churn round: 12 idle, 2 socket, 2 ports.
const ROUND: usize = 16;

#[derive(Clone, Copy)]
enum Store {
    /// Opens no socket.
    Idle,
    /// Opens a socket and binds no loopback port.
    Socket,
    /// Binds two TCP ports and one UDP port.
    Ports,
}

impl Store {
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Socket => "socket",
            Self::Ports => "ports",
        }
    }
}

type Shared = Arc<Mutex<Network>>;

fn lock(network: &Shared) -> std::sync::MutexGuard<'_, Network> {
    network.lock().unwrap_or_else(PoisonError::into_inner)
}

fn any_port() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

/// A network whose ports `1..=active` are held by long-lived stores, and
/// those stores' leases.
fn network_with(active: u16) -> (Shared, Vec<LoopbackLease>) {
    let network = Shared::default();
    let leases: Vec<LoopbackLease> = (0..LONG_LIVED)
        .map(|_| LoopbackLease::from(Arc::clone(&network)))
        .collect();
    {
        let mut net = lock(&network);
        for (port, lease) in (1..=active).zip(leases.iter().cycle()) {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            if net.bind_tcp(addr, lease.owner()).is_err() {
                unreachable!("port {port} is free");
            }
        }
    }
    (network, leases)
}

/// One store's life: lease, maybe sockets and ports, teardown.
fn store_life(network: &Shared, store: Store) {
    let lease = LoopbackLease::from(Arc::clone(network));
    match store {
        Store::Idle => {}
        Store::Socket => {
            black_box(lease.owner());
        }
        Store::Ports => {
            let owner = lease.owner();
            for _ in 0..2 {
                black_box(lock(network).bind_tcp(any_port(), owner).ok());
            }
            let permits = Arc::new(Semaphore::new(0));
            black_box(lock(network).bind_udp(any_port(), permits, owner).ok());
        }
    }
    drop(lease);
}

fn round(network: &Shared) {
    for i in 0..ROUND {
        let store = match i {
            0 | 8 => Store::Socket,
            4 | 12 => Store::Ports,
            _ => Store::Idle,
        };
        store_life(network, store);
    }
}

/// One loopback bind and close, as a guest's socket makes them.
fn bind_and_close(network: &Shared, owner: Owner) {
    let bound = lock(network).bind_tcp(any_port(), owner);
    if let Ok(addr) = bound {
        black_box(lock(network).unbind_tcp(addr));
    }
}

fn lease_drop(c: &mut Criterion) {
    let mut group = c.benchmark_group("loopback_lease_drop");
    for active in [1_000, 10_000, 50_000] {
        let (network, _held) = network_with(active);
        for store in [Store::Idle, Store::Socket, Store::Ports] {
            group.bench_with_input(
                BenchmarkId::new(store.name(), active),
                &store,
                |b, &store| b.iter(|| store_life(&network, store)),
            );
        }
    }
    group.finish();
}

/// Run `churners` threads of store rounds for `iters` rounds each while
/// `traffic` threads bind and close ports, returning the churners' time.
fn churn(network: &Shared, churners: usize, traffic: usize, iters: u64) -> Duration {
    let stop = AtomicBool::new(false);
    let barrier = Barrier::new(churners + traffic + 1);
    std::thread::scope(|s| {
        for _ in 0..traffic {
            s.spawn(|| {
                let lease = LoopbackLease::from(Arc::clone(network));
                let owner = lease.owner();
                barrier.wait();
                while !stop.load(Ordering::Relaxed) {
                    bind_and_close(network, owner);
                }
            });
        }
        let workers: Vec<_> = (0..churners)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    for _ in 0..iters {
                        round(network);
                    }
                })
            })
            .collect();
        barrier.wait();
        let start = Instant::now();
        for worker in workers {
            if worker.join().is_err() {
                unreachable!("a churner panicked");
            }
        }
        let elapsed = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        elapsed
    })
}

fn store_churn(c: &mut Criterion) {
    let (network, _held) = network_with(CHURN_ACTIVE);
    let mut group = c.benchmark_group("loopback_store_churn");
    for churners in [1, 8] {
        for traffic in [0, 4] {
            group.throughput(Throughput::Elements((churners * ROUND) as u64));
            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{churners}x{traffic}")),
                &(churners, traffic),
                |b, &(churners, traffic)| {
                    b.iter_custom(|iters| churn(&network, churners, traffic, iters));
                },
            );
        }
    }
    group.finish();
}

/// Time `iters` bind-and-close pairs on one thread while `churners` threads
/// churn stores.
fn ops_under(network: &Shared, churners: usize, iters: u64) -> Duration {
    let stop = AtomicBool::new(false);
    let barrier = Barrier::new(churners + 1);
    let lease = LoopbackLease::from(Arc::clone(network));
    let owner = lease.owner();
    std::thread::scope(|s| {
        for _ in 0..churners {
            s.spawn(|| {
                barrier.wait();
                while !stop.load(Ordering::Relaxed) {
                    round(network);
                }
            });
        }
        barrier.wait();
        let start = Instant::now();
        for _ in 0..iters {
            bind_and_close(network, owner);
        }
        let elapsed = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        elapsed
    })
}

fn socket_ops_under_churn(c: &mut Criterion) {
    let (network, _held) = network_with(CHURN_ACTIVE);
    let mut group = c.benchmark_group("loopback_socket_ops_under_churn");
    for churners in [0, 8] {
        group.bench_with_input(
            BenchmarkId::from_parameter(churners),
            &churners,
            |b, &churners| b.iter_custom(|iters| ops_under(&network, churners, iters)),
        );
    }
    group.finish();
}

criterion_group!(benches, lease_drop, store_churn, socket_ops_under_churn);
criterion_main!(benches);
