//! Container pressure scenarios, run by `bench-tools memory-pressure`.

use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use bench_tools::density::pressure::{Config, Phase, Sample, Shape};
use bench_tools::density::{Report, Step, print_step};
use wash_runtime::engine::{
    Engine,
    ctx::{Ctx, SharedCtx},
    guest_memory::{GuestMemoryMode, install_memory_limiter},
    host_memory::{HostMemoryBudgets, detected_memory_limit, parse_bytes},
    process_memory::{REFRESH_INTERVAL, process_footprint},
};
use wasmtime::component::{Component, Linker, TypedFunc};

const MIB: u64 = 1 << 20;

struct Experiment {
    engine: Engine,
    report: Mutex<Report>,
    path: PathBuf,
}

impl Experiment {
    fn report(&self, phase: Phase, admitted: usize, stop: Option<&str>) -> Result<()> {
        let budget = self.engine.guest_memory();
        let guard = self
            .engine
            .process_memory()
            .context("missing process guard")?;
        let sample = Sample {
            phase,
            guest_refused: budget.refused(),
            initial_refused: budget.initial_refused(),
            process_refused: budget.process_refused(),
            cgroup_current: cgroup_bytes("memory.current").ok(),
            cgroup_peak: cgroup_bytes("memory.peak").ok(),
            process_over: guard.is_over(),
        };
        let mut report = self
            .report
            .lock()
            .map_err(|_| anyhow::anyhow!("pressure report poisoned"))?;
        let refused = sample.guest_refused + sample.initial_refused + sample.process_refused;
        let before = report
            .steps
            .last()
            .and_then(|step| step.pressure.as_ref())
            .map_or(0, |sample| {
                sample.guest_refused + sample.initial_refused + sample.process_refused
            });
        let step = Step {
            pressure: Some(sample),
            workloads: admitted,
            footprint_bytes: process_footprint(),
            guest_memory_bytes: Some(budget.in_use()),
            initial_memory_bytes: Some(budget.initial_uncharged()),
            refused: refused.saturating_sub(before),
            clean: phase == Phase::Touched,
            ..Default::default()
        };
        if phase == Phase::Baseline {
            report.baseline_footprint_bytes = step.footprint_bytes;
        }
        if let Some(stop) = stop {
            report.stop = stop.into();
        }
        print_step(report.steps.is_empty(), &step);
        report.steps.push(step);
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(&[&*report])?)?;
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

struct Guest {
    store: wasmtime::Store<SharedCtx>,
    touch: Vec<TypedFunc<(), ()>>,
}

fn touch_all(experiment: &Experiment, guests: Vec<Guest>) -> Result<()> {
    let workers = guests.len().min(4);
    ensure!(workers > 0, "no guests were admitted");
    let mut chunks: Vec<Vec<Guest>> = (0..workers).map(|_| Vec::new()).collect();
    for (index, guest) in guests.into_iter().enumerate() {
        chunks
            .get_mut(index % workers)
            .context("missing guest worker")?
            .push(guest);
    }
    let admitted = chunks.iter().map(Vec::len).sum();
    let start = Arc::new(Barrier::new(workers + 1));
    let finished = Arc::new(Barrier::new(workers + 1));
    let release = Arc::new(Barrier::new(workers + 1));
    std::thread::scope(|scope| -> Result<()> {
        let mut threads = Vec::new();
        for mut chunk in chunks {
            let start = Arc::clone(&start);
            let finished = Arc::clone(&finished);
            let release = Arc::clone(&release);
            threads.push(scope.spawn(move || -> Result<()> {
                start.wait();
                let result = (|| -> Result<()> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    for guest in &mut chunk {
                        for function in &guest.touch {
                            runtime.block_on(function.call_async(&mut guest.store, ()))?;
                        }
                    }
                    Ok(())
                })();
                finished.wait();
                // Keep every store alive until the final sample is written.
                release.wait();
                result
            }));
        }
        let writing = experiment.report(Phase::Writing, admitted, None);
        start.wait();
        finished.wait();
        let report = experiment.report(Phase::Touched, admitted, None);
        release.wait();
        for thread in threads {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("guest worker panicked"))??;
        }
        writing.and(report)
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list") || !args.iter().any(|arg| arg == "--bench") {
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    ensure!(
        std::env::var("WASH_PRESSURE_RUN").as_deref() == Ok("1"),
        "run with cargo run -p bench-tools -- memory-pressure"
    );
    ensure!(cfg!(target_os = "linux"), "requires Linux cgroup v2");
    let limit = cgroup_bytes("memory.max")?;
    ensure!(
        (384 * MIB..=2048 * MIB).contains(&limit),
        "requires a container limit between 384 MiB and 2 GiB"
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
    let mode: GuestMemoryMode = std::env::var("WASH_PRESSURE_MODE")?
        .parse()
        .map_err(anyhow::Error::msg)?;
    ensure!(
        matches!(mode, GuestMemoryMode::Enforce | GuestMemoryMode::Growth),
        "choose enforce or growth"
    );
    let cow: bool = std::env::var("WASH_PRESSURE_COW")?.parse()?;
    let dirty_bytes = std::env::var("WASH_PRESSURE_INITIAL").map_or_else(
        |_| Ok(MIB),
        |raw| parse_bytes(&raw).map_err(anyhow::Error::msg),
    )?;
    ensure!(
        dirty_bytes == MIB || dirty_bytes == 2 * MIB,
        "initial memory must be 1 MiB or 2 MiB"
    );
    let shape: Shape = std::env::var("WASH_PRESSURE_SHAPE")?
        .parse()
        .map_err(anyhow::Error::msg)?;
    let (memories, bytes_per_memory, data_bytes) = shape.dimensions(dirty_bytes / MIB);
    let initial_bytes = memories * bytes_per_memory;
    let guest_cap = std::env::var("WASH_PRESSURE_GUEST_CAP")
        .ok()
        .map(|raw| parse_bytes(&raw).map_err(anyhow::Error::msg))
        .transpose()?;
    let mut builder = Engine::builder()
        .with_host_memory(
            HostMemoryBudgets::resolve(guest_cap, Some(64 * MIB), Some(8192), None)
                .map_err(anyhow::Error::msg)?,
        )
        .with_guest_memory_mode(mode)
        .with_memory_init_cow(cow)
        .with_fuel_consumption(false);
    if mode == GuestMemoryMode::Growth
        && let Ok(raw) = std::env::var("WASH_PRESSURE_INITIAL_CAP")
    {
        builder = builder
            .with_max_initial_guest_memory_bytes(parse_bytes(&raw).map_err(anyhow::Error::msg)?);
    }
    let engine = builder.build()?;
    let guard = engine.process_memory().context("missing guard")?;
    let config = Config {
        shape,
        cow: engine.inner().get_memory_init_cow(),
        initial_bytes,
        memories,
        swap_limit: cgroup_bytes("memory.swap.max")?,
        guest_limit: engine.guest_memory().cap(),
        initial_limit: engine.guest_memory().initial_cap(),
        process_limit: guard.limit_bytes(),
        architecture: std::env::consts::ARCH.into(),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease").ok(),
        transparent_hugepage: std::fs::read_to_string(
            "/sys/kernel/mm/transparent_hugepage/enabled",
        )
        .ok(),
        outcome: None,
    };
    let report = Report {
        bench: "memory_pressure".into(),
        label: std::env::var("WASH_PRESSURE_LABEL")?,
        subject: shape.name().into(),
        guest_memory_mode: Some(mode.as_str().into()),
        cores: 2,
        memory_limit_bytes: limit,
        pressure: Some(config),
        ..Default::default()
    };
    let experiment = Experiment {
        engine,
        report: Mutex::new(report),
        path: std::env::var_os("WASH_PRESSURE_REPORT")
            .context("missing report path")?
            .into(),
    };
    let engine = &experiment.engine;
    let mut source = String::from("(component");
    for index in 0..memories {
        let suffix = char::from(b'a' + u8::try_from(index)?);
        source.push_str(&format!(
            r#"
            (core module $m{index}
                (memory {} 1024)
                (data (i32.const 0) "{}")
                (func (export "warm")
                    (memory.fill (i32.const 0) (i32.const 17) (i32.const 65536)))
                (func (export "touch")
                    (memory.fill (i32.const 0) (i32.const 29) (i32.const {}))))
            (core instance $i{index} (instantiate $m{index}))
            (func (export "warm-{suffix}") (canon lift (core func $i{index} "warm")))
            (func (export "touch-{suffix}") (canon lift (core func $i{index} "touch")))"#,
            bytes_per_memory / 65536,
            "\\ab".repeat(usize::try_from(data_bytes)?),
            if shape == Shape::Dirty {
                bytes_per_memory
            } else {
                65536
            },
        ));
    }
    source.push(')');
    let wasm = wat::parse_str(&source)?;
    drop(source);
    let component = Component::new(engine.inner(), wasm)?;
    let pre = Linker::<SharedCtx>::new(engine.inner()).instantiate_pre(&component)?;
    experiment.report(Phase::Baseline, 0, None)?;
    let mut guests = Vec::new();
    let stop = loop {
        if let Err(error) = engine
            .process_memory()
            .context("missing guard")?
            .admit_start()
        {
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
        if (guests.len() as u64).is_multiple_of(16 * MIB / initial_bytes) {
            // Allow a fresh guard sample between bounded admission batches.
            tokio::time::sleep(REFRESH_INTERVAL + Duration::from_millis(5)).await;
        }
    };
    experiment.report(Phase::Ready, guests.len(), Some(&stop))?;
    touch_all(&experiment, guests)?;
    ensure!(engine.guest_memory().in_use() == 0, "charged bytes leaked");
    ensure!(
        engine.guest_memory().initial_uncharged() == 0,
        "initial bytes leaked"
    );
    experiment.report(Phase::Released, 0, None)
}
