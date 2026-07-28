use super::WasiSocketsCtxView;
use super::network::Network;
use std::sync::Arc;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::instance_network;

type UpstreamNetwork = wasmtime_wasi::p2::Network;

impl instance_network::Host for WasiSocketsCtxView<'_> {
    fn instance_network(&mut self) -> wasmtime::Result<Resource<UpstreamNetwork>> {
        let network = Network {
            socket_addr_check: self.ctx.socket_addr_check.clone(),
            allowed_ip_names: Arc::clone(&self.ctx.allowed_ip_names),
        };
        let network = self.table.push(network)?;
        Ok(Resource::new_own(network.rep()))
    }
}
