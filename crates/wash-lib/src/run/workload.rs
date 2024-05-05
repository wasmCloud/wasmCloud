use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiView};
use crate::run::ctx::Ctx;

/// A runnable workload with an attached [`State`].
pub struct Workload;

/// State associated with a workload.
///
/// It stores resources that can be handled by the associated workload,
/// as well as the host access rules represented by a [`Ctx`].
struct State {
    table: ResourceTable,
    ctx: Ctx,
}

impl WasiView for State {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx.wasi_ctx
    }
}
