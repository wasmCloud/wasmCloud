use wasmtime_wasi::{WasiCtx, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

pub mod subcommand;

/// The directory where plugins are stored.
pub const PLUGIN_DIR: &str = "plugins";

struct Data {
    table: wasmtime::component::ResourceTable,
    ctx: WasiCtx,
    http: WasiHttpCtx,
}

impl WasiView for Data {
    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}

impl WasiHttpView for Data {
    fn table(&mut self) -> &mut wasmtime::component::ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}
