//! WebAssembly component engine for executing workloads.
//!
//! This module provides the core engine functionality for compiling and executing
//! WebAssembly components. The [`Engine`] is responsible for:
//!
//! - Compiling WebAssembly components using wasmtime
//! - Initializing workloads with their components and dependencies
//! - Managing volume mounts and resource configurations
//! - Setting up WASI and HTTP interfaces for components
//!
//! # Key Types
//!
//! - [`Engine`] - The main engine for WebAssembly execution
//! - [`EngineBuilder`] - Builder for configuring engine settings
//! - [`WorkloadComponent`] - Individual components within a workload
//!
//! # Example
//!
//! ```no_run
//! use wash_runtime::engine::Engine;
//! use wash_runtime::types::Workload;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let engine = Engine::builder().build()?;
//! let workload = Workload {
//!     namespace: "default".to_string(),
//!     name: "my-workload".to_string(),
//!     // ... other fields
//! #   annotations: std::collections::HashMap::new(),
//! #   service: None,
//! #   components: vec![],
//! #   host_interfaces: vec![],
//! #   volumes: vec![],
//! };
//!
//! let unresolved = engine.initialize_workload("workload-1", workload).await?;
//! // ... bind to plugins and resolve
//! # Ok(())
//! # }
//! ```

use std::hash::Hash;
use std::time::Duration;

use crate::cache::{Cache, DefaultCache, coalescer::Coalescer};
use crate::sockets::loopback;
use anyhow::{Context, bail};
use tracing::{instrument, warn};
use wasmtime::PoolingAllocationConfig;
use wasmtime::component::{Component, Linker};

use crate::engine::ctx::SharedCtx;
#[cfg(feature = "wasi-tls")]
use crate::engine::ctx::SharedTlsProvider;
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent, WorkloadService};
use crate::types::{EmptyDirVolume, HostPathVolume, VolumeType, Workload};
use std::env;
use std::str::FromStr;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use wasmtime_wasi::WasiView;

// Exact instance versions registered by the Wasmtime bindings below. Keep
// these aligned when upgrading Wasmtime: partial overrides must reopen those
// instances, since Linker::instance does not apply semver matching. The override
// integration tests check both exact and compatible guest imports.
pub(crate) const WASI_LINKER_VERSIONS: [semver::Version; 2] = [
    semver::Version::new(0, 2, 12),
    semver::Version::new(0, 3, 0),
];

/// Add all WASI interfaces to the linker, using upstream for non-socket interfaces
/// and our custom socket implementation (with loopback support) for socket interfaces.
/// Both P2 and P3 bindings are registered.
fn add_wasi_to_linker(linker: &mut Linker<SharedCtx>) -> anyhow::Result<()> {
    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random, sockets};

    // IO interfaces (error, poll, streams)
    wasmtime_wasi_io::add_to_linker_async(linker)?;

    // Filesystem (async version)
    filesystem::types::add_to_linker::<SharedCtx, wasmtime_wasi::filesystem::WasiFilesystem>(
        linker,
        <SharedCtx as wasmtime_wasi::filesystem::WasiFilesystemView>::filesystem,
    )?;
    filesystem::preopens::add_to_linker::<SharedCtx, SharedCtx>(linker, ctx::extract_active_ctx)?;

    // Clocks
    clocks::wall_clock::add_to_linker::<SharedCtx, wasmtime_wasi::clocks::WasiClocks>(
        linker,
        <SharedCtx as wasmtime_wasi::clocks::WasiClocksView>::clocks,
    )?;
    clocks::monotonic_clock::add_to_linker::<SharedCtx, wasmtime_wasi::clocks::WasiClocks>(
        linker,
        <SharedCtx as wasmtime_wasi::clocks::WasiClocksView>::clocks,
    )?;

    // Random
    random::random::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(linker, |t| {
        t.ctx().ctx.random()
    })?;
    random::insecure::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(linker, |t| {
        t.ctx().ctx.random()
    })?;
    random::insecure_seed::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(
        linker,
        |t| t.ctx().ctx.random(),
    )?;

    // CLI
    cli::exit::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::environment::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stdin::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stdout::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stderr::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_input::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_output::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stdin::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stdout::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stderr::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;

    // Socket interfaces — use OUR implementation with loopback support
    sockets::tcp::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::udp::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::tcp_create_socket::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::udp_create_socket::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::instance_network::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    let net_options = sockets::network::LinkOptions::default();
    sockets::network::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        &net_options,
        ctx::extract_sockets,
    )?;
    sockets::ip_name_lookup::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;

    // CLI, clocks, filesystem, random — upstream P3 add_to_linker
    wasmtime_wasi::p3::cli::add_to_linker(linker)?;
    wasmtime_wasi::p3::clocks::add_to_linker(linker)?;
    wasmtime_wasi::p3::bindings::filesystem::types::add_to_linker::<
        SharedCtx,
        wasmtime_wasi::filesystem::WasiFilesystem,
    >(
        linker,
        <SharedCtx as wasmtime_wasi::filesystem::WasiFilesystemView>::filesystem,
    )?;
    wasmtime_wasi::p3::bindings::filesystem::preopens::add_to_linker::<SharedCtx, SharedCtx>(
        linker,
        ctx::extract_active_ctx,
    )?;
    wasmtime_wasi::p3::random::add_to_linker(linker)?;

    // Sockets with our custom P3 implementation (with loopback)
    crate::sockets::add_p3_to_linker(linker)?;

    // wasi:tls@0.3.0-draft (p3).
    #[cfg(feature = "wasi-tls")]
    wasmtime_wasi_tls::p3::add_to_linker(linker)?;

    Ok(())
}

/// Detect whether a component targets WASIP3 by checking for `@0.3` in WASI imports/exports.
/// This is a pure detection function used to pick the P2 vs P3 dispatch path; P3 bindings
/// are always registered on the linker.
pub fn targets_wasip3(component: &Component) -> bool {
    let ty = component.component_type();
    let engine = component.engine();
    ty.imports(engine)
        .any(|(import, _)| import.starts_with("wasi:") && import.contains("@0.3"))
        || ty
            .exports(engine)
            .any(|(export, _)| export.starts_with("wasi:") && export.contains("@0.3"))
}

/// Detect whether a component targets WASIP3 HTTP specifically by checking for
/// `wasi:http` imports/exports with `@0.3`. Used for HTTP dispatch to avoid
/// routing a component that imports `wasi:cli@0.3` but exports `wasi:http@0.2`
/// through the P3 HTTP handler.
pub fn targets_wasip3_http(component: &Component) -> bool {
    let ty = component.component_type();
    let engine = component.engine();
    ty.imports(engine)
        .any(|(name, _)| name.starts_with("wasi:http") && name.contains("@0.3"))
        || ty
            .exports(engine)
            .any(|(name, _)| name.starts_with("wasi:http") && name.contains("@0.3"))
}

pub mod abandon;
pub mod command;
pub mod ctx;
pub mod dispatch;
pub mod guest_memory;
pub(crate) mod instance_driver;
pub(crate) mod instance_pool;
pub(crate) mod lent_reply;
pub mod stream_payload;
pub use instance_pool::{InstancePolicy, ReclaimPolicy};
#[cfg(any(test, all(target_os = "linux", target_pointer_width = "64")))]
mod address_space;
pub mod host_memory;
pub(crate) mod linked_call;
mod pool_gc;
pub mod pool_share;
mod pool_startup;
pub mod pool_usage;
pub mod process_memory;
mod scratch;
pub(crate) mod store;
pub(crate) mod store_settings;
pub(crate) mod trap_remediation;
mod value;
mod volumes;
pub mod workload;

/// How often the engine's epoch advances.
///
/// This is the resolution of the deadline checks compiled into every guest, not
/// how often any store is interrupted: what a store does when its deadline
/// passes is [`crate::engine::abandon::arm_epoch_deadline`]'s decision. It
/// bounds how promptly an abandoned call can be noticed.
pub(crate) const EPOCH_TICK: Duration = Duration::from_millis(10);

/// What the pooling allocator was configured with, read back from the installed
/// [`PoolingAllocationConfig`].
///
/// [`host_memory::HostMemoryBudgets`] holds what was asked for. The environment
/// moves every total and the heap ceiling independently, so the two can
/// disagree.
#[derive(Debug, Clone, Copy)]
struct InstalledPool {
    max_memory_size: u64,
    /// The per-memory layout the slots were sized with.
    layout: host_memory::MemoryLayout,
    /// Elements each pooled table has room for, which sizes the table pool.
    table_elements: usize,
    totals: pool_usage::PoolTotals,
}

impl InstalledPool {
    /// Address space reserved by the installed memory layout and pool totals.
    fn reservation(&self) -> u64 {
        host_memory::pool_slab_bytes(self.max_memory_size, self.totals.memories, self.layout)
    }
}

/// Start the thread that advances `engine`'s epoch every [`EPOCH_TICK`].
///
/// Holds a [`wasmtime::Engine::weak`] handle rather than the engine itself, so
/// the ticker does not keep a dropped engine alive: the next tick after the last
/// strong reference goes away fails to upgrade and the thread ends. One ticker
/// per engine, started at construction — an engine built with
/// `epoch_interruption` whose epoch never advances would hang any store that
/// armed a deadline.
fn spawn_epoch_ticker(engine: &wasmtime::Engine) -> anyhow::Result<()> {
    let weak = engine.weak();
    std::thread::Builder::new()
        .name("wasmtime-epoch-ticker".to_string())
        .spawn(move || {
            while let Some(engine) = weak.upgrade() {
                engine.increment_epoch();
                // Drop the strong reference before sleeping, so a dropped
                // engine is not kept alive for a whole tick.
                drop(engine);
                std::thread::sleep(EPOCH_TICK);
            }
        })
        .context("failed to spawn the epoch ticker thread")?;
    Ok(())
}

/// Runs `f`, which builds a `wasmtime::Engine`, where the process-wide trap
/// handler that the first engine installs cannot be killed by a signal.
///
/// On macOS wasmtime handles traps with Mach ports: building the first
/// engine spawns a thread that blocks in `mach_msg` for exceptions, and in
/// wasmtime 48 that receive aborts the process on any result it does not
/// expect, which a signal delivered to that thread produces (`SIGCHLD` from
/// a child this host spawned, say). The abort prints nothing a test harness
/// shows (<https://github.com/bytecodealliance/wasmtime/issues/14393>;
/// wasmtime 50 drops the interruptible receive). A thread inherits the
/// signal mask of the thread that spawns it, and wasmtime spawns the
/// handler from the thread building the first engine, so every engine here
/// is built on a thread with every signal blocked: the handler never takes
/// a process-directed signal, and the kernel delivers it elsewhere.
/// Elsewhere `f` runs inline. Remove once the workspace is on wasmtime 50.
// The signal mask is set with libc, as the process footprint is read.
#[allow(unsafe_code)]
fn on_signal_quiet_thread<T: Send>(
    f: impl FnOnce() -> anyhow::Result<T> + Send,
) -> anyhow::Result<T> {
    #[cfg(target_os = "macos")]
    {
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("wasmtime-engine-build".to_string())
                .spawn_scoped(scope, || {
                    // SAFETY: `sigfillset` and `pthread_sigmask` only read
                    // and write the set on this thread's stack, and the
                    // mask applies to this thread alone, plus whatever it
                    // spawns.
                    unsafe {
                        let mut all: libc::sigset_t = std::mem::zeroed();
                        libc::sigfillset(&mut all);
                        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
                    }
                    f()
                })
                .context("failed to spawn the engine build thread")?
                .join()
                .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        f()
    }
}

/// Directory handles are released before their scratch directory is removed,
/// including when asynchronous component preparation fails or is cancelled.
#[derive(Default)]
struct WorkloadVolumes {
    opened: std::collections::HashMap<String, volumes::OpenedVolume>,
    scratch: Option<Arc<scratch::ScratchDir>>,
}

/// The core WebAssembly engine for executing components and workloads.
///
/// The `Engine` is responsible for compiling WebAssembly components, managing
/// their lifecycle, and providing the runtime environment for execution.
/// It wraps a wasmtime engine with additional functionality for workload management.
#[derive(Clone)]
pub struct Engine {
    stream_payloads: Arc<stream_payload::StreamPayloads>,
    // wasmtime engine
    pub(crate) inner: wasmtime::Engine,
    pub(crate) cache: Arc<dyn Cache>,
    /// The built-in backend, when no other was selected, for tests to inspect.
    #[cfg(test)]
    pub(crate) default_cache: Option<DefaultCache>,
    cache_engine_id: uuid::Uuid,
    /// Cache lookups in flight, shared by digest and abandoned with their
    /// last caller.
    compile_flights: Arc<Coalescer<String, Arc<LiveCompile>>>,
    /// Compiles and their publication, which finish whether or not the caller
    /// that began them still waits.
    compiling: Arc<Coalescer<String, Arc<LiveCompile>>>,
    /// Compiles running workloads hold, consulted before the cache so a live
    /// compile can be reused after cache eviction. See [`LiveCompiles`].
    live_compiles: Arc<LiveCompiles>,
    /// Compiles that actually ran on this engine.
    ///
    /// Test-only, and there is no way to do without it: what the digest-keyed
    /// cache buys is that a herd of replicas shares one compile, and from
    /// outside, one compile and fifteen racing to insert the same key leave
    /// exactly the same single cache entry.
    #[cfg(test)]
    pub(crate) compiles: Arc<std::sync::atomic::AtomicUsize>,
    /// Host-level socket policy every workload on this engine inherits:
    /// enforcement mode, address ranges, whether the host-loopback door is open,
    /// the host's port table, and the connection budget. The workload-level half
    /// (`allowedHosts`, `allowedHostLoopbackPorts`) is layered over it per component.
    pub(crate) socket_policy: Arc<crate::sockets::policy::SocketPolicy>,
    /// Which host paths a workload's `hostPath` volume may name.
    host_paths: volumes::HostPathPolicy,
    /// Where `emptyDir` volumes live. An error when the root could not be
    /// claimed, reported to the first workload that asks for one.
    scratch: Result<Arc<scratch::ScratchRoot>, Arc<str>>,
    pub(crate) host_memory: host_memory::HostMemoryBudgets,
    /// The host-wide counter of guest linear-memory bytes that
    /// [`host_memory::HostMemoryBudgets::max_guest_memory`] is the cap on.
    /// Every store this engine builds carries a limiter drawing on it.
    pub(crate) guest_memory: Arc<guest_memory::GuestMemoryBudget>,
    /// What every store this engine builds is configured with.
    pub(crate) store_settings: store_settings::StoreSettings,
    /// The whole process's footprint against `max_process_memory`, sampled on
    /// its own thread. The guest budget refuses growth while it is over.
    /// `None` when the guard was switched off.
    process_memory: Option<Arc<process_memory::ProcessMemoryGuard>>,
    /// TLS provider override for `wasi:tls` client connections.
    #[cfg(feature = "wasi-tls")]
    pub(crate) tls_provider: Option<SharedTlsProvider>,
    /// Linear memory the pooling allocator actually admits per memory, read
    /// back from the installed [`PoolingAllocationConfig`]. `None` when
    /// pooling is off, i.e. when no ceiling is enforced.
    ///
    /// Captured for the same reason as `pool_totals`:
    /// `--default-heap-memory` is only the *requested* size, and
    /// `WASMTIME_POOLING_MAX_MEMORY_SIZE` or a caller-supplied pooling config
    /// replaces it outright. Quoting the flag in a refusal that the installed
    /// ceiling caused produces advice that cannot work.
    installed_heap_memory: Option<u64>,
    /// Every total the installed pool was built with, read back from the
    /// [`PoolingAllocationConfig`] actually installed so it reflects the
    /// `WASMTIME_POOLING_TOTAL_*` overrides and a caller-supplied pooling
    /// config alike. `None` when pooling is off or unsupported.
    ///
    /// Recorded rather than recomputed: re-deriving `max_memories.unwrap_or(…)`
    /// somewhere else would miss the env override (applied inside
    /// [`new_pooling_config`]) and drift the moment either side changed.
    pool_totals: Option<pool_usage::PoolTotals>,
    /// The linkers every workload component starts from, built once and
    /// shared (see [`BaseLinkers`]).
    base_linkers: Arc<BaseLinkers>,
}

/// The WASI linker, with and without `wasi:http`, that every workload
/// component starts from.
///
/// Building one is the largest part of what an idle workload costs the host:
/// about 140 KiB for WASI and 180 KiB with `wasi:http`, most of a
/// scale-to-zero workload. What goes into them depends on nothing about the
/// workload, so each is built once per engine and shared; a component's
/// [`workload::WorkloadMetadata::linker`] copies its own only when a plugin
/// or a link writes to it.
#[derive(Default)]
struct BaseLinkers {
    wasi: OnceLock<Arc<Linker<SharedCtx>>>,
    wasi_http: OnceLock<Arc<Linker<SharedCtx>>>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Engine");
        #[cfg(feature = "wasi-tls")]
        s.field("tls_provider", &self.tls_provider.is_some());
        s.finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CacheKey {
    engine: uuid::Uuid,
    digest: String,
}

impl CacheKey {
    /// Process-local identity of the engine that owns this compile.
    pub fn engine(&self) -> uuid::Uuid {
        self.engine
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// The cache identity of a component's bytes: `sha256:` and their hex hash.
pub(crate) fn content_digest(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// One compile, and whether a store of it may need a GC heap: read off the
/// source the first time a workload asks, and kept with the compile so it is
/// never read again. Services never ask.
#[derive(Clone)]
pub struct CacheValue {
    component: Component,
    may_use_gc: Arc<std::sync::OnceLock<bool>>,
}

impl CacheValue {
    /// Wrap an already compiled component. The runtime verifies its engine and
    /// initializes GC metadata from the source when a workload first needs it.
    pub fn new(component: Component) -> Self {
        Self {
            component,
            may_use_gc: Arc::default(),
        }
    }

    /// Inspect the compiled component without losing its associated metadata.
    pub fn component(&self) -> &Component {
        &self.component
    }
}

impl std::fmt::Debug for CacheValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheValue").finish()
    }
}

/// Compiles that running workloads still hold, by digest.
///
/// The compile cache is bounded and forgets idle entries, but a workload
/// keeps its `Component` for as long as it runs. Without this, a digest the
/// cache let go of would be compiled again by the next workload to start
/// from it, holding a second copy of the machine code beside the first.
/// Consulting the live set first keeps one copy per digest while any
/// workload holds it, however small the cache.
type LiveCompiles =
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<LiveCompile>>>;

/// One digest's compile, held by every workload loaded from it. The entry in
/// [`LiveCompiles`] lives exactly as long as some workload holds this.
pub(crate) struct LiveCompile {
    digest: String,
    compiled: CacheValue,
    live: std::sync::Weak<LiveCompiles>,
}

impl Drop for LiveCompile {
    fn drop(&mut self) {
        let Some(live) = self.live.upgrade() else {
            return;
        };
        let mut live = live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A workload that started after this one's last holder let go has
        // recorded a fresh compile under the same digest; leave that one.
        if live
            .get(&self.digest)
            .is_some_and(|entry| entry.strong_count() == 0)
        {
            live.remove(&self.digest);
        }
    }
}

/// Whether initializing a workload may block the thread it runs on: compile,
/// open files, or build a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blocking {
    /// On the blocking pool, or a caller that may block.
    Allowed,
    /// On an async worker, where blocking stalls every task that worker runs.
    Forbidden,
}

impl Blocking {
    /// Refuse `step` when blocking is forbidden. Reaching this means
    /// [`Engine::initialize_workload_inline`] let through a workload whose
    /// initialization blocks: the start fails rather than stall the worker.
    fn ensure_allowed(self, step: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            self == Blocking::Allowed,
            "initializing this workload inline would {step} on an async worker"
        );
        Ok(())
    }
}

/// Whether a start answers to the host's `max_process_memory` admission.
///
/// Only a [`crate::host::Host`] admits workload starts, and counts the ones it
/// refuses. An engine used directly, or compiling a host component plugin, is
/// not a workload start and is never refused as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    /// A start the host admitted. Its compile and its initialization queue on
    /// the blocking pool, so each rechecks when it begins. A start that joins
    /// a compile already in flight shares that compile's check.
    Recheck,
    /// Not a host's workload start.
    Unchecked,
}

impl Admission {
    fn check(self, engine: &Engine) -> anyhow::Result<()> {
        if self == Admission::Recheck
            && let Some(guard) = engine.process_memory()
        {
            guard.admit_start().map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}

/// What [`Engine::initialize_workload_inline`] did with a workload.
// Returned once per start and matched at once: boxing would only allocate.
#[allow(clippy::large_enum_variant)]
pub(crate) enum InlineInit {
    /// Initialized on the calling thread, blocking nothing.
    Initialized(anyhow::Result<UnresolvedWorkload>),
    /// Not initialized: doing so could block. `pinned` holds the compiles
    /// already found, so initializing on the blocking pool reuses them.
    WouldBlock {
        workload: Workload,
        pinned: Vec<Arc<LiveCompile>>,
    },
}

/// An in-flight failure, shared by every caller that raced for the same key.
///
/// The coordinator shares failures as `Arc<anyhow::Error>`, and an
/// `Arc` is not `Into<anyhow::Error>` — so `anyhow!(arc)` takes the *adhoc*
/// branch and builds a fresh error whose whole content is the `Arc`'s `Display`.
/// `anyhow::Error`'s `Display` prints only its outermost context, so every cause
/// under it is dropped: a component that fails to compile reports
/// "failed to compile component from bytes" and never the wasmtime diagnostic
/// saying *why*, which is the only part worth reading.
///
/// Wrapping instead of reformatting keeps the chain walkable — `source()` steps
/// straight into the original error, since `anyhow::Error` derefs to
/// `dyn StdError`.
#[derive(Debug)]
pub(crate) struct SharedError(pub(crate) Arc<anyhow::Error>);

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&*self.0, f)
    }
}

impl std::error::Error for SharedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Explain a pool refusal using its installed heap ceiling and reservation.
fn explain_compile_failure(
    e: wasmtime::Error,
    heap_ceiling: u64,
    memory_reservation: u64,
) -> anyhow::Error {
    use host_memory::{render_bytes, render_bytes_exact};

    let text = format!("{e:#}");
    let Some(required) = parse_pooling_minimum(&text) else {
        return anyhow::Error::from(e);
    };
    let suggested = required
        .checked_next_power_of_two()
        .unwrap_or(required)
        .max(4 * host_memory::MIB);
    let advice = if required > memory_reservation {
        format!(
            "--memory-reservation is {}. Raise it to at least {} \
             (chart: runtime.resources.memoryReservation), and ensure --default-heap-memory \
             permits that size (chart: runtime.resources.defaultHeapMemory).",
            render_bytes(memory_reservation),
            render_bytes_exact(suggested),
        )
    } else {
        format!(
            "Raise --default-heap-memory to at least {} \
             (chart: runtime.resources.defaultHeapMemory).",
            render_bytes_exact(suggested.min(memory_reservation)),
        )
    };
    anyhow::Error::from(e).context(format!(
        "the component needs {} of linear memory but the pooling allocator's \
         per-memory ceiling is {}. {advice}",
        render_bytes(required),
        render_bytes(heap_ceiling),
    ))
}

/// Reads the minimum byte size out of the pooling allocator's refusal.
///
/// Matching on the message is unpleasant, but wasmtime offers no typed form of
/// it and the alternative is discarding the only number that makes the failure
/// actionable.
fn parse_pooling_minimum(text: &str) -> Option<u64> {
    let tail = text.split_once("minimum byte size of")?.1;
    let digits: String = tail
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    // A zero-length run, or a leading `0`, means the size was not printed as
    // plain decimal — wasmtime already renders the limit half of this message
    // in hex. Fall back to the original error rather than reporting 0 bytes.
    match digits.parse().ok()? {
        0 => None,
        required => Some(required),
    }
}

impl Engine {
    /// Register application-defined stream/future payloads before resolving workloads.
    pub fn stream_payloads(&self) -> &Arc<stream_payload::StreamPayloads> {
        &self.stream_payloads
    }

    /// host_memory has the memory budgets this engine was built with including max host
    /// memory available, default component heap limit, and number of core instances available.
    /// With a pool, `max_memories` and `core_instances` are the installed totals, so what a
    /// caller reports is what the engine runs with, however the count was set or derived.
    pub fn host_memory(&self) -> host_memory::HostMemoryBudgets {
        self.host_memory
    }

    /// The host-wide guest memory budget, for reporting what guests are
    /// holding and what the budget has refused.
    pub fn guest_memory(&self) -> &Arc<guest_memory::GuestMemoryBudget> {
        &self.guest_memory
    }

    /// The hostcall fuel each store starts every guest-to-host call with: see
    /// [`EngineBuilder::with_hostcall_fuel`]. `None` is wasmtime's default.
    pub fn hostcall_fuel(&self) -> Option<std::num::NonZeroUsize> {
        self.store_settings.hostcall_fuel
    }

    /// The process's memory footprint against `max_process_memory`, for an
    /// embedder to report, or to stop taking on work while
    /// [`process_memory::ProcessMemoryGuard::is_over`].
    /// `None` when the engine was built
    /// [`without_max_process_memory`](EngineBuilder::without_max_process_memory).
    pub fn process_memory(&self) -> Option<&Arc<process_memory::ProcessMemoryGuard>> {
        self.process_memory.as_ref()
    }

    /// Inject admission state without the process sampler overwriting it.
    #[cfg(all(test, feature = "washlet"))]
    pub(crate) fn with_unsampled_admission_guard(
        mut self,
        guard: Arc<process_memory::ProcessMemoryGuard>,
    ) -> Self {
        self.process_memory = Some(guard);
        self
    }

    /// Core instances the pooling allocator was configured to admit, captured
    /// from the [`PoolingAllocationConfig`] actually installed — so it reflects
    /// the `WASMTIME_POOLING_TOTAL_CORE_INSTANCES` override and a
    /// caller-supplied pooling config alike. `None` when pooling is off or
    /// unsupported.
    ///
    /// This is a count of live core modules, several per component, and it is
    /// sized not to bind before the linear memories do; what the pool holds in
    /// components depends on their resource requirements and all the installed
    /// limits reported by [`Self::pool_usage`], including independent overrides.
    ///
    /// Deprecated because its meaning moved under the same name: until the
    /// pool totals were derived from the memory count this was also the
    /// number of instances the pool held, and a caller still sizing work from
    /// it now reads sixteen times that. [`Self::total_memories`] is the count
    /// that reserves the pool and bounds instances of the usual shape.
    #[deprecated(
        since = "2.12.0",
        note = "a counter sixteen times the memory count, not the pool's instance capacity; \
                use `total_memories` for the count that sizes the pool"
    )]
    pub fn total_core_instances(&self) -> Option<u32> {
        Some(self.pool_totals.as_ref()?.core_instances)
    }

    /// Linear memories the pool can hold at once: the count `--max-memories`
    /// sizes, and the one that reserves address space. A component can contain
    /// several memories, and other pool totals can be overridden independently;
    /// this is not a count of component instances. `None` when pooling is off
    /// or unsupported.
    pub fn total_memories(&self) -> Option<u32> {
        Some(self.pool_totals.as_ref()?.memories)
    }

    /// All installed pooling limits, including environment and caller-supplied
    /// overrides. `None` when pooling is off or unsupported.
    pub(crate) fn pool_totals(&self) -> Option<pool_usage::PoolTotals> {
        self.pool_totals
    }

    /// Every pooling allocator total against what is in use now, so an
    /// embedder can report a host whose pool is full: its workloads still say
    /// running while every instantiation is refused. `None` when pooling is
    /// off. See [`pool_usage::pool_refusals`] for refusals between readings.
    pub fn pool_usage(&self) -> Option<pool_usage::PoolUsage> {
        let totals = self.pool_totals.as_ref()?;
        let metrics = self.inner.pooling_allocator_metrics()?;
        Some(pool_usage::PoolUsage::read(&metrics, totals))
    }

    /// Creates a new [`EngineBuilder`] for configuring an engine.
    ///
    /// # Returns
    /// A default `EngineBuilder` that can be customized with additional configuration.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Gets a reference to the inner wasmtime engine.
    pub fn inner(&self) -> &wasmtime::Engine {
        &self.inner
    }

    /// Whether this engine compiles fuel counters into its guests, and so
    /// whether a store it builds can be metered by fuel at all.
    ///
    /// Read back from the config wasmtime resolved rather than from
    /// [`EngineBuilder::with_fuel_consumption`], so a base config supplied
    /// through [`EngineBuilder::with_config`] is answered for too.
    pub fn consumes_fuel(&self) -> bool {
        self.inner.get_consume_fuel()
    }

    /// Initializes a workload by validating and preparing all its components.
    ///
    /// This function takes a workload definition and prepares it for execution by:
    /// - Validating service components (if present)
    /// - Setting up volumes (both host path and empty directory types)
    /// - Initializing all components with their resource configurations
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this workload instance
    /// * `workload` - The workload configuration containing components, services, and volumes
    ///
    /// # Returns
    /// An `UnresolvedWorkload` that still needs to be bound to plugins and resolved
    /// before execution.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Service component validation fails
    /// - Volume paths don't exist or aren't accessible
    /// - Component initialization fails
    pub async fn initialize_workload(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
    ) -> anyhow::Result<UnresolvedWorkload> {
        self.initialize_workload_admitting(id, workload, Admission::Unchecked)
            .await
    }

    /// [`Self::initialize_workload`] for a start a [`crate::host::Host`] has
    /// admitted against `max_process_memory`, rechecked where the start's
    /// queued blocking work begins.
    pub(crate) async fn initialize_admitted_workload(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
    ) -> anyhow::Result<UnresolvedWorkload> {
        self.initialize_workload_admitting(id, workload, Admission::Recheck)
            .await
    }

    async fn initialize_workload_admitting(
        &self,
        id: impl AsRef<str>,
        mut workload: Workload,
        admission: Admission,
    ) -> anyhow::Result<UnresolvedWorkload> {
        let id = id.as_ref().to_owned();
        // Blocking tasks do not inherit the caller's span: enter it in each, so
        // what they log is tied to the start that caused it.
        let span = tracing::Span::current();
        let volumes = std::mem::take(&mut workload.volumes);
        let has_volumes = !volumes.is_empty();
        let prepared_volumes = if has_volumes {
            let engine = self.clone();
            let span = span.clone();
            tokio::task::spawn_blocking(move || {
                let _entered = span.enter();
                engine.prepare_volumes(volumes, Blocking::Allowed)
            })
            .await
            .context("volume preparation task failed")??
        } else {
            WorkloadVolumes::default()
        };
        let mut pinned = Vec::new();
        if let Some(service) = &mut workload.service {
            pinned.push(
                self.prepare_compile(service.bytes.clone(), &mut service.digest, admission)
                    .await
                    .context("failed to create component from bytes")?,
            );
        }
        for component in &mut workload.components {
            pinned.push(
                self.prepare_compile(component.bytes.clone(), &mut component.digest, admission)
                    .await
                    .context("failed to create component from bytes")?,
            );
        }
        // Every compile is now held. With no volumes or service, and nothing
        // left to read off a component's source, what remains blocks nothing:
        // finish here, as `initialize_workload_inline` does for live compiles.
        // This is the path of a digest the cache kept after its last workload
        // stopped, a scale from zero or a replaced last replica.
        if !has_volumes
            && workload.service.is_none()
            && pinned.iter().all(|live| self.knows_gc_use(&live.compiled))
        {
            // Cache preparation may have waited since the host admitted the start.
            admission.check(self)?;
            // A panic fails this start, as it does on the blocking pool.
            return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.initialize_prepared_workload(
                    id,
                    workload,
                    Blocking::Forbidden,
                    prepared_volumes,
                )
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown panic");
                Err(anyhow::anyhow!(
                    "workload initialization panicked: {message}"
                ))
            });
        }
        let engine = self.clone();
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            let _pinned = pinned;
            // The blocking executor can queue this after the host's own check.
            admission.check(&engine)?;
            engine.initialize_prepared_workload(id, workload, Blocking::Allowed, prepared_volumes)
        })
        .await
        .context("workload initialization task failed")?
    }

    async fn prepare_compile(
        &self,
        bytes: bytes::Bytes,
        digest: &mut Option<String>,
        admission: Admission,
    ) -> anyhow::Result<Arc<LiveCompile>> {
        if digest.is_none() {
            let bytes = bytes.clone();
            *digest = Some(tokio::task::spawn_blocking(move || content_digest(&bytes)).await?);
        }
        let (_, live) = self
            .load_live_bytes(bytes, digest.as_deref(), Blocking::Allowed, admission)
            .await?;
        Ok(live)
    }

    /// The compiled-component backend this engine was built with. The handle
    /// is fixed for the engine's life and shared by its clones.
    pub fn cache(&self) -> Arc<dyn Cache> {
        Arc::clone(&self.cache)
    }

    /// Reserve further paths from guest volume mounts, for storage a host
    /// selects after this engine was built.
    #[cfg(feature = "oci")]
    pub(crate) fn reserve_host_paths(&mut self, paths: Vec<PathBuf>) {
        self.host_paths.reserve(paths);
    }

    /// Initialize `workload` on the calling thread if doing so compiles
    /// nothing, opens no files and builds no service, or hand it back with
    /// whatever compiles it already holds.
    ///
    /// Initialization runs in [`Blocking::Forbidden`], so a step that would
    /// block fails the start rather than stall an async worker:
    /// anything added to initialization that can block must check that mode,
    /// and add the matching check here.
    pub(crate) fn initialize_workload_inline(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
    ) -> InlineInit {
        if !workload.volumes.is_empty() || workload.service.is_some() {
            return InlineInit::WouldBlock {
                workload,
                pinned: Vec::new(),
            };
        }
        let mut pinned = Vec::with_capacity(workload.components.len());
        for component in &workload.components {
            match component
                .digest
                .as_deref()
                .and_then(|digest| self.live_compile(digest))
            {
                Some(live) => pinned.push(live),
                // Stop at the first miss: the blocking pool loads the rest.
                None => return InlineInit::WouldBlock { workload, pinned },
            }
        }
        // `pinned` keeps every compile live, so none can be evicted and
        // recompiled while this initializes.
        let initialized = self.initialize_workload_with(id, workload, Blocking::Forbidden);
        drop(pinned);
        InlineInit::Initialized(initialized)
    }

    fn initialize_workload_with(
        &self,
        id: impl AsRef<str>,
        mut workload: Workload,
        blocking: Blocking,
    ) -> anyhow::Result<UnresolvedWorkload> {
        let volumes = self.prepare_volumes(std::mem::take(&mut workload.volumes), blocking)?;
        self.initialize_prepared_workload(id, workload, blocking, volumes)
    }

    fn initialize_prepared_workload(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
        blocking: Blocking,
        prepared_volumes: WorkloadVolumes,
    ) -> anyhow::Result<UnresolvedWorkload> {
        let Workload {
            namespace,
            name,
            components,
            service,
            volumes: _,
            host_interfaces,
            annotations,
            ..
        } = workload;
        let annotations_arc = Arc::new(annotations);

        // Bind scratch first so it is dropped after all directory handles.
        let WorkloadVolumes {
            scratch: workload_scratch,
            opened: validated_volumes,
        } = prepared_volumes;

        let loopback = Arc::default();
        // One per deploy, shared by every component in it.
        let trap_hints = Arc::default();

        // Iniitalize service
        let service = if let Some(svc) = service {
            blocking.ensure_allowed("initialize a service")?;
            match self.initialize_service(
                id.as_ref(),
                &name,
                &namespace,
                svc,
                &validated_volumes,
                Arc::clone(&loopback),
            ) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized service component");
                    Some(
                        handle
                            .with_annotations(Arc::clone(&annotations_arc))
                            .with_trap_hints(&trap_hints),
                    )
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize service component");
                    bail!(e);
                }
            }
        } else {
            None
        };

        // Initialize all components
        let mut workload_components = Vec::new();
        for component in components.into_iter() {
            match self.initialize_workload_component(
                id.as_ref(),
                &name,
                &namespace,
                component,
                &validated_volumes,
                Arc::clone(&loopback),
                blocking,
            ) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized workload component");
                    workload_components.push(
                        handle
                            .with_annotations(Arc::clone(&annotations_arc))
                            .with_trap_hints(&trap_hints),
                    );
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize component");
                    bail!(e);
                }
            }
        }

        let workload = UnresolvedWorkload::new(
            id.as_ref(),
            name,
            namespace,
            service,
            workload_components,
            host_interfaces,
        )
        .with_scratch(workload_scratch)
        .with_annotations(&annotations_arc)
        .with_stream_payloads(Arc::clone(&self.stream_payloads));

        #[cfg(feature = "wasi-tls")]
        let workload = workload.maybe_with_tls_provider(self.tls_provider.clone());

        Ok(workload)
    }

    fn prepare_volumes(
        &self,
        volumes: Vec<crate::types::Volume>,
        blocking: Blocking,
    ) -> anyhow::Result<WorkloadVolumes> {
        // Created on the first `emptyDir`, and removed when the workload is
        // released or, for a start that fails before then, when this drops.
        // Declared first to drop last: Windows cannot remove a directory while
        // the volume handles below are open.
        let mut workload_scratch: Option<Arc<scratch::ScratchDir>> = None;
        // Process and validate volumes - create a lookup map from volume name to validated host path
        let mut validated_volumes = std::collections::HashMap::new();

        for v in volumes {
            blocking.ensure_allowed("open a volume")?;
            let host_path = match v.volume_type {
                VolumeType::HostPath(HostPathVolume { local_path }) => {
                    let path = PathBuf::from(&local_path);
                    if !path.is_dir() {
                        anyhow::bail!(
                            "HostPath volume '{local_path}' does not exist or is not a directory",
                        );
                    }
                    self.host_paths.open(&path)?
                }
                VolumeType::EmptyDir(EmptyDirVolume {}) => {
                    let workload_scratch = match &mut workload_scratch {
                        Some(dir) => dir,
                        None => {
                            let root = self.scratch.as_ref().map_err(|e| {
                                anyhow::anyhow!("emptyDir volumes are unavailable: {e}")
                            })?;
                            workload_scratch.insert(root.workload_dir()?)
                        }
                    };
                    // Kept on disk: the workload's scratch directory owns it.
                    let volume_dir = tempfile::Builder::new()
                        .prefix("volume-")
                        .tempdir_in(workload_scratch.path())
                        .context("failed to create an emptyDir volume")?
                        .keep();
                    tracing::debug!(path = ?volume_dir, "created emptyDir volume");
                    volumes::ReservedHostPaths::default().open(&volume_dir)?
                }
            };

            // Store the validated volume for later lookup
            validated_volumes.insert(v.name.clone(), host_path);
        }

        Ok(WorkloadVolumes {
            opened: validated_volumes,
            scratch: workload_scratch,
        })
    }

    #[instrument(name = "initialize_service", skip_all)]
    fn initialize_service(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        service: crate::types::Service,
        validated_volumes: &std::collections::HashMap<String, volumes::OpenedVolume>,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
    ) -> anyhow::Result<WorkloadService> {
        // Create a wasmtime component from the bytes
        let (
            CacheValue {
                component: wasmtime_component,
                ..
            },
            compiled,
        ) = self
            .prepared_component(service.digest.as_deref(), Blocking::Allowed)
            .context("failed to create component from bytes")?;

        // Create a linker for this component
        let mut linker: Linker<SharedCtx> = Linker::new(&self.inner);

        // Add WASI interfaces to the linker (with custom socket implementation)
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker if the component uses them
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
                .map_err(anyhow::Error::from)
                .context("failed to add wasi:http/types to linker")?;
            wasmtime_wasi_http::p3::add_to_linker(&mut linker)
                .map_err(|e| anyhow::anyhow!(e).context("failed to add wasi:http p3 to linker"))?;
        }

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        let mut resolved_volume_mounts = Vec::new();
        for vm in &service.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.path.clone(), vm.clone()));
                resolved_volume_mounts
                    .push(volumes::ResolvedVolumeMount::from_opened(host_path, vm)?);
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        let mut service = WorkloadService::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            wasmtime_component,
            linker,
            component_volume_mounts,
            service.local_resources,
            service.max_restarts,
            loopback,
        );
        service.metadata.resolved_volume_mounts = resolved_volume_mounts;
        service.metadata.socket_policy = Arc::clone(&self.socket_policy);
        service.metadata.guest_memory = Arc::clone(&self.guest_memory);
        service.metadata.store_settings = self.store_settings;
        service.metadata.compiled = Some(compiled);

        let world = service.world();

        if !world.exports.iter().any(|iface| {
            iface.namespace == "wasi" && iface.package == "cli" && iface.interfaces.contains("run")
        }) && world.exports.len() != 1
        {
            bail!("Service must export a single interface with the 'run' function");
        }

        // Create the WorkloadService with volume mounts
        Ok(service)
    }

    /// See [`explain_compile_failure`].
    fn explain_compile_failure(&self, e: wasmtime::Error) -> anyhow::Error {
        explain_compile_failure(
            e,
            self.effective_heap_memory(),
            self.inner.get_memory_reservation(),
        )
    }

    /// The linear-memory ceiling a compile failure was actually measured
    /// against: the installed pool's, falling back to the flag when there is
    /// no pool to have overridden it.
    fn effective_heap_memory(&self) -> u64 {
        self.installed_heap_memory
            .unwrap_or(self.host_memory.default_heap_memory)
    }

    /// Whether a store of the component in `bytes` may need a GC heap, read
    /// off its source ([`pool_gc::may_use_gc`]). Only pool admission asks,
    /// so an engine with no pool skips the walk over every function body and
    /// answers `false`, which nothing reads.
    fn may_use_gc(&self, bytes: &[u8]) -> bool {
        self.pool_totals.is_some() && pool_gc::may_use_gc(bytes)
    }

    fn compile(&self, bytes: &[u8]) -> anyhow::Result<CacheValue> {
        let component = Component::new(&self.inner, bytes)
            .map_err(|e| self.explain_compile_failure(e))
            .context("failed to compile component from bytes")?;
        Ok(CacheValue::new(component))
    }

    /// [`Self::may_use_gc`] for `compiled`, read off `bytes`, its source,
    /// only the first time any workload of that compile asks.
    fn compiled_may_use_gc(&self, compiled: &CacheValue, bytes: &[u8]) -> bool {
        *compiled.may_use_gc.get_or_init(|| self.may_use_gc(bytes))
    }

    /// Whether [`Self::compiled_may_use_gc`] answers for `compiled` without
    /// reading its source.
    fn knows_gc_use(&self, compiled: &CacheValue) -> bool {
        self.pool_totals.is_none() || compiled.may_use_gc.get().is_some()
    }

    /// Load or compile asynchronously, retaining live ownership independently
    /// of cache eviction. The flight covers the cache lookup, which a caller
    /// that stops waiting abandons, and waits on [`Self::compile_shared`].
    #[instrument(name = "load_component_bytes", skip_all, fields(digest = tracing::field::Empty))]
    async fn load_live_bytes(
        &self,
        bytes: bytes::Bytes,
        digest: Option<impl AsRef<str>>,
        blocking: Blocking,
        admission: Admission,
    ) -> anyhow::Result<(CacheValue, Arc<LiveCompile>)> {
        let digest = match digest {
            Some(digest) => digest.as_ref().to_owned(),
            None => {
                blocking.ensure_allowed("hash a component")?;
                let bytes = bytes.clone();
                tokio::task::spawn_blocking(move || content_digest(&bytes)).await?
            }
        };
        tracing::Span::current().record("digest", digest.as_str());
        if let Some(live) = self.live_compile(&digest) {
            return Ok((live.compiled.clone(), live));
        }
        blocking.ensure_allowed("compile a component")?;
        let live = self
            .compile_flights
            .run(digest.clone(), || async {
                if let Some(live) = self.live_compile(&digest) {
                    return Ok(live);
                }
                let key = CacheKey {
                    engine: self.cache_engine_id,
                    digest: digest.clone(),
                };
                if let Some(compiled) = self.cached_compile(&key).await? {
                    return Ok(self.record_live(&digest, compiled));
                }
                self.compile_shared(bytes, key, admission).await
            })
            .await?;
        Ok((live.compiled.clone(), live))
    }

    /// Treat unavailable retention as a miss, but reject another engine's code.
    async fn cached_compile(&self, key: &CacheKey) -> anyhow::Result<Option<CacheValue>> {
        // The cache only saves a compile: a backend that cannot answer
        // costs one, and never the start.
        let cached = self.cache.get_compiled(key).await.unwrap_or_else(|error| {
            warn!(
                cache = self.cache.name(),
                error = format!("{error:#}"),
                "compilation cache lookup failed; compiling instead"
            );
            None
        });
        if let Some(compiled) = &cached {
            // Not an outage: instantiating another engine's code is
            // unsound, and a backend that returns it is broken.
            anyhow::ensure!(
                wasmtime::Engine::same(&self.inner, compiled.component.engine()),
                "cache returned a component belonging to a different engine"
            );
        }
        Ok(cached)
    }

    /// Recheck retention, compile if needed, and publish on a task of its own.
    ///
    /// Blocking work cannot be aborted, so a compile always finishes. Running
    /// it, and its publication, apart from the caller means a start that is
    /// cancelled still leaves its compile in the cache for the retry, and a
    /// caller arriving meanwhile joins the compile instead of starting another.
    async fn compile_shared(
        &self,
        bytes: bytes::Bytes,
        key: CacheKey,
        admission: Admission,
    ) -> anyhow::Result<Arc<LiveCompile>> {
        use tracing::Instrument as _;

        let engine = self.clone();
        let work = async move {
            engine
                .compiling
                .run(key.digest.clone(), || async {
                    if let Some(live) = engine.live_compile(&key.digest) {
                        return Ok(live);
                    }
                    // A cancelled start may have published after this caller's
                    // earlier miss, then left the flight and released its live compile.
                    if let Some(compiled) = engine.cached_compile(&key).await? {
                        return Ok(engine.record_live(&key.digest, compiled));
                    }
                    let compiled = engine.compile_owned(bytes, admission).await?;
                    // The compile is in hand: failing to retain it costs the
                    // next start a compile, not this one its component.
                    if let Err(error) = engine.cache.put_compiled(&key, compiled.clone()).await {
                        warn!(
                            cache = engine.cache.name(),
                            error = format!("{error:#}"),
                            "compilation cache publication failed; continuing uncached"
                        );
                    }
                    Ok(engine.record_live(&key.digest, compiled))
                })
                .await
        };
        tokio::spawn(work.in_current_span())
            .await
            .context("component preparation task failed")?
    }

    async fn compile_owned(
        &self,
        bytes: bytes::Bytes,
        admission: Admission,
    ) -> anyhow::Result<CacheValue> {
        let engine = self.clone();
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            admission.check(&engine)?;
            #[cfg(test)]
            engine
                .compiles
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            engine.compile(&bytes)
        })
        .await
        .context("component compilation task failed")?
    }

    #[cfg(test)]
    async fn load_live_component(
        &self,
        bytes: impl AsRef<[u8]>,
        digest: Option<impl AsRef<str>>,
        blocking: Blocking,
    ) -> anyhow::Result<(CacheValue, Arc<LiveCompile>)> {
        self.load_live_bytes(
            bytes::Bytes::copy_from_slice(bytes.as_ref()),
            digest,
            blocking,
            Admission::Unchecked,
        )
        .await
    }

    fn prepared_component(
        &self,
        digest: Option<&str>,
        blocking: Blocking,
    ) -> anyhow::Result<(CacheValue, Arc<LiveCompile>)> {
        let Some(digest) = digest else {
            blocking.ensure_allowed("hash a component")?;
            bail!("component was not prepared before initialization");
        };
        let Some(live) = self.live_compile(digest) else {
            blocking.ensure_allowed("compile a component")?;
            bail!("component was not prepared before initialization");
        };
        Ok((live.compiled.clone(), live))
    }

    /// Record `compiled` as `digest`'s live compile, or share the one another
    /// start recorded since the caller last looked rather than keep a second.
    ///
    /// Called once the compile or cache read is done, outside this lock:
    /// a compile is slow, and a cache read can run cache housekeeping.
    fn record_live(&self, digest: &str, compiled: CacheValue) -> Arc<LiveCompile> {
        let mut live = self
            .live_compiles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = live.get(digest).and_then(std::sync::Weak::upgrade) {
            return existing;
        }
        let entry = Arc::new(LiveCompile {
            digest: digest.to_string(),
            compiled,
            live: Arc::downgrade(&self.live_compiles),
        });
        live.insert(digest.to_string(), Arc::downgrade(&entry));
        entry
    }

    fn live_compile(&self, digest: &str) -> Option<Arc<LiveCompile>> {
        self.live_compiles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(digest)
            .and_then(std::sync::Weak::upgrade)
    }

    #[cfg(test)]
    async fn load_component_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
        digest: Option<impl AsRef<str>>,
    ) -> anyhow::Result<CacheValue> {
        match digest {
            Some(digest) => Ok(self
                .load_live_component(bytes, Some(digest), Blocking::Allowed)
                .await?
                .0),
            None => {
                self.compile_owned(
                    bytes::Bytes::copy_from_slice(bytes.as_ref()),
                    Admission::Unchecked,
                )
                .await
            }
        }
    }

    /// Initialize a component that is a part of a workload, add wasi@0.2 interfaces (and
    /// wasi:http if the `http` feature is enabled) to the linker.
    #[allow(clippy::too_many_arguments)]
    #[instrument(name = "initialize_workload_component", skip_all, fields(component.name = %component.name))]
    fn initialize_workload_component(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        component: crate::types::Component,
        validated_volumes: &std::collections::HashMap<String, volumes::OpenedVolume>,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
        blocking: Blocking,
    ) -> anyhow::Result<WorkloadComponent> {
        // Read before the component's fields are moved out below.
        let instances = InstancePolicy::from_component(&component);

        // Create a wasmtime component from the bytes
        let (cached, compiled) = self
            .prepared_component(component.digest.as_deref(), blocking)
            .context("failed to create component from bytes")?;
        let may_use_gc = self.compiled_may_use_gc(&cached, &component.bytes);
        let wasmtime_component = cached.component;

        let linker = self.base_linker(uses_wasi_http(&wasmtime_component))?;

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        let mut resolved_volume_mounts = Vec::new();
        for vm in &component.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.path.clone(), vm.clone()));
                resolved_volume_mounts
                    .push(volumes::ResolvedVolumeMount::from_opened(host_path, vm)?);
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        // Create the WorkloadComponent with volume mounts
        let mut workload_component = WorkloadComponent::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            component.name,
            wasmtime_component,
            linker,
            component_volume_mounts,
            component.local_resources,
            loopback,
            instances,
        );
        workload_component.metadata.resolved_volume_mounts = resolved_volume_mounts;
        workload_component.metadata.socket_policy = Arc::clone(&self.socket_policy);
        workload_component.metadata.guest_memory = Arc::clone(&self.guest_memory);
        workload_component.metadata.store_settings = self.store_settings;
        workload_component.metadata.compiled = Some(compiled);
        // From the source alone. A component wasmtime cannot cost
        // (`resources_required` is `None`) imports a core module, which no
        // linker here provides, so it never instantiates to need a heap.
        workload_component.metadata.may_use_gc = may_use_gc;
        Ok(workload_component)
    }

    /// The shared linker a workload component starts from: WASI (with our
    /// socket implementation), plus `wasi:http` when the component uses it.
    /// See [`BaseLinkers`].
    fn base_linker(&self, with_http: bool) -> anyhow::Result<Arc<Linker<SharedCtx>>> {
        let slot = if with_http {
            &self.base_linkers.wasi_http
        } else {
            &self.base_linkers.wasi
        };
        if let Some(linker) = slot.get() {
            return Ok(Arc::clone(linker));
        }
        let mut linker: Linker<SharedCtx> = Linker::new(&self.inner);
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;
        if with_http {
            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
                .map_err(anyhow::Error::from)
                .context("failed to add wasi:http/types to linker")?;
            wasmtime_wasi_http::p3::add_to_linker(&mut linker)
                .map_err(|e| anyhow::anyhow!(e).context("failed to add wasi:http p3 to linker"))?;
        }
        // Concurrent first starts may each build one; all but one are dropped.
        Ok(Arc::clone(slot.get_or_init(|| Arc::new(linker))))
    }

    /// Compile a host component plugin and build a linker with WASI (and
    /// `wasi:http` if the component uses it) added.
    ///
    /// Unlike a workload component, a host component plugin is host-scoped: it is
    /// instantiated once at host start into its own long-lived store. The caller
    /// may install additional shims on the returned [`Linker`] (e.g. to route a
    /// plugin's own capability imports back to itself) before calling
    /// [`Linker::instantiate_pre`]; the [`Component`] is returned alongside for
    /// exported- and imported-type introspection.
    pub async fn prepare_host_component(
        &self,
        bytes: &[u8],
    ) -> anyhow::Result<(Component, Linker<SharedCtx>)> {
        // Not a workload: compiled off the caller's thread, but neither cached,
        // since a plugin is loaded once and holds its component for good, nor
        // refused as a workload start.
        let component = self
            .compile_owned(bytes::Bytes::copy_from_slice(bytes), Admission::Unchecked)
            .await
            .context("failed to compile host component plugin")?
            .component;

        let mut linker: Linker<SharedCtx> = Linker::new(&self.inner);
        add_wasi_to_linker(&mut linker).context("failed to add WASI to plugin linker")?;
        if uses_wasi_http(&component) {
            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
                .map_err(anyhow::Error::from)
                .context("failed to add wasi:http/types to plugin linker")?;
            wasmtime_wasi_http::p3::add_to_linker(&mut linker).map_err(|e| {
                anyhow::anyhow!(e).context("failed to add wasi:http p3 to plugin linker")
            })?;
        }

        Ok((component, linker))
    }
}

/// A wasmtime WebAssembly proposal that can be opted into on the engine.
///
/// Each variant maps to one or more `wasmtime::Config` feature flags. Use it
/// with [`EngineBuilder::with_wasm_proposal`] to enable a bundle of related
/// settings without having to replace the entire config via
/// [`EngineBuilder::with_config`].
///
/// ```no_run
/// # use wash_runtime::engine::{Engine, WasmProposal};
/// # fn example() -> anyhow::Result<()> {
/// let engine = Engine::builder()
///     .with_wasm_proposal(WasmProposal::Gc)
///     .with_wasm_proposal(WasmProposal::Threads)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WasmProposal {
    /// Component model async ABI (`stream`/`future`/`error-context` types).
    /// Enables `wasm_component_model_async`. Required for WASIP3.
    ComponentModelAsync,
    /// Component model `(implements ..)` named imports, letting a component
    /// import the same interface multiple times under distinct names so host
    /// plugins can route each independently. Enables
    /// `wasm_component_model_implements`. Available with the
    /// `wasm_component_model_implements` crate feature, which is on by default
    /// and carries the plugin-side routing this proposal is useful for.
    #[cfg(feature = "wasm_component_model_implements")]
    WasmComponentModelImplements,
    /// Component model `map<k, v>` type. Enables `wasm_component_model_map`,
    /// which is what lets a component whose WIT uses `map` types be validated
    /// and instantiated. Enabled by default, like [`Self::ComponentModelAsync`].
    ComponentModelMap,
    /// Garbage collection (with its `wasm_function_references` prerequisite).
    /// Enabled by default in wasmtime >= 47; accepted for compatibility.
    Gc,
    /// Exception handling. Enabled by default in wasmtime >= 47; accepted for
    /// compatibility.
    ExceptionHandling,
    /// 128-bit wide arithmetic. Enabled by default in wasmtime >= 47; accepted
    /// for compatibility.
    WideArithmetic,
    /// Shared-memory threads. Enables `wasm_threads`.
    Threads,
    /// Tail calls. Enables `wasm_tail_call`.
    TailCall,
}

impl WasmProposal {
    /// Apply this proposal's wasmtime feature flags onto `cfg`.
    fn apply(self, cfg: &mut wasmtime::Config) {
        match self {
            WasmProposal::ComponentModelAsync => {
                cfg.wasm_component_model_async(true);
            }
            #[cfg(feature = "wasm_component_model_implements")]
            WasmProposal::WasmComponentModelImplements => {
                cfg.wasm_component_model_implements(true);
            }
            WasmProposal::ComponentModelMap => {
                cfg.wasm_component_model_map(true);
            }
            // GC (with its `wasm_function_references` prerequisite), exception
            // handling, and wide arithmetic are all enabled by default in
            // wasmtime >= 47, so opting into them needs no config change. The
            // variants stay accepted for CLI/config compatibility.
            WasmProposal::Gc | WasmProposal::ExceptionHandling | WasmProposal::WideArithmetic => {}
            // Threads and tail calls are also on by default in wasmtime >= 47,
            // but only conditionally — `wasm_threads` follows the `threads`
            // crate feature and `wasm_tail_call` is off under the Winch compiler
            // — so these set their flag explicitly to stay correct regardless of
            // build or config.
            WasmProposal::Threads => {
                cfg.wasm_threads(true);
            }
            WasmProposal::TailCall => {
                cfg.wasm_tail_call(true);
            }
        }
    }
}

/// Error returned when a string cannot be parsed into a [`WasmProposal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseWasmProposalError(String);

impl std::fmt::Display for ParseWasmProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown wasm proposal {:?}; expected one of: component-model-async, \
             component-model-map, gc, exception-handling, wide-arithmetic, threads, \
             tail-call",
            self.0
        )
    }
}

impl std::error::Error for ParseWasmProposalError {}

impl FromStr for WasmProposal {
    type Err = ParseWasmProposalError;

    /// Parse a proposal from its kebab-case name. Matching is case-insensitive
    /// and treats `_` and `-` interchangeably, so `gc`, `GC`,
    /// `exception_handling`, and `exception-handling` all parse.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "component-model-async" => Ok(Self::ComponentModelAsync),
            "component-model-map" => Ok(Self::ComponentModelMap),
            "gc" => Ok(Self::Gc),
            "exception-handling" => Ok(Self::ExceptionHandling),
            "wide-arithmetic" => Ok(Self::WideArithmetic),
            "threads" => Ok(Self::Threads),
            "tail-call" => Ok(Self::TailCall),
            _ => Err(ParseWasmProposalError(s.to_string())),
        }
    }
}

/// Builder for constructing an [`Engine`] with custom configuration.
///
/// The builder pattern allows for flexible configuration of the engine
/// before creation. By default, it enables async support which is required
/// for component execution.
///
/// Settings configured through the builder ([`with_pooling_allocator`],
/// [`with_fuel_consumption`], [`with_wasm_proposal`], …) are layered on top of
/// any base config supplied via [`with_config`], so a custom config can be
/// combined with pooling and proposal flags rather than replacing them.
///
/// [`with_pooling_allocator`]: EngineBuilder::with_pooling_allocator
/// [`with_fuel_consumption`]: EngineBuilder::with_fuel_consumption
/// [`with_wasm_proposal`]: EngineBuilder::with_wasm_proposal
/// [`with_config`]: EngineBuilder::with_config
#[derive(Default)]
pub struct EngineBuilder {
    config: Option<wasmtime::Config>,
    use_pooling_allocator: Option<bool>,
    max_memories: Option<MaxMemories>,
    max_core_instances: Option<u32>,
    pooling_config: Option<PoolingAllocationConfig>,
    proposals: std::collections::BTreeSet<WasmProposal>,
    cache: Option<Arc<dyn Cache>>,
    compilation_cache_size: Option<u64>,
    compilation_cache_ttl: Option<Duration>,
    fuel_consumption: Option<bool>,
    parallel_compilation: Option<bool>,
    native_unwind_info: Option<bool>,
    memory_init_cow: Option<bool>,
    memory_layout: Option<host_memory::MemoryLayout>,
    socket_policy: Option<Arc<crate::sockets::policy::SocketPolicy>>,
    reserved_host_paths: Vec<PathBuf>,
    allowed_host_paths: Vec<PathBuf>,
    scratch_root: Option<PathBuf>,
    host_memory: Option<host_memory::HostMemoryBudgets>,
    max_process_memory_bytes: Option<u64>,
    max_process_memory_off: bool,
    strict_process_memory: bool,
    guest_memory_mode: guest_memory::GuestMemoryMode,
    max_initial_guest_memory_bytes: Option<u64>,
    hostcall_fuel: Option<std::num::NonZeroUsize>,
    /// Optional TLS provider override for wasi:tls client connections.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

/// The memory count [`EngineBuilder`] was given, and how.
#[derive(Debug, Clone, Copy)]
enum MaxMemories {
    /// [`EngineBuilder::with_max_memories`].
    Named(u32),
    /// [`EngineBuilder::with_derived_max_memories`], derived when the engine is built.
    Derived(host_memory::MaxMemoriesDerivation),
}

impl MaxMemories {
    /// The count, deriving it with the engine's copy-on-write setting and
    /// memory layout unless the derivation names its own.
    fn count(
        self,
        memory_init_cow: Option<bool>,
        memory_layout: Option<host_memory::MemoryLayout>,
    ) -> u32 {
        match self {
            Self::Named(count) => count,
            Self::Derived(derivation) => {
                host_memory::derive_max_memories_with(host_memory::MaxMemoriesDerivation {
                    memory_init_cow: derivation.memory_init_cow.or(memory_init_cow),
                    memory_layout: derivation.memory_layout.or(memory_layout),
                    ..derivation
                })
            }
        }
    }
}

impl EngineBuilder {
    /// Reserve paths that no workload volume may equal, contain, or lie
    /// inside. The scratch root (see [`Self::with_scratch_root`]) is always
    /// reserved.
    #[must_use]
    pub fn with_reserved_host_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.reserved_host_paths.extend(paths);
        self
    }

    /// Permit `hostPath` volumes within these directories. Adds to any
    /// permitted before; with none, no `hostPath` volume is permitted.
    ///
    /// This is a host-wide guardrail for a trusted embedder, and for migrating
    /// hosts whose workloads already mount host directories. It is not a
    /// multi-tenant storage policy: any workload on the engine may mount any
    /// directory permitted here. Where tenants author workloads, disable their
    /// raw `hostPath` volumes in the control plane, and have it give each
    /// workload a directory of its own instead.
    #[must_use]
    pub fn with_allowed_host_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.allowed_host_paths.extend(paths);
        self
    }

    /// Where `emptyDir` volumes live. Unset, `wasmcloud-scratch` under the
    /// system temporary directory.
    ///
    /// The engine claims a directory of its own here, and removes what hosts
    /// that are no longer running left behind. The root is reserved: no
    /// `hostPath` volume may equal it, contain it, or lie inside it, whatever
    /// [`Self::with_allowed_host_paths`] permits.
    #[must_use]
    pub fn with_scratch_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.scratch_root = Some(root.into());
        self
    }

    /// Install the host-level socket policy every workload on this engine
    /// inherits.
    ///
    /// The workload-level half (`allowedHosts`, `allowedHostLoopbackPorts`) comes
    /// from each component's `LocalResources` and is layered over this, so a
    /// workload can only ever narrow what the host permits.
    #[must_use]
    pub fn with_socket_policy(mut self, policy: Arc<crate::sockets::policy::SocketPolicy>) -> Self {
        self.socket_policy = Some(policy);
        self
    }

    /// Cap the whole process's memory footprint, guests and host together.
    ///
    /// Past it, guest memory growth and new instances are refused until the
    /// footprint falls back (see [`process_memory`]). Unset, it is derived:
    /// 90% of the cgroup limit, or of physical memory where there is none, so
    /// a host never sets out to use more memory than the machine has.
    ///
    /// Overrides an earlier [`Self::without_max_process_memory`].
    #[must_use]
    pub fn with_max_process_memory_bytes(mut self, bytes: u64) -> Self {
        self.max_process_memory_bytes = Some(bytes);
        self.max_process_memory_off = false;
        self
    }

    /// Require a detectable capacity and a process threshold below it.
    /// This validates configuration; it does not make sampling a hard cap.
    #[must_use]
    pub fn with_strict_process_memory(mut self, strict: bool) -> Self {
        self.strict_process_memory = strict;
        self
    }

    /// Hold the process to no footprint at all: no sampler runs, and only the
    /// guest budget and the kernel limit memory. For an embedder that bounds
    /// the process some other way, or a host whose footprint reading cannot
    /// be trusted.
    ///
    /// Overrides an earlier [`Self::with_max_process_memory_bytes`].
    #[must_use]
    pub fn without_max_process_memory(mut self) -> Self {
        self.max_process_memory_bytes = None;
        self.max_process_memory_off = true;
        self
    }

    /// Set the host's memory budget, per-memory ceiling and instance count.
    /// Unset, the pool knobs are wasmtime's defaults and the budget is derived
    /// from this process's memory limit — and enforced, unless
    /// [`Self::with_guest_memory_mode`] says otherwise.
    pub fn with_host_memory(mut self, host_memory: host_memory::HostMemoryBudgets) -> Self {
        self.host_memory = Some(host_memory);
        self
    }

    /// Whether `max_guest_memory` is enforced or only accounted.
    ///
    /// Unset, it is [`guest_memory::GuestMemoryMode::Enforce`]: growth past
    /// the budget is refused. `max_guest_memory` is derived when an operator
    /// sets nothing, so every engine has a ceiling unless it opts into
    /// [`guest_memory::GuestMemoryMode::Count`].
    ///
    /// [`guest_memory::GuestMemoryMode::Growth`] keeps the ceiling for growth,
    /// but leaves part of each memory's first allocation uncharged,
    /// for hosts packing many small instances,
    /// up to its own cap (see [`Self::with_max_initial_guest_memory_bytes`]).
    #[must_use]
    pub fn with_guest_memory_mode(mut self, mode: guest_memory::GuestMemoryMode) -> Self {
        self.guest_memory_mode = mode;
        self
    }

    /// Under [`guest_memory::GuestMemoryMode::Growth`],
    /// the most initial memory the budget may leave uncharged across the host;
    /// past it, a new memory is refused.
    ///
    /// - Unset, it is [`guest_memory::DEFAULT_INITIAL_OVERCOMMIT`] times `max_guest_memory`.
    /// - Other modes charge initial memory, ignore it, and warn that it was set.
    #[must_use]
    pub fn with_max_initial_guest_memory_bytes(mut self, bytes: u64) -> Self {
        self.max_initial_guest_memory_bytes = Some(bytes);
        self
    }

    /// Roughly the most bytes a guest may copy to the host in one
    /// guest-to-host call, or in one read the host makes from a future or
    /// stream: wasmtime's [`wasmtime::Store::set_hostcall_fuel`], refilled for
    /// each. Past it the call traps. Byte streams the host reads directly,
    /// such as HTTP bodies and socket writes, are not counted. Unset, it is
    /// wasmtime's default of 128 MiB.
    ///
    /// The host allocates these bytes outside the guest memory budget, so a
    /// value past what the process holds back from guests draws a startup
    /// warning when that budget is enforced.
    #[must_use]
    pub fn with_hostcall_fuel(mut self, fuel: std::num::NonZeroUsize) -> Self {
        self.hostcall_fuel = Some(fuel);
        self
    }

    /// Creates a new `EngineBuilder` with default configuration.
    ///
    /// # Returns
    /// A new builder instance with default wasmtime configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables the pooling allocator for instance allocation.
    pub fn with_pooling_allocator(mut self, enable: bool) -> Self {
        self.use_pooling_allocator = Some(enable);
        self
    }

    /// Linear memories the pooling allocator keeps room for, and with them
    /// every total derived from that count ([`derived_pool_totals`]); the
    /// core-instance total follows at [`CORE_INSTANCES_PER_MEMORY`] per
    /// memory unless [`with_max_core_instances`](Self::with_max_core_instances)
    /// sets it. A hint the `WASMTIME_POOLING_TOTAL_*` environment can
    /// override. Wins over the count in
    /// [`with_host_memory`](Self::with_host_memory), and is ignored when a
    /// full [`PoolingAllocationConfig`] is supplied via
    /// [`with_pooling_config`](Self::with_pooling_config).
    pub fn with_max_memories(mut self, memories: u32) -> Self {
        self.max_memories = Some(MaxMemories::Named(memories));
        self
    }

    /// [`with_max_memories`](Self::with_max_memories) at the count
    /// [`host_memory::derive_max_memories_with`] sizes to this machine when the engine is built,
    /// for the engine's own copy-on-write setting unless `derivation` names one.
    /// A count derived against the commit limit lets the stack and table
    /// advisories allow that limit's share, unless a full
    /// [`PoolingAllocationConfig`] replaces it. The later of the two calls
    /// wins.
    ///
    /// For a host process that runs one engine: each engine in a process
    /// reserves address space for its whole pool. Stacks are costed at
    /// wasmtime's default size, which a base [`Self::with_config`] can raise.
    /// If allocating the derived pool fails, construction retries once at
    /// the default 1000 memories, preserving explicit overrides and memory
    /// protection. A named memory count or full pooling config disables this
    /// fallback.
    pub fn with_derived_max_memories(
        mut self,
        derivation: host_memory::MaxMemoriesDerivation,
    ) -> Self {
        self.max_memories = Some(MaxMemories::Derived(derivation));
        self
    }

    /// The old name for [`with_max_memories`](Self::with_max_memories), from
    /// when one count sized every pool total.
    #[deprecated(since = "2.12.0", note = "use with_max_memories")]
    pub fn with_max_instances(self, max: u32) -> Self {
        self.with_max_memories(max)
    }

    /// Core instances the pooling allocator admits at once, in place of the
    /// [`CORE_INSTANCES_PER_MEMORY`] per memory it would derive. A counter,
    /// not a reservation: a core instance costs the pool nothing but a count,
    /// so this bounds how many core modules may be live without moving the
    /// memories, stacks or component instances. The environment's
    /// `WASMTIME_POOLING_TOTAL_CORE_INSTANCES` still overrides it, as it does
    /// every total. Wins over the total in
    /// [`with_host_memory`](Self::with_host_memory), and is ignored when a
    /// full [`PoolingAllocationConfig`] is supplied.
    pub fn with_max_core_instances(mut self, core_instances: u32) -> Self {
        self.max_core_instances = Some(core_instances);
        self
    }

    /// Sets a fully-customized [`PoolingAllocationConfig`] for the pooling
    /// allocator, giving callers control over every pooling knob rather than
    /// only the `WASMTIME_POOLING_*` environment variables.
    ///
    /// Supplying a pooling config implies the pooling allocator is enabled and
    /// takes precedence over [`with_max_memories`](Self::with_max_memories).
    /// It is still only applied when the host supports the pooling allocator.
    pub fn with_pooling_config(mut self, config: PoolingAllocationConfig) -> Self {
        self.pooling_config = Some(config);
        self.use_pooling_allocator = Some(true);
        self
    }

    /// Whether the pool keeps a count derived against the commit limit: a
    /// caller's own pooling config replaces it.
    fn sized_to_commit_limit(&self) -> bool {
        let derived = match self.max_memories {
            Some(MaxMemories::Derived(derivation)) => derivation.from_memory_precommit_limit,
            Some(MaxMemories::Named(_)) | None => false,
        };
        derived && self.pooling_config.is_none()
    }

    /// Enables an additional WebAssembly proposal on the engine.
    ///
    /// Each [`WasmProposal`] maps to one or more `wasmtime::Config` feature
    /// flags (see the variant docs for the exact flags). Proposals are layered
    /// on top of any base config from [`with_config`](Self::with_config), so a
    /// single extra proposal can be enabled without replacing the whole config.
    /// Calling this repeatedly accumulates proposals; duplicates are ignored.
    pub fn with_wasm_proposal(mut self, proposal: WasmProposal) -> Self {
        self.proposals.insert(proposal);
        self
    }

    /// Enables or disables fuel consumption for the engine.
    ///
    /// When unset, fuel consumption is left at whatever the base config (from
    /// [`with_config`](Self::with_config), or the wasmtime default of disabled)
    /// specifies, rather than being forced off.
    pub fn with_fuel_consumption(mut self, enable: bool) -> Self {
        self.fuel_consumption = Some(enable);
        self
    }

    /// Whether Cranelift compiles a component's functions on several threads.
    ///
    /// Enabled unless this or `WASMTIME_PARALLEL_COMPILATION` turns it off.
    /// The threads come from rayon's process-wide pool, which sizes itself to
    /// every core the process can see — not to `max_concurrent_starts`, and
    /// not to the CPU a container is requested at. Two ways to hold a host
    /// down: `RAYON_NUM_THREADS` caps how wide one compile goes, and this
    /// makes it single-threaded again.
    pub fn with_parallel_compilation(mut self, enable: bool) -> Self {
        self.parallel_compilation = Some(enable);
        self
    }

    /// Whether compiled code registers unwind tables with the system unwinder,
    /// which only native debuggers and profilers that unwind by DWARF read.
    ///
    /// On, as in wasmtime, unless this or `WASMTIME_NATIVE_UNWIND_INFO` turns
    /// it off. Under LLVM's libunwind (macOS; musl and zig-built Linux
    /// binaries) every component drop scans a process-wide table of them, so
    /// a host holding many components wants it off. Windows requires them.
    pub fn with_native_unwind_info(mut self, enable: bool) -> Self {
        self.native_unwind_info = Some(enable);
        self
    }

    /// The address space wasmtime lays out for each linear memory:
    /// its reservation and guard (see [`host_memory::MemoryLayout`]).
    ///
    /// Unset, it is wasmtime's default: a 4 GiB reservation and a 32 MiB guard
    /// on 64-bit hosts. A smaller layout fits more pooled memories in the
    /// address space, which is what caps warm instances on a large host
    /// (about 16,000 at the default). Below the default it:
    ///
    /// - **moves a security boundary.** The default reservation covers a
    ///   32-bit dynamic index; the guard covers many constant offsets. Larger
    ///   offsets still need bounds checks at the defaults. A smaller layout
    ///   reduces hardware protection and relies more on software bounds checks,
    ///   whose bugs could expose a sibling's memory;
    /// - can slow guest memory access by requiring more explicit bounds checks;
    /// - with pooling, no memory may grow past the reservation, so the default
    ///   pool's `default_heap_memory` is lowered to it.
    ///
    /// Without pooling, this is an initial reservation, not a heap ceiling:
    /// wasmtime may relocate a memory to grow beyond it. The guest and process
    /// memory budgets still apply.
    ///
    /// The engine warns at startup when the layout is below wasmtime's default.
    /// A count from [`Self::with_derived_max_memories`] follows this layout.
    #[must_use]
    pub fn with_memory_layout(mut self, layout: host_memory::MemoryLayout) -> Self {
        self.memory_layout = Some(layout);
        self
    }

    /// Whether a linear memory starts as a copy-on-write mapping of its
    /// component's data segments, or has them copied in at instantiation.
    ///
    /// On, as in wasmtime, unless this or `WASMTIME_MEMORY_INIT_COW` (`true`
    /// or `false`) turns it off; a base config given to [`Self::with_config`]
    /// keeps its own setting when neither says anything.
    ///
    /// Off:
    ///
    /// - Each instance copies its data segments into its own pages,
    ///   which costs time and memory in proportion to them.
    /// - A live memory splits into two mappings instead of four,
    ///   so a host that cannot raise `vm.max_map_count` holds about half again as many warm instances.
    ///   Raising the limit is the better lever when it is available.
    /// - The map-count advisory, and a count from [`Self::with_derived_max_memories`],
    ///   follow this setting.
    /// - Under [`guest_memory::GuestMemoryMode::Growth`], the copied pages are resident from instantiation,
    ///   but no more charged than the rest of a first allocation.
    #[must_use]
    pub fn with_memory_init_cow(mut self, enable: bool) -> Self {
        self.memory_init_cow = Some(enable);
        self
    }

    /// Sets a custom wasmtime configuration to use as the *base* for the engine.
    ///
    /// This config is used as the starting point, and any other builder
    /// settings — pooling allocator, fuel consumption, and
    /// [`WasmProposal`]s — are layered on top of it. This means a custom config
    /// can be combined with [`with_pooling_allocator`](Self::with_pooling_allocator),
    /// [`with_max_memories`](Self::with_max_memories), and
    /// [`with_wasm_proposal`](Self::with_wasm_proposal) rather than being
    /// mutually exclusive with them.
    ///
    /// Unlike the default-config path, the pooling allocator is *not* enabled
    /// by default on top of a custom config. A base config's allocation
    /// strategy is preserved unless pooling is explicitly requested via
    /// [`with_pooling_allocator`](Self::with_pooling_allocator),
    /// [`with_pooling_config`](Self::with_pooling_config), or the
    /// `WASMTIME_POOLING` environment variable.
    ///
    /// # Arguments
    /// * `config` - A wasmtime `Config` object with custom settings
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_config(mut self, config: wasmtime::Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Select the compiled-component backend. Takes precedence over the
    /// default backend's settings from [`Self::with_compilation_cache`].
    pub fn with_cache(mut self, cache: Arc<dyn Cache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Configures a compilation cache for the engine.
    pub fn with_compilation_cache(mut self, size: u64, ttl: Duration) -> Self {
        self.compilation_cache_size = Some(size);
        self.compilation_cache_ttl = Some(ttl);
        self
    }

    /// Override the TLS provider used for `wasi:tls` client connections.
    ///
    /// Use this to plug in an alternative TLS backend, install a custom root
    /// certificate store (corporate CAs, certificate pinning), or integrate
    /// with HSM-backed key material.
    #[cfg(feature = "wasi-tls")]
    pub fn with_tls_provider(mut self, provider: SharedTlsProvider) -> Self {
        self.tls_provider = Some(provider);
        self
    }
}

impl EngineBuilder {
    /// Builds and returns a configured [`Engine`].
    ///
    /// This method finalizes the configuration and creates the engine.
    /// It automatically enables async support which is required for
    /// component execution.
    ///
    /// # Returns
    /// A new `Engine` instance configured with the builder's settings.
    ///
    /// # Errors
    /// Returns an error if the wasmtime engine creation fails.
    pub fn build(mut self) -> anyhow::Result<Engine> {
        #[cfg(feature = "wasi-tls")]
        {
            crate::init_crypto();
            if self.tls_provider.is_none() {
                tracing::warn!(
                    "wasi-tls is enabled but no TLS provider was set; \
                     falling back to wasmtime-wasi-tls default — \
                     set one via EngineBuilder::with_tls_provider"
                );
            }
        }

        // Start from the caller-supplied base config (or wasmtime's default) and
        // layer the builder-managed settings on top of it.
        // A caller's base config may also set what the builder otherwise
        // leaves at wasmtime's defaults (the fiber stack size among them); the
        // pool advisories can only assume the defaults when there is none.
        let has_custom_config = self.config.is_some();
        let mut config = self.config.take().unwrap_or_default();

        // Default the pooling allocator on only when starting from wasmtime's
        // default config. With a caller-supplied base config, leave its
        // allocation strategy untouched unless pooling was explicitly requested
        // (via with_pooling_allocator/with_pooling_config or WASMTIME_POOLING),
        // so a custom strategy is not silently overridden.
        let use_pooling_allocator = self
            .use_pooling_allocator
            .or_else(|| getenv::<bool>("WASMTIME_POOLING"))
            .unwrap_or(!has_custom_config);

        // Read once: each of the derivations and checks below needs it, and
        // where there is no cgroup it means asking the OS for physical memory.
        let memory_limit = host_memory::detected_memory_limit();
        // Resolved before the pooling config, which both of its knobs feed.
        let host_memory = self
            .host_memory
            .unwrap_or_else(|| host_memory::HostMemoryBudgets::derived_within(memory_limit));
        // Reads the resolved budgets, not the installed pool: it advises on the
        // knobs as configured. What the pool reserves is judged below, once
        // the pool that is actually installed is known.
        if let Some(advisory) = host_memory.advisory() {
            tracing::warn!("{advisory}");
        }
        // Only when the budget is a real ceiling: an over-large budget the host
        // merely counts against costs nothing.
        if self.guest_memory_mode.enforces()
            && let Some(advisory) = host_memory.enforcement_advisory_within(memory_limit)
        {
            tracing::warn!("{advisory}");
        }
        // Only for a value the operator named: wasmtime's default is upstream's
        // call. And only when the guest budget is a ceiling: otherwise nothing
        // is held back from guests to compare against.
        if self.guest_memory_mode.enforces()
            && let Some(fuel) = self.hostcall_fuel
            && let Some(advisory) = host_memory::hostcall_fuel_advisory_within(
                u64::try_from(fuel.get()).unwrap_or(u64::MAX),
                host_memory.max_guest_memory,
                memory_limit,
            )
        {
            tracing::warn!("{advisory}");
        }
        anyhow::ensure!(
            !(self.strict_process_memory && self.max_process_memory_off),
            "strict process memory cannot be combined with disabling max-process-memory"
        );
        let max_process_memory_bytes = if self.max_process_memory_off {
            None
        } else {
            Some(
                host_memory::resolve_process_memory_limit(
                    self.max_process_memory_bytes,
                    memory_limit,
                    self.strict_process_memory,
                )
                .map_err(anyhow::Error::msg)?,
            )
        };
        // Only for a limit the operator named: a derived one is a share of the
        // very limit this checks against.
        if !self.max_process_memory_off
            && let Some(bytes) = self.max_process_memory_bytes
            && let Some(advisory) = host_memory::process_memory_advisory_within(bytes, memory_limit)
        {
            tracing::warn!("{advisory}");
        }
        // What the startup line says the process is held to.
        let max_process_memory =
            max_process_memory_bytes.map_or_else(|| "off".to_string(), host_memory::render_bytes);

        // Resolved here, ahead of the pool advisories, which count a live
        // memory's mappings by it; applied to the config further down. What
        // the engine will use: the explicit choice, or wasmtime's default of
        // on, or, under a base config that may set it, unknown until built.
        let memory_init_cow = self
            .memory_init_cow
            .or_else(host_memory::memory_init_cow_from_env);
        let assumed_memory_init_cow = memory_init_cow.or((!has_custom_config).then_some(true));

        // Pooled memories cannot outgrow the reservation. Custom pooling
        // configs and environment overrides supply their own heap ceiling.
        let layout = self.memory_layout.unwrap_or_default();
        let heap_ceiling = match self.memory_layout {
            Some(layout) => host_memory.default_heap_memory.min(layout.reservation),
            None => host_memory.default_heap_memory,
        };
        if layout.is_reduced() {
            tracing::warn!(
                memory_reservation = %host_memory::render_bytes(layout.reservation),
                memory_guard = %host_memory::render_bytes(layout.guard),
                default_heap_memory = %host_memory::render_bytes(heap_ceiling),
                "guest memory layout is below wasmtime's default, reducing hardware \
                 protection and relying more on software bounds checks for guest isolation; \
                 this may slow guest memory access"
            );
        }
        let mut installed_pool: Option<InstalledPool> = None;
        let mut fallback_pool = None;
        let sized_to_commit_limit = self.sized_to_commit_limit();
        if use_pooling_allocator
            && let Ok(true) = on_signal_quiet_thread(is_pooling_allocator_supported)
        {
            tracing::debug!("using pooling allocator by default");
            let pooling = self.pooling_config.take().unwrap_or_else(|| {
                // The builder's own knobs win over the budgets'. A memory
                // count set here re-derives the core total, since the
                // budgets' was derived beside a different memory count.
                let builder_memories = self
                    .max_memories
                    .map(|m| m.count(memory_init_cow, self.memory_layout));
                let memories = builder_memories.unwrap_or(host_memory.max_memories);
                let core_instances = self.max_core_instances.unwrap_or_else(|| {
                    if builder_memories.is_some() {
                        derived_pool_totals(memories).core_instances
                    } else {
                        host_memory.core_instances
                    }
                });
                if let Some(advisory) =
                    host_memory::core_instance_mismatch(memories, core_instances)
                {
                    tracing::warn!("{advisory}");
                }
                let pooling = new_pooling_config(memories, core_instances, heap_ceiling);
                fallback_pool = pool_startup::derived_fallback(
                    &pooling,
                    matches!(self.max_memories, Some(MaxMemories::Derived(_))),
                    self.max_core_instances.is_some(),
                    |key| pooling_env_total_quiet(key).is_some(),
                );
                pooling
            });
            installed_pool = Some(InstalledPool {
                max_memory_size: u64::try_from(pooling.get_max_memory_size()).unwrap_or(u64::MAX),
                layout,
                table_elements: pooling.get_table_elements(),
                totals: pool_usage::PoolTotals::of(&pooling),
            });
            config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pooling));
        } else if use_pooling_allocator {
            tracing::warn!("pooling allocator requested but not supported");
        }
        // Before the engine is built, so a pool that makes the build fail is
        // explained on the very start that fails. Against the pool that was
        // installed, not the knobs: the environment or a caller's own pooling
        // config can move its memory count and slot size, and with no pool
        // there is no reservation to warn about. The builder never sets the
        // fiber stack size, so it is wasmtime's default unless a base config
        // was given; that one case is checked once the engine can say.
        let assumed_stack_size =
            (!has_custom_config).then_some(host_memory::WASMTIME_DEFAULT_ASYNC_STACK_SIZE);
        if let Some(pool) = installed_pool {
            for advisory in host_memory::pool_advisories(
                &pool.totals,
                pool.max_memory_size,
                pool.table_elements,
                assumed_stack_size,
                assumed_memory_init_cow,
                sized_to_commit_limit,
                memory_limit,
                pool.layout,
            ) {
                tracing::warn!("{advisory}");
            }
        }
        // Only override fuel consumption when the caller explicitly set it, so a
        // custom base config's setting is otherwise preserved.
        if let Some(fuel) = self.fuel_consumption {
            config.consume_fuel(fuel);
        }

        // Compiling in parallel is wasmtime's own default, so this only has to
        // carry an explicit "off" through to the config.
        if let Some(parallel) = self
            .parallel_compilation
            .or_else(|| getenv::<bool>("WASMTIME_PARALLEL_COMPILATION"))
        {
            config.parallel_compilation(parallel);
        }

        // WASIP3's async ABI requires the component-model async proposal.
        self.proposals.insert(WasmProposal::ComponentModelAsync);

        // Accept components whose WIT uses `map<k, v>` types.
        self.proposals.insert(WasmProposal::ComponentModelMap);

        // Accept components that import an interface multiple times via the
        // component-model `(implements ..)` annotation, so host plugins can
        // route each named import (e.g. two `wasi:keyvalue/store` imports
        // backed by redis vs NATS) independently. Gated on the
        // `wasm_component_model_implements` feature, which is on by default and
        // also brings in the plugin-side routing.
        #[cfg(feature = "wasm_component_model_implements")]
        self.proposals
            .insert(WasmProposal::WasmComponentModelImplements);

        // Name the collector rather than taking `Collector::Auto`. `wasm_gc` is
        // on by default, so a guest can allocate GC objects on any build, and
        // `Auto` resolves to whichever collector was compiled in — reaching the
        // null one, which reclaims nothing, when it is all that is left. Naming
        // it turns that into an `Engine::new` error naming the missing feature.
        config.collector(wasmtime::Collector::Copying);

        // Compile a deadline check into every guest's loop back-edges, so guest
        // work that never yields can still be ended. Every store must then set a
        // deadline of its own — see [`crate::engine::abandon::arm_epoch_deadline`],
        // which is what decides when one is acted on.
        config.epoch_interruption(true);

        // Unwind tables are wasmtime's own default, so this only carries an
        // explicit choice through. Windows refuses to drop them, so an "off"
        // is ignored there instead of failing `build`.
        if let Some(unwind) = self
            .native_unwind_info
            .or_else(|| getenv::<bool>("WASMTIME_NATIVE_UNWIND_INFO"))
        {
            config.native_unwind_info(unwind || cfg!(windows));
        }

        // Copy-on-write images are wasmtime's own default, so this only
        // carries an explicit choice through; see `with_memory_init_cow`.
        if let Some(cow) = memory_init_cow {
            config.memory_init_cow(cow);
        }
        // A reduced layout is the caller's opt-in; see `with_memory_layout`.
        if let Some(layout) = self.memory_layout {
            config.memory_reservation(layout.reservation);
            config.memory_guard_size(layout.guard);
        }

        for proposal in &self.proposals {
            proposal.apply(&mut config);
        }

        // The pool maps its whole slab here, so a pool too large for the
        // address space fails now, with wasmtime naming only the mapping. Say
        // which numbers produced it. A heap ceiling past the reservation is
        // refused here too, whatever the slot count, so that is named instead.
        // Where mappings are committed the table pool is charged here as
        // well, and it is a multiple of the memory count, so a build that
        // fails there must not send the operator to the reservation alone.
        let inner = pool_startup::build_with_fallback(&mut config, fallback_pool, |config| {
            on_signal_quiet_thread(|| Ok(wasmtime::Engine::new(config)?))
        });
        if let Ok(inner) = &inner
            && let Some(pool) = &mut installed_pool
            && let Some(pooling) = inner.get_pooling_config()
        {
            pool.totals = pool_usage::PoolTotals::of(pooling);
        }
        let inner = inner.map_err(|e| match installed_pool {
            Some(pool) if pool.max_memory_size > pool.layout.reservation => e.context(format!(
                "building the engine failed with a pooling allocator whose heap ceiling \
                     (default-heap-memory) of {} is past the {} memory reservation, \
                     which the pooling allocator refuses; set default-heap-memory to {} or less",
                host_memory::render_bytes(pool.max_memory_size),
                host_memory::render_bytes(pool.layout.reservation),
                host_memory::render_bytes(pool.layout.reservation),
            )),
            Some(pool) => e.context(format!(
                "building the engine failed with a pooling allocator reserving {} of address \
                 space ({} memories of {} each) out of the {} this process can map, and {}",
                host_memory::render_bytes(pool.reservation()),
                pool.totals.memories,
                host_memory::render_bytes(host_memory::pool_slot_bytes(
                    pool.max_memory_size,
                    pool.layout
                )),
                host_memory::render_bytes(host_memory::usable_address_space()),
                host_memory::pool_startup_commit(
                    &pool.totals,
                    pool.table_elements,
                    assumed_stack_size,
                ),
            )),
            None => e,
        })?;
        let installed_heap_memory = installed_pool.map(|pool| pool.max_memory_size);
        let pool_totals = installed_pool.map(|pool| pool.totals);
        // The counts the pool installed, which the builder's own knobs, the
        // environment or a caller's pooling config may have moved.
        let host_memory = match pool_totals {
            Some(totals) => host_memory::HostMemoryBudgets {
                max_memories: totals.memories,
                core_instances: totals.core_instances,
                ..host_memory
            },
            None => host_memory,
        };
        // Reported here so every embedder says what it built. The reservation
        // is `total_memories x slot_size`, not the instance count, and the slot
        // does not shrink with the heap ceiling.
        match installed_pool {
            Some(pool) => tracing::info!(
                %max_process_memory,
                max_guest_memory = %host_memory::render_bytes(host_memory.max_guest_memory),
                guest_memory_mode = self.guest_memory_mode.as_str(),
                default_heap_memory = %host_memory::render_bytes(pool.max_memory_size),
                core_instances = pool.totals.core_instances,
                memories = pool.totals.memories,
                slot_size = %host_memory::render_bytes(host_memory::pool_slot_bytes(pool.max_memory_size, pool.layout)),
                pool_reservation = %host_memory::render_bytes(pool.reservation()),
                "host memory resolved"
            ),
            // No pool, so no reservation and no installed ceiling to name.
            None => tracing::info!(
                %max_process_memory,
                max_guest_memory = %host_memory::render_bytes(host_memory.max_guest_memory),
                guest_memory_mode = self.guest_memory_mode.as_str(),
                pooling = false,
                "host memory resolved"
            ),
        }

        // The advisories deferred above: with a base config only the engine
        // knows its stack size and copy-on-write setting. The mapping count
        // was checked before the build at its lower bound, without images;
        // an engine that maps them gets the full count, unless the lower
        // bound already warned.
        if let Some(pool) = installed_pool
            && assumed_memory_init_cow.is_none()
            && inner.get_memory_init_cow()
            && host_memory::pool_map_count_advisory_for(&pool.totals, false).is_none()
            && let Some(advisory) = host_memory::pool_map_count_advisory_for(&pool.totals, true)
        {
            tracing::warn!("{advisory}");
        }
        if let Some(pool) = installed_pool
            && assumed_stack_size.is_none()
            && let Some(advisory) = host_memory::pool_stack_advisory(
                pool.totals.stacks,
                inner.get_async_stack_size(),
                sized_to_commit_limit,
                memory_limit,
            )
        {
            tracing::warn!("{advisory}");
        }

        spawn_epoch_ticker(&inner)?;
        let process_memory = match max_process_memory_bytes {
            Some(limit) => {
                let guard = Arc::new(process_memory::ProcessMemoryGuard::new(limit));
                guard.spawn_sampler()?;
                process_memory::ProcessMemoryGuard::register_metrics(
                    &opentelemetry::global::meter("wash-runtime"),
                );
                Some(guard)
            }
            None => None,
        };
        // Growth mode's bound on uncharged initial memory: named, or derived.
        let max_initial_guest_memory = self.max_initial_guest_memory_bytes.unwrap_or(
            host_memory
                .max_guest_memory
                .saturating_mul(guest_memory::DEFAULT_INITIAL_OVERCOMMIT),
        );
        if self.guest_memory_mode == guest_memory::GuestMemoryMode::Growth {
            tracing::info!(
                max_initial_guest_memory = %host_memory::render_bytes(max_initial_guest_memory),
                "guest-memory-mode is growth: initial memory is uncharged up to this cap"
            );
        } else if self.max_initial_guest_memory_bytes.is_some() {
            tracing::warn!(
                guest_memory_mode = self.guest_memory_mode.as_str(),
                "max-initial-guest-memory is set but has no effect: only guest-memory-mode 
                 growth leaves initial memory uncharged"
            );
        }
        #[cfg_attr(not(test), allow(unused_variables))]
        let (cache, default_cache): (Arc<dyn Cache>, _) = match self.cache {
            Some(cache) => (cache, None),
            None => {
                let default = DefaultCache::new(
                    self.compilation_cache_size.unwrap_or(100),
                    self.compilation_cache_ttl
                        .unwrap_or(Duration::from_secs(600)),
                );
                (Arc::new(default.clone()), Some(default))
            }
        };
        self.reserved_host_paths.extend(cache.reserved_paths());
        let socket_policy = self.socket_policy.unwrap_or_default();
        let scratch_root = self
            .scratch_root
            .unwrap_or_else(|| std::env::temp_dir().join("wasmcloud-scratch"));
        let scratch = scratch::ScratchRoot::open(&scratch_root).map_err(|e| {
            tracing::warn!(root = %scratch_root.display(), err = %e, "cannot claim the scratch root; emptyDir volumes will be refused");
            Arc::from(format!("{e:#}"))
        });
        Ok(Engine {
            stream_payloads: Arc::default(),
            base_linkers: Arc::default(),
            inner,
            cache,
            #[cfg(test)]
            default_cache,
            cache_engine_id: uuid::Uuid::new_v4(),
            compile_flights: Arc::new(Coalescer::flights(Duration::MAX)),
            compiling: Arc::new(Coalescer::flights(Duration::MAX)),
            live_compiles: Arc::default(),
            #[cfg(test)]
            compiles: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            host_paths: volumes::HostPathPolicy::new(
                self.reserved_host_paths,
                self.allowed_host_paths,
                Some(scratch_root.clone()),
                // The socket policy's counters, so an operator reads every
                // policy figure in one place.
                socket_policy.meters.clone(),
            ),
            socket_policy,
            scratch,
            host_memory,
            guest_memory: {
                let budget = guest_memory::GuestMemoryBudget::from_budgets(
                    &host_memory,
                    self.guest_memory_mode,
                );
                // The ceiling the pool actually installed, not the knob that
                // asked for it: the budget must not charge for growth the pool
                // is going to refuse. The same figure `explain_compile_failure`
                // measures against, so they cannot drift apart.
                let budget = match installed_heap_memory {
                    Some(ceiling) => budget.with_heap_ceiling(ceiling),
                    None => budget,
                };
                let budget = budget.with_initial_cap(max_initial_guest_memory);
                match &process_memory {
                    Some(guard) => budget.with_process_guard(Arc::clone(guard)),
                    None => budget,
                }
                .into_metered()
            },
            store_settings: store_settings::StoreSettings {
                hostcall_fuel: self.hostcall_fuel,
            },
            process_memory,
            #[cfg(feature = "wasi-tls")]
            tls_provider: self.tls_provider,
            installed_heap_memory,
            pool_totals,
        })
    }
}

/// Helper function to determine if a component uses wasi:http interfaces
pub fn uses_wasi_http(component: &Component) -> bool {
    imports_wasi_http(component) || exports_wasi_http(component)
}

pub fn exports_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.exports(engine)
        .any(|(export, _item)| export.starts_with("wasi:http"))
}

/// Whether a component exports the `wasmcloud:messaging/handler` interface (the
/// inbound message handler the host invokes).
pub fn exports_messaging_handler(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.exports(engine)
        .any(|(export, _item)| export.starts_with("wasmcloud:messaging/handler"))
}

pub fn imports_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.imports(engine)
        .any(|(import, _item)| import.starts_with("wasi:http"))
}

// TL;DR this is likely best for machines that can handle the large virtual memory requirement of the pooling allocator
// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn is_pooling_allocator_supported() -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}

fn getenv<T>(key: &str) -> Option<T>
where
    T: FromStr,
    T::Err: core::fmt::Debug,
{
    match env::var(key).as_deref().map(FromStr::from_str) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(err)) => {
            warn!(?err, "failed to parse `{key}` value, ignoring");
            None
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(..)) => {
            warn!("`{key}` value is not valid UTF-8, ignoring");
            None
        }
    }
}

/// Reports a `WASMTIME_POOLING_*` value that overrides a resolved flag.
///
/// The flags are what an operator set and what every sizing rule is derived
/// against; these envs are applied on top of them, here. Where the two differ
/// the host runs on the env, and an operator reading back the flag they set
/// has no way to tell — so this says which one won, once, at startup.
///
/// It does not say `host memory resolved` is wrong. That line is read back
/// from the pool that was actually built ([`InstalledPool`]), so for the three
/// knobs it names it already prints the overridden number; the other four it
/// does not mention at all.
///
/// Also refuses a zero. Every one of these knobs means "this host runs
/// nothing" at zero, which is never what an operator meant, and the flags they
/// shadow are already rejected for exactly that.
fn pooling_env_override<T>(key: &str, resolved: T, render: impl Fn(T) -> String) -> Option<T>
where
    T: FromStr + PartialEq + Default + std::fmt::Display + Copy,
    T::Err: core::fmt::Debug,
{
    let value: T = getenv(key)?;
    if value == T::default() {
        warn!(
            "`{key}` is 0, which would leave this host unable to instantiate anything; \
             ignoring it and keeping {}",
            render(resolved)
        );
        return None;
    }
    if value != resolved {
        // Rendered, not raw: the byte-sized knobs go through `render_bytes`,
        // and an operator cannot compare 4294967296 to 256MiB.
        let (resolved, value) = (render(resolved), render(value));
        warn!(
            resolved = %resolved,
            override_value = %value,
            "`{key}` overrides the flag it shadows: this host runs with {value}, not the \
             {resolved} it was configured with"
        );
    }
    Some(value)
}

/// A `WASMTIME_POOLING_*` value as [`new_pooling_config`] will apply it,
/// read without the warnings it gives, so sizing can follow the pool the
/// engine installs: unset or unparseable is `None`.
fn pooling_env_quiet<T: FromStr>(key: &str) -> Option<T> {
    env::var(key).ok()?.parse().ok()
}

/// [`pooling_env_quiet`] for a total, which [`pooling_env_override`] ignores
/// at zero.
fn pooling_env_total_quiet(key: &str) -> Option<u32> {
    pooling_env_quiet(key).filter(|&total| total != 0)
}

/// [`pooling_env_override`] for the knobs that are plain instance counts.
///
/// All of them derive from the resolved `--max-memories` and all of them
/// mean "this host runs nothing" at zero, so all of them want the same guard.
fn pooling_env_instances(key: &str, resolved: u32) -> u32 {
    pooling_env_override(key, resolved, |v| v.to_string()).unwrap_or(resolved)
}

/// Core instances the pool admits per linear memory it holds, and the ratio
/// `--max-memories` alone derives the core-instance total by.
///
/// Why 16. A component instance is one linear memory but several core
/// instances, one per core module wasmtime instantiates for it: a Rust p2
/// HTTP handler is four (the main module, the `wasi_snapshot_preview1`
/// adapter and two wit-component glue modules), a Componentize-Go component
/// five (the same with three glue modules), and a composition adds every
/// member's ([`pool_share::PoolCost::USUAL`] carries the measured shape).
/// With every total set to the memory count, the core-instance total was
/// what a warm pool filled first, at a quarter of the memories the pool had
/// reserved address space for.
///
/// Nothing is reserved for a core instance: wasmtime counts live ones against
/// this total and allocates each VMContext on the heap at its own size, which
/// the process memory guard accounts for. So the ratio costs nothing to set
/// high, and the only thing it must do is stand clear of any shape a host
/// runs, so that the memories, which do reserve, are what a full pool runs
/// out of. Sixteen is the largest measured shape, five, with three shapes'
/// worth of headroom for compositions and toolchains not measured here; a
/// composition of four Componentize-Go members fits one memory's worth of
/// core instances only if it shares memories, so a host running such
/// compositions sets `--core-instances` itself. Should the ratio become
/// configurable, it is this one number.
pub(crate) const CORE_INSTANCES_PER_MEMORY: u32 = 16;

/// Tables the pool admits per linear memory it holds. The same shapes hold one
/// or two tables (the main module's function table and the wit-component
/// shim's), so a total equal to the linear-memory count bound a warm pool at
/// half its memories.
///
/// Each pooled table is `table_elements` pointers of address space (160 KiB at
/// the default 20,000) in one mapping that is never split, so this costs no
/// `vm.max_map_count`. Where the mapping is committed at startup (Windows,
/// `vm.overcommit_memory=2`, a finite `RLIMIT_DATA`), the table pool's commit
/// charge grows with it; elsewhere it costs nothing until a table is used.
const TABLES_PER_MEMORY: u32 = 4;

/// The pool totals `instances` linear memories resolve to, before the
/// environment is consulted.
///
/// `instances` is the resolved `--max-memories`. It is the number of linear
/// memories, which reserve the pool's address space, and also of the fiber
/// stacks, component instances and GC heaps, which a component instance of
/// the usual shape ([`pool_share::PoolCost::USUAL`]) takes one of each
/// alongside its memory. The core instance and table totals, which reserve
/// nothing, are multiples of it ([`CORE_INSTANCES_PER_MEMORY`],
/// [`TABLES_PER_MEMORY`]) so that a component made of several core modules
/// does not spend several memories' worth of them.
///
/// Apart from [`new_pooling_config`] so the derivation can be pinned by a test
/// that no exported `WASMTIME_POOLING_TOTAL_*` override can disturb, and so
/// a test that needs a stock pool's shape takes it from here rather than
/// hard-coding the multiples.
pub(crate) fn derived_pool_totals(instances: u32) -> pool_usage::PoolTotals {
    pool_usage::PoolTotals {
        memories: instances,
        stacks: instances,
        component_instances: instances,
        gc_heaps: instances,
        core_instances: instances.saturating_mul(CORE_INSTANCES_PER_MEMORY),
        tables: instances.saturating_mul(TABLES_PER_MEMORY),
    }
}

/// The pooling allocator for `instances` linear memories: the totals of
/// [`derived_pool_totals`], with `core_instances` in place of the derived
/// core-instance total, each of which can still be set on its own through
/// its `WASMTIME_POOLING_TOTAL_*` variable, and the per-slot limits the
/// environment names.
fn new_pooling_config(
    instances: u32,
    core_instances: u32,
    default_heap_memory: u64,
) -> PoolingAllocationConfig {
    let mut derived = derived_pool_totals(instances);
    derived.core_instances = core_instances;
    let mut config = PoolingAllocationConfig::default();
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_UNUSED_WASM_SLOTS") {
        config.max_unused_warm_slots(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_DECOMMIT_BATCH_SIZE") {
        config.decommit_batch_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT") {
        config.async_stack_keep_resident(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT") {
        config.linear_memory_keep_resident(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TABLE_KEEP_RESIDENT") {
        config.table_keep_resident(v);
    }
    config.total_component_instances(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES",
        derived.component_instances,
    ));
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_COMPONENT_INSTANCE_SIZE") {
        config.max_component_instance_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_CORE_INSTANCES_PER_COMPONENT") {
        config.max_core_instances_per_component(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_MEMORIES_PER_COMPONENT") {
        config.max_memories_per_component(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_TABLES_PER_COMPONENT") {
        config.max_tables_per_component(v);
    }
    config.total_memories(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_MEMORIES",
        derived.memories,
    ));
    config.total_tables(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_TABLES",
        derived.tables,
    ));
    config.total_stacks(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_STACKS",
        derived.stacks,
    ));
    // A counter, not a reservation: `host memory resolved` reports it beside
    // the memory count that actually sizes the pool.
    config.total_core_instances(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_CORE_INSTANCES",
        derived.core_instances,
    ));
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_CORE_INSTANCE_SIZE") {
        config.max_core_instance_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_TABLES_PER_MODULE") {
        config.max_tables_per_module(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TABLE_ELEMENTS") {
        config.table_elements(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_MEMORIES_PER_MODULE") {
        config.max_memories_per_module(v);
    }
    // Unlike every other knob in this function this one had no `else` branch,
    // so wasmtime's default stood on every host and nothing named it. The
    // fallback is that same default unless an operator set the flag, so this
    // changes no behaviour by itself — it only makes the number reachable.
    // It is also the number a component's minimum linear memory is checked
    // against, so an env that shadows `--default-heap-memory` here is worth
    // saying out loud.
    let resolved_heap = usize::try_from(default_heap_memory).unwrap_or(usize::MAX);
    config.max_memory_size(
        pooling_env_override("WASMTIME_POOLING_MAX_MEMORY_SIZE", resolved_heap, |v| {
            host_memory::render_bytes(u64::try_from(v).unwrap_or(u64::MAX))
        })
        .unwrap_or(resolved_heap),
    );
    #[cfg(not(windows))]
    config.total_gc_heaps(pooling_env_instances(
        "WASMTIME_POOLING_TOTAL_GC_HEAPS",
        derived.gc_heaps,
    ));
    config
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Heap refusals name the installed ceiling and a size that fits.
    #[test]
    fn a_heap_floor_refusal_names_the_flag_and_both_sizes() {
        let raw = wasmtime::Error::msg(
            "module memory does not fit in pooling allocator requirements: memory has a \
             minimum byte size of 2424832 which exceeds the limit of 0x100000",
        );
        let explained = format!(
            "{:#}",
            explain_compile_failure(raw, 1024 * 1024, host_memory::WASMTIME_MEMORY_RESERVATION)
        );
        assert!(explained.contains("2.3MiB"), "{explained}");
        assert!(
            explained.contains("per-memory ceiling is 1MiB"),
            "{explained}"
        );
        assert!(
            explained.contains("Raise --default-heap-memory to at least 4MiB"),
            "{explained}"
        );
        assert!(!explained.contains("--memory-reservation"), "{explained}");
        assert!(explained.contains("pooling allocator"), "{explained}");
    }

    /// wasmtime renders the limit half of this message in hex, and a size it
    /// did not print as plain decimal used to parse as `0` — advising an
    /// operator to raise the flag to 4MiB for a component needing 0 bytes.
    #[test]
    fn a_refusal_without_a_decimal_size_is_left_alone() {
        for text in [
            "memory has a minimum byte size of 0x250000 which exceeds the limit of 0x100000",
            "memory has a minimum byte size of 0 which exceeds the limit of 0x100000",
        ] {
            let explained = format!(
                "{:#}",
                explain_compile_failure(
                    wasmtime::Error::msg(text),
                    1024 * 1024,
                    host_memory::WASMTIME_MEMORY_RESERVATION,
                )
            );
            assert_eq!(explained, text, "should not have been annotated");
        }
    }

    /// `next_power_of_two` panics under `debug_assertions` and wraps to zero in
    /// release, so a refusal quoting a size near `u64::MAX` used to take the
    /// host out rather than explain itself.
    #[test]
    fn an_unroundable_requirement_does_not_panic() {
        let raw = wasmtime::Error::msg(format!(
            "memory has a minimum byte size of {} which exceeds the limit of 0x100000",
            u64::MAX
        ));
        let explained = format!(
            "{:#}",
            explain_compile_failure(raw, 1024 * 1024, host_memory::WASMTIME_MEMORY_RESERVATION)
        );
        assert!(explained.contains("Raise it to at least"), "{explained}");
    }

    #[test]
    fn an_unrelated_compile_failure_is_left_alone() {
        let raw = wasmtime::Error::msg("expected a WebAssembly component");
        let explained = format!(
            "{:#}",
            explain_compile_failure(raw, 1024 * 1024, host_memory::WASMTIME_MEMORY_RESERVATION)
        );
        assert_eq!(explained, "expected a WebAssembly component");
    }

    #[test]
    fn heap_refusal_advice_stays_within_the_memory_reservation() {
        for (reservation, suggested) in [(3 * host_memory::MIB, "3MiB"), (2_500_000, "2500000B")] {
            let raw = wasmtime::Error::msg(
                "memory has a minimum byte size of 2424832 which exceeds the limit of 0x100000",
            );
            let explained = format!(
                "{:#}",
                explain_compile_failure(raw, host_memory::MIB, reservation)
            );
            assert!(
                explained.contains(&format!(
                    "Raise --default-heap-memory to at least {suggested}"
                )),
                "{explained}"
            );
            assert!(!explained.contains("--memory-reservation"), "{explained}");
        }
    }

    fn volume_only_workload(volumes: Vec<crate::types::Volume>) -> Workload {
        Workload {
            namespace: "ns".into(),
            name: "volumes".into(),
            annotations: Default::default(),
            service: None,
            components: vec![],
            host_interfaces: vec![],
            volumes,
        }
    }

    fn host_path(path: &std::path::Path) -> crate::types::Volume {
        crate::types::Volume {
            name: "data".into(),
            volume_type: VolumeType::HostPath(HostPathVolume {
                local_path: path.to_string_lossy().into_owned(),
            }),
        }
    }

    #[tokio::test]
    async fn an_engine_refuses_a_host_path_outside_its_allowlist() {
        let root = tempfile::tempdir().unwrap();
        let allowed = root.path().join("allowed");
        let other = root.path().join("other");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let meters = Arc::new(crate::host::quota::PolicyMeters::default());
        let engine = Engine::builder()
            .with_socket_policy(Arc::new(crate::sockets::policy::SocketPolicy {
                meters: Some(Arc::clone(&meters)),
                ..Default::default()
            }))
            .with_allowed_host_paths([allowed.clone()])
            .build()
            .unwrap();

        let Err(err) = engine
            .initialize_workload("w", volume_only_workload(vec![host_path(&other)]))
            .await
        else {
            panic!("a host path outside the allowlist should be refused");
        };
        assert!(
            format!("{err:#}").contains("--allowed-host-path"),
            "{err:#}"
        );
        assert_eq!(meters.host_path_denied(), 1);
        engine
            .initialize_workload("w", volume_only_workload(vec![host_path(&allowed)]))
            .await
            .unwrap();
        // An empty directory is the host's own, never a path a workload named.
        engine
            .initialize_workload(
                "w",
                volume_only_workload(vec![crate::types::Volume {
                    name: "scratch".into(),
                    volume_type: VolumeType::EmptyDir(EmptyDirVolume {}),
                }]),
            )
            .await
            .unwrap();
    }

    fn empty_dir(name: &str) -> crate::types::Volume {
        crate::types::Volume {
            name: name.into(),
            volume_type: VolumeType::EmptyDir(EmptyDirVolume {}),
        }
    }

    /// Scratch lives under the root the engine owns, and a workload that goes
    /// away takes its scratch with it.
    #[tokio::test]
    async fn empty_dir_scratch_lives_under_the_root_and_leaves_with_its_workload() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::builder()
            .with_scratch_root(root.path())
            .build()
            .unwrap();
        let scratch_files = || {
            walkdir(root.path())
                .into_iter()
                .filter(|p| !p.ends_with(".lock"))
                .count()
        };
        let before = scratch_files();
        let Ok(workload) = engine
            .initialize_workload(
                "w",
                volume_only_workload(vec![empty_dir("a"), empty_dir("b")]),
            )
            .await
        else {
            panic!("a workload with emptyDir volumes should initialize");
        };
        // One workload directory holding a directory per volume.
        assert_eq!(scratch_files(), before + 3);
        drop(workload);
        assert_eq!(scratch_files(), before);
    }

    /// A volume around the scratch root, as `/tmp` is around the default one,
    /// the root itself, or a workload's scratch inside it would expose other
    /// workloads' scratch, so each is refused even when the allowlist names it.
    #[tokio::test]
    async fn a_host_path_exposing_scratch_is_refused_even_when_allowed() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("scratch");
        let meters = Arc::new(crate::host::quota::PolicyMeters::default());
        let engine = Engine::builder()
            .with_socket_policy(Arc::new(crate::sockets::policy::SocketPolicy {
                meters: Some(Arc::clone(&meters)),
                ..Default::default()
            }))
            .with_scratch_root(&scratch)
            .with_allowed_host_paths([root.path().to_path_buf(), scratch.clone()])
            .build()
            .unwrap();
        let workload = engine
            .initialize_workload("w", volume_only_workload(vec![empty_dir("a")]))
            .await
            .unwrap();
        let inside = walkdir(&scratch)
            .into_iter()
            .find(|p| p.is_dir())
            .expect("the workload's scratch directory");
        for (what, path) in [
            ("around", root.path()),
            ("equal to", scratch.as_path()),
            ("inside", inside.as_path()),
        ] {
            assert!(
                engine
                    .initialize_workload("w2", volume_only_workload(vec![host_path(path)]))
                    .await
                    .is_err(),
                "{what} the scratch root"
            );
        }
        assert_eq!(meters.host_path_denied(), 3);
        drop(workload);
    }

    fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walkdir(&path));
            }
            found.push(path);
        }
        found
    }

    /// With no allowed host paths configured, no `hostPath` volume is permitted.
    #[tokio::test]
    async fn a_default_engine_refuses_every_host_path() {
        let root = tempfile::tempdir().unwrap();
        let meters = Arc::new(crate::host::quota::PolicyMeters::default());
        let engine = Engine::builder()
            .with_socket_policy(Arc::new(crate::sockets::policy::SocketPolicy {
                meters: Some(Arc::clone(&meters)),
                ..Default::default()
            }))
            .build()
            .unwrap();
        assert!(
            engine
                .initialize_workload("w", volume_only_workload(vec![host_path(root.path())]))
                .await
                .is_err()
        );
        assert_eq!(meters.host_path_denied(), 1);
    }

    // Compiling is parallel unless a host says otherwise, and saying so
    // reaches the engine rather than being dropped on the builder.
    #[test]
    fn parallel_compilation_is_on_and_can_be_turned_off() {
        let engine = Engine::builder().build().expect("default should build");
        assert!(engine.inner().get_parallel_compilation());

        let engine = Engine::builder()
            .with_parallel_compilation(false)
            .build()
            .expect("serial compilation should build");
        assert!(!engine.inner().get_parallel_compilation());
    }

    // Copy-on-write images follow the builder, whichever way it is set. The
    // default is not asserted: `WASMTIME_MEMORY_INIT_COW` in the test's own
    // environment (a bench runner's, say) would decide it, not the builder.
    #[test]
    fn memory_init_cow_follows_the_builder() {
        let engine = Engine::builder()
            .with_memory_init_cow(true)
            .build()
            .expect("images on should build");
        assert!(engine.inner().get_memory_init_cow());
        let engine = Engine::builder()
            .with_memory_init_cow(false)
            .build()
            .expect("turning copy-on-write images off should build");
        assert!(!engine.inner().get_memory_init_cow());
    }

    // Growth mode's initial cap: derived from the budget unless named.
    #[test]
    fn growth_mode_caps_uncharged_initial_memory() {
        let budgets = || host_memory::HostMemoryBudgets::resolve(Some(64 << 20), None, None, None);
        let engine = Engine::builder()
            .with_host_memory(budgets().unwrap())
            .with_guest_memory_mode(guest_memory::GuestMemoryMode::Growth)
            .build()
            .expect("growth mode should build");
        assert_eq!(
            engine.guest_memory().initial_cap(),
            (64 << 20) * guest_memory::DEFAULT_INITIAL_OVERCOMMIT
        );
        let engine = Engine::builder()
            .with_host_memory(budgets().unwrap())
            .with_guest_memory_mode(guest_memory::GuestMemoryMode::Growth)
            .with_max_initial_guest_memory_bytes(10 << 20)
            .build()
            .expect("an explicit cap should build");
        assert_eq!(engine.guest_memory().initial_cap(), 10 << 20);
    }

    // A reduced layout lowers the pool's heap ceiling to its reservation, so
    // the pool builds rather than being refused.
    #[test]
    fn a_reduced_memory_layout_lowers_the_heap_ceiling() {
        let layout = host_memory::MemoryLayout {
            reservation: 256 << 20,
            guard: 64 << 10,
        };
        let engine = Engine::builder()
            .with_pooling_allocator(true)
            .with_max_memories(10)
            .with_memory_layout(layout)
            .build()
            .expect("a reduced layout should build");
        if let Some(heap) = engine.installed_heap_memory {
            assert_eq!(heap, 256 << 20);
        }
    }

    #[test]
    fn a_custom_memory_layout_matches_the_pool_slot_stride() -> anyhow::Result<()> {
        let layout = host_memory::MemoryLayout {
            reservation: host_memory::MIB + 1,
            guard: 1,
        };
        let engine = Engine::builder()
            .with_pooling_allocator(true)
            .with_max_memories(2)
            .with_memory_layout(layout)
            .build()?;
        if engine.pool_totals().is_none() {
            return Ok(());
        }
        let bytes = wat::parse_str("(module (memory (export \"memory\") 1))")?;
        let module = wasmtime::Module::new(engine.inner(), bytes)?;
        let mut store = wasmtime::Store::new(engine.inner(), ());
        let mut allocate = || -> anyhow::Result<usize> {
            let instance = wasmtime::Instance::new(&mut store, &module, &[])?;
            let memory = instance.get_memory(&mut store, "memory").unwrap();
            Ok(memory.data_ptr(&store) as usize)
        };
        let stride = allocate()?.abs_diff(allocate()?);
        assert_eq!(
            u64::try_from(stride)?,
            host_memory::pool_slot_bytes(layout.reservation, layout)
        );
        Ok(())
    }

    #[test]
    fn a_memory_reservation_refusal_names_both_flags_and_recovers() -> anyhow::Result<()> {
        use host_memory::{HostMemoryBudgets, MIB, MemoryLayout};

        let bytes = wat::parse_str(
            "(component (core module $m (memory 32)) (core instance (instantiate $m)))",
        )?;
        let budgets = HostMemoryBudgets::resolve(None, Some(4 * MIB), Some(2), None)
            .map_err(anyhow::Error::msg)?;
        let build = |reservation| {
            Engine::builder()
                .with_pooling_allocator(true)
                .with_host_memory(budgets)
                .with_memory_layout(MemoryLayout {
                    reservation,
                    guard: 0,
                })
                .build()
        };
        let engine = build(MIB)?;
        if engine.pool_totals().is_none() {
            return Ok(());
        }
        let Err(error) = engine.compile(&bytes) else {
            panic!("a 2 MiB minimum must exceed a 1 MiB reservation");
        };
        let explained = format!("{error:#}");
        for expected in [
            "per-memory ceiling is 1MiB",
            "--memory-reservation is 1MiB",
            "Raise it to at least 4MiB",
            "runtime.resources.memoryReservation",
            "ensure --default-heap-memory permits that size",
            "runtime.resources.defaultHeapMemory",
        ] {
            assert!(explained.contains(expected), "{explained}");
        }
        build(4 * MIB)?.compile(&bytes)?;
        Ok(())
    }

    #[test]
    fn an_unpooled_memory_can_outgrow_its_reservation_within_the_budget() -> anyhow::Result<()> {
        use guest_memory::install_memory_limiter;
        use host_memory::{HostMemoryBudgets, MIB, MemoryLayout};

        for builder in [
            Engine::builder().with_pooling_allocator(false),
            Engine::builder().with_config(wasmtime::Config::new()),
        ] {
            let engine = builder
                .with_host_memory(
                    HostMemoryBudgets::resolve(Some(2 * MIB), None, None, None)
                        .map_err(anyhow::Error::msg)?,
                )
                .with_memory_layout(MemoryLayout {
                    reservation: MIB,
                    guard: 0,
                })
                .build()?;
            assert!(engine.pool_totals().is_none());
            let ctx = ctx::SharedCtx::new(ctx::Ctx::builder("memory", "reservation").build())
                .with_guest_memory(engine.guest_memory());
            let mut store = wasmtime::Store::new(engine.inner(), ctx);
            install_memory_limiter(&mut store);
            let memory = wasmtime::Memory::new(&mut store, wasmtime::MemoryType::new(16, None))?;
            memory.grow(&mut store, 16)?;
            assert_eq!(memory.data_size(&store) as u64, 2 * MIB);
            assert_eq!(engine.guest_memory().in_use(), 2 * MIB);
            assert!(memory.grow(&mut store, 1).is_err());
            assert_eq!(engine.guest_memory().in_use(), 2 * MIB);
        }
        Ok(())
    }

    // Unwind tables are on unless a host turns them off, and turning them off
    // reaches the engine everywhere but Windows, which requires them.
    #[test]
    fn native_unwind_info_is_on_and_can_be_turned_off() {
        let engine = Engine::builder().build().expect("default should build");
        assert_eq!(engine.inner().get_native_unwind_info(), Some(true));

        let engine = Engine::builder()
            .with_native_unwind_info(false)
            .build()
            .expect("turning unwind info off should build");
        assert_eq!(engine.inner().get_native_unwind_info(), Some(cfg!(windows)));
    }

    // A custom base config can now be combined with the pooling allocator and
    // instance limits, which previously errored out of `build()`.
    #[test]
    fn custom_config_layers_with_pooling() {
        let mut cfg = wasmtime::Config::default();
        cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);

        Engine::builder()
            .with_config(cfg)
            .with_pooling_allocator(true)
            .with_max_memories(64)
            .build()
            .expect("custom config + pooling should build");
    }

    // A single extra proposal can be enabled on top of a custom config without
    // replacing the whole config.
    #[test]
    fn wasm_proposal_layers_with_custom_config() {
        Engine::builder()
            .with_config(wasmtime::Config::default())
            .with_wasm_proposal(WasmProposal::Gc)
            .build()
            .expect("custom config + GC proposal should build");
    }

    /// Linear memories, fiber stacks and component instances share one count;
    /// core instances and tables, which a component spends several of and
    /// which reserve nothing, stand clear so the linear memories are what a
    /// warm pool fills first.
    #[test]
    #[allow(deprecated)]
    fn pool_counters_do_not_bind_before_the_memories() {
        // The derivation, before the environment has its say.
        let derived = derived_pool_totals(1000);
        assert_eq!(derived.memories, 1000);
        assert_eq!(derived.stacks, 1000);
        assert_eq!(derived.component_instances, 1000);
        assert_eq!(derived.gc_heaps, 1000);
        assert_eq!(derived.core_instances, 16_000);
        assert_eq!(derived.tables, 4000);
        assert_eq!(derived_pool_totals(u32::MAX).core_instances, u32::MAX);

        // What the config and then the engine install is the derivation with
        // the environment applied, so this half holds in a shell that exports
        // a `WASMTIME_POOLING_TOTAL_*` override too: the expectation is read
        // through the same override the code is.
        let installed = |key, resolved| pooling_env_instances(key, resolved);
        let derived = derived_pool_totals(64);
        let config = new_pooling_config(
            64,
            derived.core_instances,
            host_memory::WASMTIME_DEFAULT_HEAP_MEMORY,
        );
        assert_eq!(
            config.get_total_memories(),
            installed("WASMTIME_POOLING_TOTAL_MEMORIES", derived.memories)
        );
        assert_eq!(
            config.get_total_core_instances(),
            installed(
                "WASMTIME_POOLING_TOTAL_CORE_INSTANCES",
                derived.core_instances
            )
        );
        assert_eq!(
            config.get_total_tables(),
            installed("WASMTIME_POOLING_TOTAL_TABLES", derived.tables)
        );

        let engine = Engine::builder()
            .with_max_memories(64)
            .build()
            .expect("pooling engine should build");
        if let Some(totals) = engine.pool_totals() {
            assert_eq!(totals, pool_usage::PoolTotals::of(&config));
            assert_eq!(engine.total_memories(), Some(config.get_total_memories()));
            assert_eq!(
                engine.total_core_instances(),
                Some(config.get_total_core_instances())
            );
        }
    }

    /// `--core-instances` beside `--max-memories` moves the core-instance
    /// total alone, leaving the memories and the other totals derived from
    /// the memory count, and the environment still has the last word over it.
    #[test]
    #[allow(deprecated)]
    fn an_explicit_core_instance_total_moves_only_that_total() {
        let installed = |key, resolved| pooling_env_instances(key, resolved);
        let derived = derived_pool_totals(64);
        let config = new_pooling_config(64, 100, host_memory::WASMTIME_DEFAULT_HEAP_MEMORY);
        assert_eq!(
            config.get_total_core_instances(),
            installed("WASMTIME_POOLING_TOTAL_CORE_INSTANCES", 100)
        );
        assert_eq!(
            config.get_total_memories(),
            installed("WASMTIME_POOLING_TOTAL_MEMORIES", derived.memories)
        );
        assert_eq!(
            config.get_total_tables(),
            installed("WASMTIME_POOLING_TOTAL_TABLES", derived.tables)
        );

        let engine = Engine::builder()
            .with_max_memories(64)
            .with_max_core_instances(100)
            .without_max_process_memory()
            .build()
            .expect("pooling engine should build");
        if engine.pool_totals().is_some() {
            assert_eq!(
                engine.total_core_instances(),
                Some(installed("WASMTIME_POOLING_TOTAL_CORE_INSTANCES", 100))
            );
            assert_eq!(engine.total_memories(), Some(config.get_total_memories()));
        }
    }

    // An externally-supplied pooling config is accepted and applied.
    #[test]
    fn external_pooling_config_builds() {
        let mut pool = PoolingAllocationConfig::default();
        pool.total_component_instances(32);

        Engine::builder()
            .with_pooling_config(pool)
            .build()
            .expect("external pooling config should build");
    }

    /// Only a count derived against the commit limit is allowed that limit's
    /// share; a later named count or a caller's pooling config replaces it.
    #[test]
    fn only_a_count_derived_to_the_commit_limit_takes_its_share() {
        let precommit =
            host_memory::MaxMemoriesDerivation::default().with_memory_precommit_limit(true);
        let derived = || Engine::builder().with_derived_max_memories(precommit);
        assert!(derived().sized_to_commit_limit());
        assert!(
            !Engine::builder()
                .with_derived_max_memories(host_memory::MaxMemoriesDerivation::default())
                .sized_to_commit_limit()
        );
        assert!(!derived().with_max_memories(64).sized_to_commit_limit());
        assert!(
            !derived()
                .with_pooling_config(PoolingAllocationConfig::default())
                .sized_to_commit_limit()
        );
        assert!(
            !Engine::builder()
                .with_max_memories(64)
                .sized_to_commit_limit()
        );
    }

    /// The budgets an engine reports carry the counts its pool installed,
    /// not the ones its budgets asked for.
    #[test]
    fn host_memory_reports_the_installed_pool_counts() -> anyhow::Result<()> {
        let engine = Engine::builder()
            .with_host_memory(
                host_memory::HostMemoryBudgets::resolve(None, None, None, None)
                    .map_err(anyhow::Error::msg)?,
            )
            .with_max_memories(64)
            .build()?;
        let Some(totals) = engine.pool_totals() else {
            return Ok(());
        };
        let reported = engine.host_memory();
        assert_eq!(reported.max_memories, totals.memories);
        assert_eq!(reported.core_instances, totals.core_instances);
        Ok(())
    }

    /// GC use is read the first time a workload asks and kept with the
    /// compile, so a workload served from the cache, after the last one
    /// holding the compile let go, still has its answer without a second
    /// compile or a second read of the source.
    #[tokio::test]
    async fn a_cached_compile_carries_its_gc_answer() -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;

        let engine = pool_share::test_support::engine(pool_share::test_support::config());
        if engine.pool_totals().is_none() {
            return Ok(());
        }
        let cases = [
            (
                "sha256:numeric",
                "(component (core module (memory 1)))",
                false,
            ),
            (
                "sha256:gc",
                "(component (core module (type (struct))))",
                true,
            ),
        ];
        for (compiles, (digest, wat, uses_gc)) in (1..).zip(cases) {
            let bytes = wat::parse_str(wat)?;
            let (first, live) = engine
                .load_live_component(&bytes, Some(digest), Blocking::Allowed)
                .await?;
            assert_eq!(
                first.may_use_gc.get(),
                None,
                "{digest}: read only on asking"
            );
            assert_eq!(
                engine.compiled_may_use_gc(&first, &bytes),
                uses_gc,
                "{digest}"
            );
            drop((first, live));
            assert!(engine.live_compiles.lock().unwrap().is_empty());

            let (cached, _) = engine
                .load_live_component(&bytes, Some(digest), Blocking::Allowed)
                .await?;
            assert_eq!(engine.compiles.load(Ordering::SeqCst), compiles);
            assert_eq!(cached.may_use_gc.get(), Some(&uses_gc), "{digest}");
        }

        let unpooled = Engine::builder().with_pooling_allocator(false).build()?;
        let gc = wat::parse_str("(component (core module (type (struct))))")?;
        assert!(
            !unpooled.may_use_gc(&gc),
            "an engine with no pool admits nothing"
        );
        Ok(())
    }

    // Proposal names parse case-insensitively and accept `_`/`-` interchangeably;
    // unknown names are rejected.
    #[test]
    fn wasm_proposal_from_str() {
        assert_eq!("gc".parse(), Ok(WasmProposal::Gc));
        assert_eq!("GC".parse(), Ok(WasmProposal::Gc));
        assert_eq!(" threads ".parse(), Ok(WasmProposal::Threads));
        assert_eq!(
            "exception_handling".parse(),
            Ok(WasmProposal::ExceptionHandling)
        );
        assert_eq!(
            "component-model-async".parse(),
            Ok(WasmProposal::ComponentModelAsync)
        );
        assert_eq!(
            "component-model-map".parse(),
            Ok(WasmProposal::ComponentModelMap)
        );
        assert!("nonsense".parse::<WasmProposal>().is_err());
    }

    // A component whose type section uses `map<k, v>` fails validation unless
    // the component-model map proposal is on, so compiling one on a
    // default-built engine is what proves the proposal is enabled by default.
    #[test]
    fn default_engine_accepts_component_model_map() {
        let bytes = wat::parse_str(
            r#"(component
                 (type $m (map string string))
                 (import "test:map/iface" (instance
                   (export "take" (func (param "m" $m)))
                 ))
               )"#,
        )
        .expect("map component should assemble");

        let engine = Engine::builder().build().expect("engine should build");
        Component::new(&engine.inner, &bytes).expect("map component should compile");
    }

    /// A component built against a newer WASI patch than the host links still
    /// resolves, provided it only uses what the host's version defines.
    #[test]
    fn a_newer_compatible_wasi_import_links_against_the_host() {
        let engine = Engine::builder().build().expect("engine should build");
        let mut linker: Linker<SharedCtx> = Linker::new(&engine.inner);
        add_wasi_to_linker(&mut linker).expect("WASI should link");
        let importing = |version: &str| {
            let wat = format!(
                r#"(component (import "wasi:random/random@{version}"
                    (instance (export "get-random-u64" (func (result u64))))))"#
            );
            let bytes = wat::parse_str(wat).expect("component should assemble");
            Component::new(&engine.inner, &bytes).expect("component should compile")
        };

        for version in ["0.2.0", "0.2.99", "0.3.0", "0.3.3"] {
            linker
                .instantiate_pre(&importing(version))
                .unwrap_or_else(|e| panic!("wasi:random@{version} should link: {e:#}"));
        }
        assert!(linker.instantiate_pre(&importing("0.4.0")).is_err());
    }

    /// A digest a running workload holds is never compiled again, even once
    /// the cache has let it go; once nothing holds it, it is forgotten.
    #[tokio::test]
    async fn a_digest_in_use_is_not_recompiled_after_cache_eviction() {
        use std::sync::atomic::Ordering;
        const DIGEST: &str = "sha256:held-image";

        let bytes = wat::parse_str("(component)").expect("component should assemble");
        let engine = Engine::builder().build().expect("engine should build");

        let (_, first) = engine
            .load_live_component(&bytes, Some(DIGEST), Blocking::Allowed)
            .await
            .expect("first load");
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .invalidate_all();
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .run_pending_tasks();
        let (_, second) = engine
            .load_live_component(&bytes, Some(DIGEST), Blocking::Allowed)
            .await
            .expect("second load");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&first, &second));

        drop((first, second));
        assert!(engine.live_compiles.lock().unwrap().is_empty());
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .invalidate_all();
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .run_pending_tasks();
        engine
            .load_live_component(&bytes, Some(DIGEST), Blocking::Allowed)
            .await
            .expect("third load");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 2);
    }

    /// Bytes loaded without a digest compile once and share that compile,
    /// with each other and with a file source's content digest.
    #[tokio::test]
    async fn bytes_without_a_digest_share_one_compile() {
        use std::sync::atomic::Ordering;

        let bytes = wat::parse_str("(component)").expect("component should assemble");
        let engine = Engine::builder().build().expect("engine should build");

        let (_, first) = engine
            .load_live_component(&bytes, None::<&str>, Blocking::Allowed)
            .await
            .expect("first load");
        let (_, second) = engine
            .load_live_component(&bytes, None::<&str>, Blocking::Allowed)
            .await
            .expect("second load");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&first, &second));

        let (_, by_digest) = engine
            .load_live_component(&bytes, Some(content_digest(&bytes)), Blocking::Allowed)
            .await
            .expect("load by content digest");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&first, &by_digest));

        let other = wat::parse_str("(component (core module))").expect("component should assemble");
        engine
            .load_live_component(&other, None::<&str>, Blocking::Allowed)
            .await
            .expect("other load");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 2);
    }

    fn component_workload(digest: Option<&str>, volumes: Vec<crate::types::Volume>) -> Workload {
        let bytes = wat::parse_str("(component)").expect("component should assemble");
        Workload {
            components: vec![crate::types::Component {
                name: "c".into(),
                digest: digest.map(str::to_string),
                bytes: bytes.into(),
                ..Default::default()
            }],
            ..volume_only_workload(volumes)
        }
    }

    /// A workload whose digest is compiled initializes inline, compiling
    /// nothing, and holds its compile: a later inline start of the same digest
    /// is served from it after a cache eviction.
    #[tokio::test]
    async fn a_compiled_workload_initializes_inline_without_compiling() {
        use std::sync::atomic::Ordering;
        const DIGEST: &str = "sha256:pinned-image";

        let engine = Engine::builder().build().expect("engine should build");
        let workload = component_workload(Some(DIGEST), vec![]);
        assert!(matches!(
            engine.initialize_workload_inline("w", workload.clone()),
            InlineInit::WouldBlock { pinned, .. } if pinned.is_empty()
        ));

        let bytes = wat::parse_str("(component)").expect("component should assemble");
        let (_, prepared) = engine
            .load_live_component(&bytes, Some(DIGEST), Blocking::Allowed)
            .await
            .expect("compile");
        let InlineInit::Initialized(first) =
            engine.initialize_workload_inline("w", workload.clone())
        else {
            panic!("a compiled digest initializes inline");
        };
        let first = first.expect("initialize");
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .invalidate_all();
        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .run_pending_tasks();

        let InlineInit::Initialized(second) = engine.initialize_workload_inline("w2", workload)
        else {
            panic!("a live compile initializes inline after an eviction");
        };
        let second = second.expect("initialize");
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
        drop((prepared, first, second));
        assert!(
            engine.live_compiles.lock().unwrap().is_empty(),
            "a pin outlived its holders"
        );
    }

    /// A workload that could compile, open files or build a service is handed
    /// back, holding only compiles that exist, and those go with it.
    #[tokio::test]
    async fn a_workload_that_could_block_is_not_initialized_inline() {
        const DIGEST: &str = "sha256:compiled-image";

        let engine = Engine::builder().build().expect("engine should build");
        let bytes = wat::parse_str("(component)").expect("component should assemble");
        let (_, prepared) = engine
            .load_live_component(&bytes, Some(DIGEST), Blocking::Allowed)
            .await
            .expect("compile");
        let would_block = |workload| {
            matches!(
                engine.initialize_workload_inline("w", workload),
                InlineInit::WouldBlock { .. }
            )
        };

        assert!(would_block(component_workload(None, vec![])));
        assert!(would_block(component_workload(
            Some(DIGEST),
            vec![empty_dir("a")]
        )));
        assert!(would_block(Workload {
            service: Some(crate::types::Service {
                bytes: bytes.clone().into(),
                digest: Some(DIGEST.into()),
                local_resources: Default::default(),
                max_restarts: 0,
            }),
            ..component_workload(Some(DIGEST), vec![])
        }));

        let mixed = Workload {
            components: [Some(DIGEST), Some("sha256:not-compiled")]
                .map(|digest| crate::types::Component {
                    name: "c".into(),
                    digest: digest.map(str::to_string),
                    bytes: bytes.clone().into(),
                    ..Default::default()
                })
                .into(),
            ..volume_only_workload(vec![])
        };
        let InlineInit::WouldBlock { pinned, .. } = engine.initialize_workload_inline("w", mixed)
        else {
            panic!("an uncompiled component is not initialized inline");
        };
        assert_eq!(pinned.len(), 1, "the compiled component is handed on");
        drop((prepared, pinned));
        assert!(
            engine.live_compiles.lock().unwrap().is_empty(),
            "a handed-back pin outlived its holder"
        );
    }

    /// A digest the cache kept after its last workload stopped, as on a scale
    /// from zero, initializes on the calling task. The only blocking thread is
    /// held, so a start that needed the blocking pool would never finish.
    #[test]
    fn a_cached_idle_digest_initializes_without_the_blocking_pool() {
        use std::sync::atomic::Ordering;
        const DIGEST: &str = "sha256:idle-image";

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("runtime should build");
        runtime.block_on(async {
            let engine = Engine::builder().build().expect("engine should build");
            let workload = component_workload(Some(DIGEST), vec![]);
            engine
                .initialize_workload("first", workload.clone())
                .await
                .map(drop)
                .expect("first start");
            assert!(engine.live_compiles.lock().unwrap().is_empty());

            let (release, held) = std::sync::mpsc::channel::<()>();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = held.recv();
            });
            ready.await.expect("the blocking thread is held");

            tokio::time::timeout(
                Duration::from_secs(30),
                engine.initialize_workload("second", workload),
            )
            .await
            .expect("a cached digest should not wait for the blocking pool")
            .map(drop)
            .expect("second start");
            assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);

            drop(release);
            blocker.await.expect("blocker finishes");
        });
    }

    /// Only a host's workload start answers to `max_process_memory`. A plugin
    /// compile and an engine used directly are neither refused nor counted.
    #[tokio::test]
    async fn only_host_starts_answer_to_process_memory_admission() {
        let engine = Engine::builder()
            .with_max_process_memory_bytes(1024 * 1024)
            .build()
            .expect("engine should build");
        let guard = Arc::clone(engine.process_memory().expect("guard is on"));
        assert!(guard.admit_start().is_err(), "a test process is past 1 MiB");
        let refused = guard.refused_starts();

        let bytes = wat::parse_str("(component)").expect("component should assemble");
        engine
            .prepare_host_component(&bytes)
            .await
            .map(drop)
            .expect("a plugin compile is not a workload start");
        engine
            .initialize_workload("direct", component_workload(None, vec![]))
            .await
            .map(drop)
            .expect("direct engine use is not admitted by a host");
        assert_eq!(guard.refused_starts(), refused);

        let Err(err) = engine
            .initialize_admitted_workload(
                "host",
                component_workload(Some("sha256:not-compiled"), vec![]),
            )
            .await
        else {
            panic!("a host start past the limit should be refused");
        };
        assert!(format!("{err:#}").contains("max-process-memory"), "{err:#}");
        assert_eq!(guard.refused_starts(), refused + 1);
    }

    /// A cache hit can arrive after pressure has changed since the host admitted
    /// the start. Even initialization that needs no blocking work must recheck.
    #[tokio::test]
    async fn a_cached_start_rechecks_memory_after_awaiting_the_backend() -> anyhow::Result<()> {
        struct PressureOnHit {
            cache: DefaultCache,
            guard: Arc<process_memory::ProcessMemoryGuard>,
        }
        #[async_trait::async_trait]
        impl Cache for PressureOnHit {
            async fn get_compiled(&self, key: &CacheKey) -> anyhow::Result<Option<CacheValue>> {
                let cached = self.cache.get_compiled(key).await?;
                if cached.is_some() {
                    tokio::task::yield_now().await;
                    self.guard.observe(2048);
                }
                Ok(cached)
            }
            async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> anyhow::Result<()> {
                self.cache.put_compiled(key, value).await
            }
        }

        // This guard is not registered with the OS sampler: pressure changes
        // deterministically when the backend returns, without allocating RAM.
        let guard = Arc::new(process_memory::ProcessMemoryGuard::new(1024));
        let mut engine = Engine::builder()
            .with_pooling_allocator(false)
            .without_max_process_memory()
            .with_cache(Arc::new(PressureOnHit {
                cache: DefaultCache::default(),
                guard: Arc::clone(&guard),
            }))
            .build()?;
        let workload = component_workload(Some("sha256:pressure-on-hit"), vec![]);
        drop(engine.initialize_workload("warm", workload.clone()).await?);
        assert!(engine.live_compiles.lock().unwrap().is_empty());
        engine.process_memory = Some(Arc::clone(&guard));
        guard.observe(0);
        guard
            .admit_start()
            .expect("the host initially admits this start");

        let result = engine
            .initialize_admitted_workload("host", workload.clone())
            .await;
        assert!(guard.is_over(), "the cache lookup raised memory pressure");
        let Err(error) = result else {
            panic!("a cached start must recheck memory admission");
        };
        assert!(
            format!("{error:#}").contains("max-process-memory"),
            "{error:#}"
        );
        assert_eq!(guard.refused_starts(), 1);

        // Direct engine use remains outside host start admission.
        drop(engine.initialize_workload("direct", workload).await?);
        assert_eq!(guard.refused_starts(), 1);
        Ok(())
    }

    /// A retry can miss while a cancelled start's detached publication is
    /// pending, then obtain the compile flight only after that publication ends.
    #[tokio::test]
    async fn a_retry_reuses_a_compile_published_after_its_cache_miss() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::Semaphore;

        struct PausedCache {
            cache: DefaultCache,
            pause_write: AtomicBool,
            writing: Semaphore,
            publish: Semaphore,
            pause_read: AtomicBool,
            reading: Semaphore,
            finish_read: Semaphore,
        }
        #[async_trait::async_trait]
        impl Cache for PausedCache {
            async fn get_compiled(&self, key: &CacheKey) -> anyhow::Result<Option<CacheValue>> {
                let snapshot = self.cache.get_compiled(key).await?;
                if self.pause_read.swap(false, Ordering::SeqCst) {
                    assert!(snapshot.is_none(), "the retry reads before publication");
                    self.reading.add_permits(1);
                    self.finish_read.acquire().await?.forget();
                }
                Ok(snapshot)
            }
            async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> anyhow::Result<()> {
                if self.pause_write.swap(false, Ordering::SeqCst) {
                    self.writing.add_permits(1);
                    self.publish.acquire().await?.forget();
                }
                self.cache.put_compiled(key, value).await
            }
        }

        let cache = Arc::new(PausedCache {
            cache: DefaultCache::default(),
            pause_write: AtomicBool::new(true),
            writing: Semaphore::new(0),
            publish: Semaphore::new(0),
            pause_read: AtomicBool::new(false),
            reading: Semaphore::new(0),
            finish_read: Semaphore::new(0),
        });
        let engine = Engine::builder()
            .with_pooling_allocator(false)
            .without_max_process_memory()
            .with_cache(cache.clone())
            .build()?;
        let workload = component_workload(Some("sha256:publication-race"), vec![]);
        let first = tokio::spawn({
            let engine = engine.clone();
            let workload = workload.clone();
            async move { engine.initialize_workload("cancelled", workload).await }
        });
        tokio::time::timeout(Duration::from_secs(30), cache.writing.acquire())
            .await??
            .forget();
        first.abort();
        assert!(matches!(first.await, Err(error) if error.is_cancelled()));

        cache.pause_read.store(true, Ordering::SeqCst);
        let retry = tokio::spawn({
            let engine = engine.clone();
            async move { engine.initialize_workload("retry", workload).await }
        });
        tokio::time::timeout(Duration::from_secs(30), cache.reading.acquire())
            .await??
            .forget();
        cache.publish.add_permits(1);
        tokio::time::timeout(Duration::from_secs(30), async {
            while engine.compiling.in_flight_count() != 0
                || !engine.live_compiles.lock().unwrap().is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
        cache.finish_read.add_permits(1);
        let retried = tokio::time::timeout(Duration::from_secs(30), retry).await???;
        assert_eq!(
            engine.compiles.load(Ordering::SeqCst),
            1,
            "the published compile must be reused"
        );
        drop(retried);
        Ok(())
    }

    /// Initialization with blocking forbidden refuses every step that blocks,
    /// so a workload let through inline by mistake fails its start rather
    /// than stall an async worker.
    #[test]
    fn forbidden_blocking_refuses_each_blocking_step() {
        use std::sync::atomic::Ordering;

        let engine = Engine::builder().build().expect("engine should build");
        for (workload, step) in [
            (component_workload(None, vec![]), "hash a component"),
            (
                component_workload(Some("sha256:not-compiled"), vec![]),
                "compile a component",
            ),
            (volume_only_workload(vec![empty_dir("a")]), "open a volume"),
        ] {
            let err = engine
                .initialize_workload_with("w", workload, Blocking::Forbidden)
                .err()
                .expect("a blocking step is refused");
            assert!(
                format!("{err:#}").contains(step),
                "expected a refusal to {step}, got: {err:#}"
            );
        }
        assert_eq!(engine.compiles.load(Ordering::SeqCst), 0);
    }

    // A scheduler placing N replicas of one image sends N starts carrying one
    // digest, and the herd is affordable only because they share a compile.
    // Keying the cache on anything that differs between replicas — a workload
    // id, a component name — would put a Cranelift compile behind every one of
    // them, and nothing in a deployment's behaviour would say so.
    #[tokio::test]
    async fn a_herd_of_replicas_shares_one_compiled_component() {
        const HERD: usize = 15;
        const DIGEST: &str = "sha256:one-image";

        let bytes = wat::parse_str("(component)").expect("component should assemble");
        let engine = Engine::builder().build().expect("engine should build");

        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..HERD {
            let engine = engine.clone();
            let bytes = bytes.clone();
            tasks.spawn(async move { engine.load_component_bytes(&bytes, Some(DIGEST)).await });
        }
        while let Some(result) = tasks.join_next().await {
            result
                .expect("task finishes")
                .expect("every member of the herd should load");
        }

        // The claim is that the herd shares a compile, not merely that it ends
        // up with one entry: a loader that compiled per caller and let them
        // race to insert the same key would leave one entry too, and this test
        // would have said nothing about the cost it exists to prevent.
        assert_eq!(
            engine.compiles.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{HERD} replicas of one image ran more than one compile"
        );

        engine
            .default_cache
            .as_ref()
            .unwrap()
            .compiled
            .run_pending_tasks();
        assert_eq!(
            engine
                .default_cache
                .as_ref()
                .unwrap()
                .compiled
                .entry_count(),
            1,
            "{HERD} replicas of one image left more than one cache entry"
        );

        // A hit is served without looking at the bytes, so bytes that could
        // never compile are what prove the entry came back from the cache
        // rather than from a compile of its own.
        engine
            .load_component_bytes(b"definitely not a wasm component", Some(DIGEST))
            .await
            .map(|_| ())
            .expect("a digest the herd already compiled should be served from the cache");
    }

    // A compile failure that goes through the cache reports everything the
    // uncached path reports. `try_get_with` returns its error as
    // `Arc<anyhow::Error>`, which is easy to collapse into its outermost message
    // alone — leaving logs that say a component failed to compile and never why.
    #[tokio::test]
    async fn cached_compile_failure_keeps_the_cause_chain() {
        let engine = Engine::builder().build().expect("engine should build");
        let garbage = b"definitely not a wasm component";

        // `Component` is not `Debug`, so `expect_err` is unavailable here.
        let Err(uncached) = engine.load_component_bytes(garbage, None::<&str>).await else {
            panic!("garbage bytes should not compile");
        };
        let Err(cached) = engine
            .load_component_bytes(garbage, Some("sha256:not-a-real-digest"))
            .await
        else {
            panic!("garbage bytes should not compile");
        };

        let kept: Vec<String> = cached.chain().map(|c| c.to_string()).collect();
        for cause in uncached.chain() {
            let cause = cause.to_string();
            assert!(
                kept.contains(&cause),
                "cached path dropped {cause:?}; it kept only {kept:?}"
            );
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod contract_tests;
