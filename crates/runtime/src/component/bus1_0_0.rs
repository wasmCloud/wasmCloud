//! Compatibility implementation of the `wasmcloud:bus/lattice@1.0.0` interface
use super::{Ctx, Handler, TableResult};

use crate::capability::bus1_0_0::lattice;

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::Resource;

#[async_trait]
/// `wasmcloud:bus/lattice@1.0.0` implementation
pub trait Bus {
    /// Set the link name to use for a given list of interfaces
    async fn set_link_name(&self, link_name: String, interfaces: Vec<Arc<CallTargetInterface>>);
}

#[async_trait]
impl<H: Handler> lattice::Host for Ctx<H> {
    async fn set_link_name(
        &mut self,
        link_name: String,
        interfaces: Vec<Resource<Arc<CallTargetInterface>>>,
    ) -> anyhow::Result<()> {
        let interfaces = interfaces
            .into_iter()
            .map(|interface| self.table.get(&interface).cloned())
            .collect::<TableResult<_>>()
            .context("failed to convert call target interfaces")?;
        // NOTE: We're try-unwrapping the outer error, the inner Result should be ignored.
        let _ = self.handler.set_link_name(link_name, interfaces).await?;
        Ok(())
    }
}

#[async_trait]
impl<H: Handler> lattice::HostCallTargetInterface for Ctx<H> {
    async fn new(
        &mut self,
        namespace: String,
        package: String,
        interface: String,
    ) -> anyhow::Result<Resource<Arc<CallTargetInterface>>> {
        self.table
            .push(Arc::new(CallTargetInterface {
                namespace,
                package,
                interface,
            }))
            .context("failed to push target interface")
    }

    async fn drop(&mut self, interface: Resource<Arc<CallTargetInterface>>) -> anyhow::Result<()> {
        self.table.delete(interface)?;
        Ok(())
    }
}
