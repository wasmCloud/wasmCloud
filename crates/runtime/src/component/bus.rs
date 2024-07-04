use super::{Ctx, Handler, TableResult};

use crate::capability::bus::lattice;

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::Resource;

#[async_trait]
/// `wasmcloud:bus/host` implementation
pub trait Bus {
    /// Set link name
    async fn set_link_name(
        &self,
        target: String,
        interfaces: Vec<Arc<CallTargetInterface>>,
    ) -> anyhow::Result<()>;
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
        self.handler
            .set_link_name(link_name, interfaces)
            .await
            .context("failed to set link name")?;
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

    fn drop(&mut self, interface: Resource<Arc<CallTargetInterface>>) -> anyhow::Result<()> {
        self.table.delete(interface)?;
        Ok(())
    }
}
