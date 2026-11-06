//! Container pressure scenarios, run by `bench-tools memory-pressure`.
//!
//! Each sample after `baseline` is written only after the guest accounting
//! it implies has been checked, so a missing sample means a failed check.

use std::path::PathBuf;
use std::sync::Barrier;

use anyhow::{Context as _, Result, ensure};
use bench_tools::pressure::{
    CASE_ENV, Case, Host, MIB, Mode, Phase, REPORT_ENV, Report, Sample, WARM_BYTES, print_sample,
};

const WASM_PAGE: u64 = 64 * 1024;
use wash_runtime::engine::{
    Engine,
    ctx::{Ctx, SharedCtx},
    guest_memory::{FIRST_ALLOCATION_UNCHARGED, GuestMemoryMode, install_memory_limiter},
    host_memory::{HostMemoryBudgets, detected_memory_limit},
    process_memory::process_footprint,
};
use wasmtime::component::{Component, Linker, TypedFunc};

/// Heap ceiling per memory: each synthetic memory's maximum is 1024 pages.
/// Pool slots stay the default reservation plus guard.
const HEAP_BYTES: u64 = 64 * MIB;
/// Pool memories: room for `4 × G` of 1 MiB memories at the largest limit.
const MAX_MEMORIES: u32 = 8192;

/// Stores the budgets alone admit, before the guard or the pool can stop admission.
fn budget_stores(case: &Case, guest_cap: u64, initial_cap: u64) -> u64 {
    let (memories, per_memory, _) = case.dimensions();
    let uncharged = match case.mode {
        Mode::Enforce => 0,
        Mode::Growth => memories * per_memory.min(FIRST_ALLOCATION_UNCHARGED),
    };
    let charged = case.initial_bytes() - uncharged;
    let by_charged = guest_cap.checked_div(charged).unwrap_or(u64::MAX);
    let by_uncharged = initial_cap.checked_div(uncharged).unwrap_or(u64::MAX);
    by_charged.min(by_uncharged)
}

struct Recorder {
    report: Report,
    path: PathBuf,
}

impl Recorder {
    /// Append a sample and replace the report file atomically.
    fn record(&mut self, engine: &Engine, phase: Phase, stores: usize) -> Result<()> {
        let budget = engine.guest_memory();
        let guard = engine.process_memory().context("missing process guard")?;
        let sample = Sample {
            phase,
            stores,
            footprint_bytes: process_footprint(),
            guest_memory_bytes: budget.in_use(),
            initial_memory_bytes: budget.initial_uncharged(),
            cgroup_current: cgroup_bytes("memory.current").ok(),
            cgroup_peak: cgroup_bytes("memory.peak").ok(),
            guest_refused: budget.refused(),
            initial_refused: budget.initial_refused(),
            process_refused: budget.process_refused(),
            process_over: guard.is_over(),
        };
        print_sample(self.report.samples.is_empty(), &sample);
        self.report.samples.push(sample);
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(&self.report)?)?;
        std::fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

fn cgroup_bytes(name: &str) -> Result<u64> {
    std::fs::read_to_string(format!("/sys/fs/cgroup/{name}"))?
        .trim()
        .parse()
        .with_context(|| format!("{name} must be a finite cgroup v2 byte count"))
}

/// CPUs granted by `cpu.max` (`<quota> <period>`); `None` when unlimited.
fn cgroup_cpus() -> Option<f64> {
    let raw = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let (quota, period) = raw.trim().split_once(' ')?;
    Some(quota.parse::<f64>().ok()? / period.parse::<f64>().ok()?)
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

struct Guest {
    store: wasmtime::Store<SharedCtx>,
    touch: Vec<TypedFunc<(), ()>>,
}

/// Write from up to four threads at once, then hand every store back alive.
fn touch_all(guests: Vec<Guest>) -> Result<Vec<Guest>> {
    let size = guests.len().div_ceil(4).max(1);
    let mut chunks = Vec::new();
    let mut remaining = guests.into_iter();
    loop {
        let chunk: Vec<Guest> = remaining.by_ref().take(size).collect();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);
    }
    let start = Barrier::new(chunks.len());
    std::thread::scope(|scope| {
        let workers: Vec<_> = chunks
            .into_iter()
            .map(|mut chunk| {
                let start = &start;
                scope.spawn(move || -> Result<Vec<Guest>> {
                    start.wait();
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    for guest in &mut chunk {
                        for function in &guest.touch {
                            runtime.block_on(function.call_async(&mut guest.store, ()))?;
                        }
                    }
                    Ok(chunk)
                })
            })
            .collect();
        let mut guests = Vec::new();
        for worker in workers {
            guests.extend(
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("guest worker panicked"))??,
            );
        }
        Ok(guests)
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list") || !args.iter().any(|arg| arg == "--bench") {
        return Ok(());
    }
    // The runtime's guard and budget warnings are rate-limited; keep them in stderr.log.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let case: Case = serde_json::from_str(&std::env::var(CASE_ENV).with_context(|| {
        format!("{CASE_ENV} is unset; run with cargo run -p bench-tools -- memory-pressure")
    })?)?;
    let path: PathBuf = std::env::var_os(REPORT_ENV)
        .with_context(|| format!("{REPORT_ENV} is unset"))?
        .into();
    ensure!(cfg!(target_os = "linux"), "requires Linux cgroup v2");
    let limit = cgroup_bytes("memory.max")?;
    ensure!(
        limit == case.memory_mib * MIB,
        "cgroup limit {limit} differs from the requested {} MiB",
        case.memory_mib
    );
    ensure!(
        cgroup_bytes("memory.swap.max")? == 0,
        "swap must be disabled"
    );
    ensure!(
        detected_memory_limit() == Some(limit),
        "runtime limit differs from cgroup limit"
    );
    ensure!(
        process_footprint().is_some(),
        "process footprint is unavailable"
    );
    let mode = match case.mode {
        Mode::Enforce => GuestMemoryMode::Enforce,
        Mode::Growth => GuestMemoryMode::Growth,
    };
    let (memories, bytes_per_memory, data_bytes) = case.dimensions();
    let mut builder = Engine::builder()
        .with_host_memory(
            HostMemoryBudgets::resolve(
                case.guest_memory_mib.map(|mib| mib * MIB),
                Some(HEAP_BYTES),
                Some(MAX_MEMORIES),
                None,
            )
            .map_err(anyhow::Error::msg)?,
        )
        .with_guest_memory_mode(mode)
        .with_memory_init_cow(case.cow)
        .with_fuel_consumption(false);
    let initial_cap = case.initial_cap_mib.filter(|_| case.mode == Mode::Growth);
    if let Some(cap) = initial_cap {
        builder = builder.with_max_initial_guest_memory_bytes(cap * MIB);
    }
    let engine = builder.build()?;
    let budget = engine.guest_memory();
    ensure!(
        engine.inner().get_memory_init_cow() == case.cow,
        "copy-on-write setting was not applied"
    );
    if let Some(mib) = case.guest_memory_mib {
        ensure!(
            budget.cap() == mib * MIB,
            "guest memory cap was not applied"
        );
    }
    if let Some(mib) = initial_cap {
        ensure!(
            budget.initial_cap() == mib * MIB,
            "initial cap was not applied"
        );
    }
    // `enforce` ignores the initial cap the engine always installs.
    let initial_limit = (case.mode == Mode::Growth).then(|| budget.initial_cap());
    let budget_stores = budget_stores(&case, budget.cap(), initial_limit.unwrap_or(u64::MAX));
    ensure!(
        budget_stores.saturating_mul(memories) <= u64::from(MAX_MEMORIES),
        "the budgets admit {budget_stores} stores ({} memories), more than the pool's \
         {MAX_MEMORIES}; lower --guest-memory-mib or --initial-cap-mib",
        budget_stores.saturating_mul(memories)
    );
    let mut report = Report::new(case.clone());
    report.host = Some(Host {
        architecture: std::env::consts::ARCH.into(),
        kernel: read_trimmed("/proc/sys/kernel/osrelease"),
        transparent_hugepage: read_trimmed("/sys/kernel/mm/transparent_hugepage/enabled"),
        shmem_hugepage: read_trimmed("/sys/kernel/mm/transparent_hugepage/shmem_enabled"),
        cpus: cgroup_cpus(),
        memory_limit: limit,
        guest_limit: budget.cap(),
        initial_limit,
        budget_stores,
        process_limit: engine
            .process_memory()
            .context("missing process guard")?
            .limit_bytes(),
    });
    let mut recorder = Recorder { report, path };

    let mut source = String::from("(component");
    for index in 0..memories {
        let suffix = char::from(b'a' + u8::try_from(index)?);
        source.push_str(&format!(
            r#"
            (core module $m{index}
                (memory {} 1024)
                (data (i32.const 0) "{}")
                (func (export "warm")
                    (memory.fill (i32.const 0) (i32.const 17) (i32.const {WARM_BYTES})))
                (func (export "touch")
                    (memory.fill (i32.const 0) (i32.const 29) (i32.const {}))))
            (core instance $i{index} (instantiate $m{index}))
            (func (export "warm-{suffix}") (canon lift (core func $i{index} "warm")))
            (func (export "touch-{suffix}") (canon lift (core func $i{index} "touch")))"#,
            bytes_per_memory / WASM_PAGE,
            "\\ab".repeat(usize::try_from(data_bytes)?),
            if case.shape == bench_tools::pressure::Shape::Dirty {
                bytes_per_memory
            } else {
                WARM_BYTES
            },
        ));
    }
    source.push(')');
    let component = Component::new(engine.inner(), wat::parse_str(&source)?)?;
    drop(source);
    let pre = Linker::<SharedCtx>::new(engine.inner()).instantiate_pre(&component)?;
    recorder.record(&engine, Phase::Baseline, 0)?;

    // Admit without pauses, as a burst of real starts would; the guard
    // refreshes its own reading at most every `REFRESH_INTERVAL`.
    let guard = engine.process_memory().context("missing process guard")?;
    let mut guests = Vec::new();
    let stop = loop {
        if let Err(error) = guard.admit_start() {
            break error;
        }
        let ctx = SharedCtx::new(Ctx::builder("pressure", "memory").build())
            .with_guest_memory(engine.guest_memory());
        let mut store = wasmtime::Store::new(engine.inner(), ctx);
        install_memory_limiter(&mut store);
        store.set_epoch_deadline(1_000_000_000);
        let instance = match pre.instantiate_async(&mut store).await {
            Ok(instance) => instance,
            Err(error) => break format!("{error:#}"),
        };
        let mut touch = Vec::new();
        for index in 0..memories {
            let suffix = char::from(b'a' + u8::try_from(index)?);
            instance
                .get_typed_func::<(), ()>(&mut store, &format!("warm-{suffix}"))?
                .call_async(&mut store, ())
                .await?;
            touch.push(instance.get_typed_func::<(), ()>(&mut store, &format!("touch-{suffix}"))?);
        }
        guests.push(Guest { store, touch });
    };
    recorder.report.stop = stop;

    let admitted = guests.len() as u64;
    let uncharged = match case.mode {
        Mode::Enforce => 0,
        Mode::Growth => memories * bytes_per_memory.min(FIRST_ALLOCATION_UNCHARGED),
    };
    let charged = admitted * (case.initial_bytes() - uncharged);
    let uncharged = admitted * uncharged;
    ensure!(
        budget.in_use() == charged && budget.initial_uncharged() == uncharged,
        "guest reservations ({} charged, {} uncharged) differ from the retained memories \
         ({charged} charged, {uncharged} uncharged)",
        budget.in_use(),
        budget.initial_uncharged()
    );
    ensure!(
        charged <= budget.cap() && uncharged <= budget.initial_cap(),
        "guest reservations exceeded their budgets"
    );
    recorder.record(&engine, Phase::Ready, guests.len())?;

    let guests = touch_all(guests)?;
    ensure!(
        budget.in_use() == charged && budget.initial_uncharged() == uncharged,
        "guest reservations changed during writes"
    );
    recorder.record(&engine, Phase::Touched, guests.len())?;

    drop(guests);
    ensure!(budget.in_use() == 0, "charged bytes leaked");
    ensure!(budget.initial_uncharged() == 0, "initial bytes leaked");
    recorder.record(&engine, Phase::Released, 0)
}
