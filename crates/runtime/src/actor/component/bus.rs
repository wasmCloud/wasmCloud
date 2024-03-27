use super::{Ctx, Instance, TableResult};

use crate::capability::bus::{guest_config, lattice};
use crate::capability::Bus;

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use tracing::instrument;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::Resource;

impl Instance {
    /// Set [`Bus`] handler for this [Instance].
    pub fn bus(&mut self, bus: Arc<dyn Bus + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_bus(bus);
        self
    }
}

#[async_trait]
impl lattice::Host for Ctx {
    async fn set_link_name(
        &mut self,
        link_name: String,
        interfaces: Vec<Resource<CallTargetInterface>>,
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
impl lattice::HostCallTargetInterface for Ctx {
    async fn new(
        &mut self,
        namespace: String,
        package: String,
        interface: String,
    ) -> anyhow::Result<Resource<lattice::CallTargetInterface>> {
        self.table
            .push(CallTargetInterface {
                namespace,
                package,
                interface,
            })
            .context("failed to push target interface")
    }

    fn drop(&mut self, interface: Resource<lattice::CallTargetInterface>) -> anyhow::Result<()> {
        self.table.delete(interface)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl guest_config::Host for Ctx {
    #[instrument]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, guest_config::ConfigError>> {
        self.handler.get(&key).await
    }

    #[instrument]
    async fn get_all(
        &mut self,
    ) -> anyhow::Result<Result<Vec<(String, Vec<u8>)>, guest_config::ConfigError>> {
        self.handler.get_all().await
    }
}
