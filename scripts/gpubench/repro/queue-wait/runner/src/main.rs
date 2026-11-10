// A bare wasmtime host for wasip3 commands that import wasi:webgpu, with no
// wasmCloud in the way. Adapted from Mendy Berger's wasi-bench-runtime.

use std::sync::Arc;

use clap::Parser;
use wasi_webgpu_wasmtime::reexports::{wgpu_core, wgpu_types};
use wasmtime::{
    component::{Component, Linker},
    format_err, Config, Engine, Result, Store,
};
use wasmtime_wasi::{
    FsPerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
};

#[derive(Parser, Debug)]
#[command(version, about = "Run a llama.cpp wasip3 bench component", long_about = None)]
struct Args {
    /// Component to run, e.g. build-wasip3-gpu/bin/llama-bench
    component: String,

    /// Directory to preopen, as HOST[::GUEST]. Repeatable. Defaults to ".".
    #[arg(long = "dir", value_name = "HOST[::GUEST]")]
    dirs: Vec<String>,

    /// Byte budget for guest-to-host copies. wasmtime defaults to 128 MiB, which
    /// model weight uploads through wasi:webgpu exhaust.
    #[arg(long, default_value_t = 8 << 30)]
    hostcall_fuel: usize,

    /// Arguments passed through to the component.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    guest_args: Vec<String>,
}

struct BenchState {
    table: ResourceTable,
    ctx: WasiCtx,
    instance: Arc<wgpu_core::global::Global>,
    options: wasi_webgpu_wasmtime::WasiWebGpuOptions,
}

impl WasiView for BenchState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

impl wasi_webgpu_wasmtime::WasiWebGpuCtxView for BenchState {
    fn webgpu_ctx(&mut self) -> wasi_webgpu_wasmtime::WasiWebGpuCtx<'_> {
        wasi_webgpu_wasmtime::WasiWebGpuCtx {
            table: &mut self.table,
            instance: &self.instance,
            options: &self.options,
        }
    }
}

fn wgpu_instance() -> wgpu_core::global::Global {
    wgpu_core::global::Global::new(
        "webgpu",
        wgpu_types::InstanceDescriptor {
            backends: wgpu_types::Backends::all(),
            flags: wgpu_types::InstanceFlags::from_build_config(),
            backend_options: wgpu_types::BackendOptions::default(),
            memory_budget_thresholds: wgpu_types::MemoryBudgetThresholds::default(),
            display: None,
        },
        None,
    )
}

fn split_dir(spec: &str) -> (&str, &str) {
    match spec.split_once("::") {
        Some((host, guest)) => (host, guest),
        None => (spec, spec),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut config = Config::default();
    config.wasm_component_model(true);
    // llama.cpp's wasi builds use the wasm EH proposal (-fwasm-exceptions).
    config.wasm_exceptions(true);
    config.consume_fuel(false);
    let engine = Engine::new(&config)?;

    let mut linker: Linker<BenchState> = Linker::new(&engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasi_webgpu_wasmtime::add_to_linker(&mut linker)?;

    let mut builder = WasiCtxBuilder::new();
    let dirs = if args.dirs.is_empty() {
        vec![".".to_string()]
    } else {
        args.dirs.clone()
    };
    for spec in &dirs {
        let (host, guest) = split_dir(spec);
        builder
            .preopened_dir(host, guest, FsPerms::ReadWrite)
            .map_err(|e| format_err!("preopening {host}: {e}"))?;
    }

    // argv[0] is the program name, the rest are the component's own arguments.
    let mut argv = vec![args.component.clone()];
    argv.extend(args.guest_args.iter().cloned());
    builder.args(&argv).inherit_stdio();

    let gpu = Arc::new(wgpu_instance());
    let state = BenchState {
        table: ResourceTable::new(),
        ctx: builder.build(),
        instance: gpu,
        options: Default::default(),
    };

    let mut store = Store::new(&engine, state);
    store.set_hostcall_fuel(args.hostcall_fuel);

    let component = Component::from_file(&engine, &args.component)
        .map_err(|e| format_err!("loading {}: {e}", args.component))?;

    let command =
        wasmtime_wasi::p3::bindings::Command::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| format_err!("instantiating wasi:cli/command: {e}"))?;

    // wasi:cli/run is lifted sync, so the guest blocks in wgpuInstanceWaitAny to
    // drive its async webgpu imports to completion.
    let result = store
        .run_concurrent(async move |store| command.wasi_cli_run().call_run(store).await)
        .await??;

    match result {
        Ok(()) => Ok(()),
        Err(()) => std::process::exit(1),
    }
}
