//! Valgrind/cachegrind instruction counts for host component plugins.
//!
//! Instruction counts rather than wall-clock, because wall-clock cannot resolve
//! what this measures. A criterion version of these measurements was written and
//! discarded: driven over HTTP, a plugin-backed request and a plugin-free one
//! land inside each other's confidence intervals, so the cross-store hop is
//! smaller than that harness's noise floor — and its between-run variance was
//! wide enough to produce a confident-looking result that did not reproduce.
//! Cachegrind counts are deterministic, so the deltas below are exactly the
//! costs timing could not see.
//!
//! Three measurements, covering the plugin's whole surface:
//!
//! * `capability_call` — one HTTP request per arm against the same warm host,
//!   so differencing any arm against `no_plugin` isolates one mechanism and
//!   differencing it against `get` isolates that mechanism's cost above a plain
//!   call. The arms walk the relocation surface a capability call can use:
//!   handle-free primitives (`get`), a cross-store `own<T>` proxy with borrow
//!   methods and a drop (`resource`), streams in both directions (`stream_in`,
//!   `stream_out`), a future (`future`), and the plugin's self-import taking a
//!   re-entrant hop back into its own store (`reentrant`). Each of those is a
//!   distinct path through `relocate` and the resource bridge, not a variation
//!   in payload size.
//! * `lifecycle_bind` / `lifecycle_unbind` — the
//!   `wasmcloud:host/workload-lifecycle` hooks, driven directly against a
//!   started plugin. They are public `HostPlugin` methods, so no host, HTTP
//!   server, or `workload_start` is in the window; these are absolute numbers,
//!   not a pair, with nothing to subtract off.
//! * `plugin_start` — bringing one incarnation up: build the store, instantiate
//!   from the pre-compiled component, spawn the supervisor. Compilation sits in
//!   setup deliberately, because a supervised restart reuses the `InstancePre`
//!   and pays only what is measured here.
//!
//! `plugin_start` and `lifecycle_bind` together are the restart story: a
//! restart costs one `plugin_start` plus one `lifecycle_bind` per bound
//! workload, replayed serially, during which no capability call is served
//! (`host/trigger_service/mod.rs`).
//!
//! Requires `valgrind` and the `gungraun-runner` binary, version-locked to the
//! `gungraun` dependency. Valgrind does not support Apple Silicon, so this runs
//! on the Linux bench host, not on a Mac laptop:
//!
//! ```text
//! cargo install gungraun-runner --version 0.19.1
//! cargo bench -p wash-runtime --features host-component-plugins \
//!   --bench gungraun_plugin
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(not(feature = "host-component-plugins"))]
fn main() {
    eprintln!(
        "gungraun_plugin bench requires the `host-component-plugins` feature:\n  \
         cargo bench -p wash-runtime --features host-component-plugins --bench gungraun_plugin"
    );
}

#[cfg(feature = "host-component-plugins")]
use std::{
    collections::{HashMap, HashSet},
    hint::black_box,
    net::SocketAddr,
    sync::Arc,
};

#[cfg(feature = "host-component-plugins")]
use gungraun::{library_benchmark, library_benchmark_group, main};
#[cfg(feature = "host-component-plugins")]
use tokio::runtime::Runtime;

#[cfg(feature = "host-component-plugins")]
use wash_runtime::{
    engine::Engine,
    engine::workload::{UnresolvedWorkload, WorkloadComponent},
    host::{
        Host, HostApi, HostBuilder,
        http::{DynamicRouter, Ingress},
    },
    plugin::component_host::ComponentHostPlugin,
    plugin::{HostPlugin, WitInterfaces},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadStopRequest},
    wit::WitInterface,
};

#[cfg(feature = "host-component-plugins")]
const KV_PLUGIN_WASM: &[u8] = include_bytes!("../tests/wasm/kv_plugin.wasm");
#[cfg(feature = "host-component-plugins")]
const KV_PLUGIN_CALLER_WASM: &[u8] = include_bytes!("../tests/wasm/kv_plugin_caller.wasm");
#[cfg(feature = "host-component-plugins")]
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("../tests/wasm/http_handler_p3.wasm");

#[cfg(feature = "host-component-plugins")]
const PLUGIN_ID: &str = "acme-kv-plugin";
/// Key seeded at setup and read back by the `get` arm.
#[cfg(feature = "host-component-plugins")]
const HOT_KEY: &str = "bench";
/// Bytes moved by the stream arms. Large enough that the stream machinery
/// dominates a single chunk, small enough to stay a mechanism measurement
/// rather than a payload-size one.
#[cfg(feature = "host-component-plugins")]
const STREAM_BYTES: u64 = 4096;
/// One self-import hop for the re-entrancy arm — a hop measurement, not a
/// depth-limit test.
#[cfg(feature = "host-component-plugins")]
const RECURSE_DEPTH: u64 = 1;
/// Workload id used by the lifecycle-hook measurements. They drive the hooks
/// directly, so nothing else needs to agree on it.
#[cfg(feature = "host-component-plugins")]
const LIFECYCLE_WORKLOAD_ID: &str = "bench-lifecycle-workload";

/// The bespoke capability the plugin exports and a workload matches on.
#[cfg(feature = "host-component-plugins")]
fn acme_kv_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "kv".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// One shape of capability call, exercised over the same HTTP harness so the
/// arms are directly comparable. Every arm but [`Call::NoPlugin`] runs the same
/// `kv-plugin-caller` workload and differs only in which route it hits, so the
/// delta between two arms is the delta between two mechanisms.
#[cfg(feature = "host-component-plugins")]
#[derive(Copy, Clone, Debug)]
pub enum Call {
    /// `http-handler-p3`: no capability import at all. The baseline every other
    /// arm is differenced against — it is the HTTP and component-invoke cost
    /// with no plugin involvement.
    NoPlugin,
    /// One handle-free `get`: primitives across the store boundary, the
    /// relocation fast path.
    Get,
    /// A cross-store `own<bucket>` proxy — open it, `set` and `get` through
    /// borrow methods, then drop it so the real resource is freed in the plugin
    /// store. Exercises the resource bridge and the drop flush, which steps out
    /// of `run_concurrent` to run the guest destructor.
    Resource,
    /// caller -> plugin `stream<u8>`: the plugin totals the bytes it is fed.
    StreamIn,
    /// plugin -> caller `stream<u8>`: the caller drains what the plugin emits.
    StreamOut,
    /// plugin -> caller `future<u64>`, resolved across the boundary.
    Future,
    /// The plugin's self-import: it calls its own capability, so the call
    /// re-enters the store it is already running in.
    Reentrant,
}

#[cfg(feature = "host-component-plugins")]
impl Call {
    /// Whether this arm's workload imports the plugin capability.
    fn is_plugin(self) -> bool {
        !matches!(self, Call::NoPlugin)
    }

    fn host_header(self) -> &'static str {
        if self.is_plugin() {
            "bench-plugin"
        } else {
            "bench-no-plugin"
        }
    }

    fn component_name(self) -> &'static str {
        if self.is_plugin() {
            "kv-plugin-caller"
        } else {
            "http-handler-p3"
        }
    }

    fn wasm(self) -> &'static [u8] {
        if self.is_plugin() {
            KV_PLUGIN_CALLER_WASM
        } else {
            HTTP_HANDLER_P3_WASM
        }
    }

    /// The route this arm drives per request.
    fn path(self) -> String {
        match self {
            Call::NoPlugin => "/".to_string(),
            Call::Get => format!("/get?key={HOT_KEY}"),
            Call::Resource => "/bucket?name=bench&key=k&value=v".to_string(),
            Call::StreamIn => format!("/total?count={STREAM_BYTES}"),
            Call::StreamOut => format!("/emit?count={STREAM_BYTES}"),
            Call::Future => "/eventually?value=7".to_string(),
            Call::Reentrant => format!("/recurse?n={RECURSE_DEPTH}"),
        }
    }

    fn host_interfaces(self) -> Vec<WitInterface> {
        let mut config = HashMap::new();
        config.insert("host".to_string(), self.host_header().to_string());
        let http = WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config,
            name: None,
        };
        if self.is_plugin() {
            vec![http, acme_kv_interface()]
        } else {
            vec![http]
        }
    }

    fn workload_request(self) -> WorkloadStartRequest {
        WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "bench".to_string(),
                name: self.host_header().to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![Component {
                    name: self.component_name().to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(self.wasm()),
                    local_resources: LocalResources::default(),
                    pool_size: 0,
                    max_invocations: 0,
                }],
                host_interfaces: self.host_interfaces(),
                volumes: vec![],
            },
        }
    }
}

/// A started plugin-equipped host and the runtime that owns it. The host must
/// be dropped inside its runtime, so the two travel together.
#[cfg(feature = "host-component-plugins")]
pub struct Warm {
    rt: Runtime,
    host: Arc<Host>,
    workload_ids: Vec<String>,
    addr: SocketAddr,
    client: reqwest::Client,
    call: Call,
}

/// Build a host with the component plugin loaded but no workloads started.
/// Used by `setup_host` below; kept separate so host construction stays out of
/// the measured window.
#[cfg(feature = "host-component-plugins")]
fn setup_bare_host(call: Call) -> Warm {
    let rt = Runtime::new().expect("tokio runtime");
    let (host, addr, client) = rt.block_on(async {
        let engine = Engine::builder().build().expect("engine");
        let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let addr = ingress.addr();
        let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine.clone())
            .expect("failed to build host component plugin");
        let host = HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(ingress))
            .with_plugin(Arc::new(plugin))
            .unwrap()
            .build()
            .unwrap();
        let host = host.start().await.unwrap();
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(16)
            .tcp_nodelay(true)
            .build()
            .unwrap();
        (host, addr, client)
    });

    Warm {
        rt,
        host,
        workload_ids: Vec::new(),
        addr,
        client,
        call,
    }
}

/// Bare host plus this arm's workload, warmed by taking the arm's own route
/// once so no first-instantiation cost lands inside the measurement window.
#[cfg(feature = "host-component-plugins")]
fn setup_host(call: Call) -> Warm {
    let mut warm = setup_bare_host(call);
    let addr = warm.addr;
    let ids = warm.rt.block_on(async {
        let req = call.workload_request();
        let id = req.workload_id.clone();
        warm.host.workload_start(req).await.unwrap();

        // Seed the key the `get` arm reads back, so it measures a hit rather
        // than the miss path. Harmless for the other plugin arms, and it warms
        // the capability path itself.
        if call.is_plugin() {
            let seed = warm
                .client
                .get(format!("http://{addr}/set?key={HOT_KEY}&value=v"))
                .header("HOST", call.host_header())
                .send()
                .await
                .unwrap();
            assert!(
                seed.status().is_success(),
                "seeding failed: {}",
                seed.status()
            );
        }

        let warmup = warm
            .client
            .get(format!("http://{addr}{}", call.path()))
            .header("HOST", call.host_header())
            .send()
            .await
            .unwrap();
        assert!(
            warmup.status().is_success(),
            "warmup failed for {call:?}: {}",
            warmup.status()
        );
        let _ = warmup.bytes().await.unwrap();
        vec![id]
    });
    warm.workload_ids = ids;
    warm
}

/// Stop workloads and the host outside the measurement window. Unlike the
/// plugin-free benches, dropping is not enough here: the plugin's supervisor
/// and trigger-service tasks are torn down by `Host::stop`, and its
/// per-workload unbind runs on `workload_stop`.
#[cfg(feature = "host-component-plugins")]
fn drop_warm(warm: Warm) {
    let Warm {
        rt,
        host,
        workload_ids,
        ..
    } = warm;
    rt.block_on(async {
        for workload_id in workload_ids {
            let _ = host
                .workload_stop(WorkloadStopRequest { workload_id })
                .await;
        }
        let _ = host.stop().await;
    });
}

// One HTTP request against a warm host, once per arm. `no_plugin` makes no
// capability call at all; every other arm makes one of a different shape, so an
// arm minus `no_plugin` is that mechanism end to end and an arm minus `get` is
// what the mechanism costs above a plain handle-free call.
//
// Doc comments are not allowed on a `#[library_benchmark]` fn — the macro
// accepts only `bench`/`benches` attributes — so this stays a line comment.
#[cfg(feature = "host-component-plugins")]
#[library_benchmark]
#[bench::no_plugin(args = (Call::NoPlugin), setup = setup_host, teardown = drop_warm)]
#[bench::get(args = (Call::Get), setup = setup_host, teardown = drop_warm)]
#[bench::resource(args = (Call::Resource), setup = setup_host, teardown = drop_warm)]
#[bench::stream_in(args = (Call::StreamIn), setup = setup_host, teardown = drop_warm)]
#[bench::stream_out(args = (Call::StreamOut), setup = setup_host, teardown = drop_warm)]
#[bench::future(args = (Call::Future), setup = setup_host, teardown = drop_warm)]
#[bench::reentrant(args = (Call::Reentrant), setup = setup_host, teardown = drop_warm)]
fn capability_call(warm: Warm) -> Warm {
    let url = format!("http://{}{}", warm.addr, warm.call.path());
    warm.rt.block_on(async {
        let resp = warm
            .client
            .get(&url)
            .header("HOST", warm.call.host_header())
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "non-2xx: {}", resp.status());
        let _ = black_box(resp.bytes().await.unwrap());
    });
    warm
}

/// A started plugin and the arguments for one lifecycle hook call — no host,
/// no HTTP server, no workload deploy. `on-workload-bind` and
/// `on-workload-unbind` are public `HostPlugin` methods, so the hooks can be
/// driven directly and measured with nothing else in the window.
#[cfg(feature = "host-component-plugins")]
pub struct Lifecycle {
    rt: Runtime,
    plugin: ComponentHostPlugin,
    workload: UnresolvedWorkload,
    interfaces: HashSet<WitInterface>,
}

/// Start a plugin and build one workload's bind arguments. When `bind_first`,
/// the bind is also delivered here — untimed — so an unbind measurement has
/// something to reclaim rather than hitting the not-bound early return.
#[cfg(feature = "host-component-plugins")]
fn setup_lifecycle(bind_first: bool) -> Lifecycle {
    let rt = Runtime::new().expect("tokio runtime");
    let engine = Engine::builder().build().expect("engine");
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine)
        .expect("failed to build host component plugin");
    let workload = UnresolvedWorkload::new(
        LIFECYCLE_WORKLOAD_ID,
        "bench",
        "bench",
        None,
        std::iter::empty::<WorkloadComponent>(),
        Vec::new(),
    );
    let interfaces: HashSet<WitInterface> = [acme_kv_interface()].into_iter().collect();

    rt.block_on(async {
        plugin.start().await.expect("failed to start plugin");
        if bind_first {
            plugin
                .on_workload_bind(&workload, WitInterfaces::new(&interfaces))
                .await
                .expect("setup bind failed");
        }
    });

    Lifecycle {
        rt,
        plugin,
        workload,
        interfaces,
    }
}

#[cfg(feature = "host-component-plugins")]
fn drop_lifecycle(lc: Lifecycle) {
    lc.rt.block_on(async {
        let _ = lc.plugin.stop().await;
    });
}

// One `on-workload-bind` delivered to the plugin: build the `workload-info`
// record, cross the store boundary, run the guest hook, decode the reply.
// Nothing else is in the window — no component instantiation, no HTTP, no
// `workload_start` — so this is the bind cost itself rather than an upper
// bound on it.
//
// This is also the per-workload cost of lifecycle REPLAY, which is what makes
// it worth tracking: replay re-invokes exactly this hook for every bound
// workload, serially, and gates all capability calls until the last one
// finishes (`host/trigger_service/mod.rs`). N x this number, plus one
// `plugin_start`, is the restart stall.
#[cfg(feature = "host-component-plugins")]
#[library_benchmark]
#[bench::bind(args = (false), setup = setup_lifecycle, teardown = drop_lifecycle)]
fn lifecycle_bind(lc: Lifecycle) -> Lifecycle {
    {
        let Lifecycle {
            rt,
            plugin,
            workload,
            interfaces,
        } = &lc;
        rt.block_on(async {
            black_box(
                plugin
                    .on_workload_bind(workload, WitInterfaces::new(interfaces))
                    .await,
            )
            .expect("bind failed");
        });
    }
    lc
}

// The unbind half of the same contract, against a workload bound during setup.
#[cfg(feature = "host-component-plugins")]
#[library_benchmark]
#[bench::unbind(args = (true), setup = setup_lifecycle, teardown = drop_lifecycle)]
fn lifecycle_unbind(lc: Lifecycle) -> Lifecycle {
    {
        let Lifecycle {
            rt,
            plugin,
            interfaces,
            ..
        } = &lc;
        rt.block_on(async {
            black_box(
                plugin
                    .on_workload_unbind(LIFECYCLE_WORKLOAD_ID, WitInterfaces::new(interfaces))
                    .await,
            )
            .expect("unbind failed");
        });
    }
    lc
}

/// A built-but-not-started plugin, for measuring what bringing an incarnation
/// up costs.
#[cfg(feature = "host-component-plugins")]
pub struct Stopped {
    rt: Runtime,
    plugin: ComponentHostPlugin,
}

/// Build the plugin — which compiles and introspects the component — outside
/// the measured window. A supervised restart reuses that work, so counting it
/// here would overstate what a restart actually pays.
#[cfg(feature = "host-component-plugins")]
fn setup_stopped_plugin(_unused: bool) -> Stopped {
    let rt = Runtime::new().expect("tokio runtime");
    let engine = Engine::builder().build().expect("engine");
    let plugin = ComponentHostPlugin::new(PLUGIN_ID, KV_PLUGIN_WASM, engine)
        .expect("failed to build host component plugin");
    Stopped { rt, plugin }
}

#[cfg(feature = "host-component-plugins")]
fn drop_stopped(stopped: Stopped) {
    stopped.rt.block_on(async {
        let _ = stopped.plugin.stop().await;
    });
}

// Bringing one plugin incarnation up: build the store, instantiate the
// component from the pre-compiled `InstancePre`, publish the capability
// channel, spawn the supervisor and its trigger-service driver. This is the
// fixed half of a supervised restart; `lifecycle_bind` is the per-workload
// half.
#[cfg(feature = "host-component-plugins")]
#[library_benchmark]
#[bench::start(args = (false), setup = setup_stopped_plugin, teardown = drop_stopped)]
fn plugin_start(stopped: Stopped) -> Stopped {
    {
        let Stopped { rt, plugin } = &stopped;
        rt.block_on(async {
            black_box(plugin.start().await).expect("plugin start failed");
        });
    }
    stopped
}

#[cfg(feature = "host-component-plugins")]
library_benchmark_group!(
    name = plugin;
    benchmarks = capability_call, lifecycle_bind, lifecycle_unbind, plugin_start
);

#[cfg(feature = "host-component-plugins")]
main!(library_benchmark_groups = plugin);
