//! Container pressure scenarios, run by scripts/bench/guest-memory-pressure.py.

use std::io::Write as _;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use wash_runtime::engine::{
    Engine,
    ctx::{Ctx, SharedCtx},
    guest_memory::{GuestMemoryMode, install_memory_limiter},
    host_memory::{HostMemoryBudgets, detected_memory_limit, parse_bytes},
    process_memory::{REFRESH_INTERVAL, process_footprint},
};
use wasmtime::component::{Component, Linker, TypedFunc};

const MIB: u64 = 1 << 20;

#[derive(Serialize)]
struct Sample<'a> {
    event: &'a str,
    mode: &'a str,
    cow: bool,
    shape: &'a str,
    memories: u64,
    admitted: usize,
    initial_bytes: u64,
    limit: u64,
    swap_limit: u64,
    guest_limit: u64,
    initial_limit: u64,
    process_limit: u64,
    charged: u64,
    uncharged: u64,
    guest_refused: u64,
    initial_refused: u64,
    process_refused: u64,
    footprint: Option<u64>,
    cgroup_current: Option<u64>,
    cgroup_peak: Option<u64>,
    process_over: bool,
    stop: Option<&'a str>,
    architecture: &'static str,
    kernel: Option<String>,
    transparent_hugepage: Option<String>,
}

struct Experiment {
    engine: Engine,
    initial_bytes: u64,
    limit: u64,
    shape: String,
    memories: u64,
}

impl Experiment {
    fn report(&self, event: &str, admitted: usize, stop: Option<&str>) -> Result<()> {
        let budget = self.engine.guest_memory();
        let guard = self
            .engine
            .process_memory()
            .context("missing process guard")?;
        let sample = Sample {
            event,
            mode: budget.mode().as_str(),
            cow: self.engine.inner().get_memory_init_cow(),
            shape: &self.shape,
            memories: self.memories,
            admitted,
            initial_bytes: self.initial_bytes,
            limit: self.limit,
            swap_limit: cgroup_bytes("memory.swap.max")?,
            guest_limit: budget.cap(),
            initial_limit: budget.initial_cap(),
            process_limit: guard.limit_bytes(),
            charged: budget.in_use(),
            uncharged: budget.initial_uncharged(),
            guest_refused: budget.refused(),
            initial_refused: budget.initial_refused(),
            process_refused: budget.process_refused(),
            footprint: process_footprint(),
            cgroup_current: cgroup_bytes("memory.current").ok(),
            cgroup_peak: cgroup_bytes("memory.peak").ok(),
            process_over: guard.is_over(),
            stop,
            architecture: std::env::consts::ARCH,
            kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease").ok(),
            transparent_hugepage: std::fs::read_to_string(
                "/sys/kernel/mm/transparent_hugepage/enabled",
            )
            .ok(),
        };
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &sample)?;
        writeln!(stdout)?;
        stdout.flush()?;
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
        let writing = experiment.report("writing", admitted, None);
        start.wait();
        finished.wait();
        let report = experiment.report("touched", admitted, None);
        release.wait();
        for thread in threads {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("guest worker panicked"))??;
        }
        writing.and(report)
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    ensure!(
        std::env::var("WASH_PRESSURE_RUN").as_deref() == Ok("1"),
        "run with python3 scripts/bench/guest-memory-pressure.py"
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
    let shape = std::env::var("WASH_PRESSURE_SHAPE")?;
    let (memories, bytes_per_memory, data_bytes) = match shape.as_str() {
        "sparse" => (1, MIB, 4096),
        "data-heavy" => (1, 8 * MIB, 8 * MIB),
        "multiple" => (4, MIB, 4096),
        "dirty" => (1, dirty_bytes, 4096),
        _ => anyhow::bail!("unknown shape: {shape}"),
    };
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
    let experiment = Experiment {
        engine: builder.build()?,
        initial_bytes,
        limit,
        shape,
        memories,
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
            if experiment.shape == "dirty" {
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
    experiment.report("baseline", 0, None)?;
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
    experiment.report("ready", guests.len(), Some(&stop))?;
    touch_all(&experiment, guests)?;
    ensure!(engine.guest_memory().in_use() == 0, "charged bytes leaked");
    ensure!(
        engine.guest_memory().initial_uncharged() == 0,
        "initial bytes leaked"
    );
    experiment.report("released", 0, None)
}
