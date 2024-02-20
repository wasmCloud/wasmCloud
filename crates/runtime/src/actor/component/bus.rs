use super::{Ctx, Instance, TableResult};

use crate::capability::bus::{guest_config, lattice};
use crate::capability::{Bus, TargetInterface};

use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use tracing::instrument;
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
    async fn set_target(
        &mut self,
        target: Option<lattice::TargetEntity>,
        interfaces: Vec<Resource<TargetInterface>>,
    ) -> anyhow::Result<()> {
        let interfaces = interfaces
            .into_iter()
            .map(|interface| self.table.get(&interface).cloned())
            .collect::<TableResult<_>>()
            .map_err(|e| anyhow!(e).context("failed to get interface"))?;
        let target = target
            .map(TryInto::try_into)
            .transpose()
            .context("failed to parse target")?;
        self.handler
            .set_target(target, interfaces)
            .await
            .context("failed to set target")?;
        Ok(())
    }
}

#[async_trait]
impl lattice::HostTargetInterface for Ctx {
    async fn new(
        &mut self,
        namespace: String,
        package: String,
        interface: String,
    ) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::Custom {
                namespace,
                package,
                interface,
            })
            .context("failed to push target interface")
    }

    async fn wasi_blobstore_blobstore(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasiBlobstoreBlobstore)
            .context("failed to push target interface")
    }

    async fn wasi_keyvalue_atomic(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasiKeyvalueAtomic)
            .context("failed to push target interface")
    }

    async fn wasi_keyvalue_eventual(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasiKeyvalueEventual)
            .context("failed to push target interface")
    }

    async fn wasi_logging_logging(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasiLoggingLogging)
            .context("failed to push target interface")
    }

    async fn wasi_http_outgoing_handler(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasiHttpOutgoingHandler)
            .context("failed to push target interface")
    }

    async fn wasmcloud_messaging_consumer(&mut self) -> anyhow::Result<Resource<TargetInterface>> {
        self.table
            .push(TargetInterface::WasmcloudMessagingConsumer)
            .context("failed to push target interface")
    }

    fn drop(&mut self, interface: Resource<TargetInterface>) -> anyhow::Result<()> {
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
