use super::{Ctx, Handler, TableResult};

use crate::capability::bus::lattice;

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::Resource;

#[async_trait]
/// `wasmcloud:bus/lattice` implementation
pub trait Bus {
    /// Set the link name to use for a given list of interfaces, returning an error
    /// if a link doesn't exist on the given interfaces for the given target
    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<Arc<CallTargetInterface>>,
    ) -> anyhow::Result<std::result::Result<(), String>>;
}

#[async_trait]
impl<H: Handler> lattice::Host for Ctx<H> {
    async fn set_link_name(
        &mut self,
        link_name: String,
        interfaces: Vec<Resource<Arc<CallTargetInterface>>>,
    ) -> anyhow::Result<std::result::Result<(), String>> {
        let interfaces = interfaces
            .into_iter()
            .map(|interface| self.table.get(&interface).cloned())
            .collect::<TableResult<_>>()
            .context("failed to convert call target interfaces")?;
        self.handler
            .set_link_name(link_name, interfaces)
            .await
            .context("failed to set link name")
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
