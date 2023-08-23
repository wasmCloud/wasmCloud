use super::{AsyncStream, Ctx, Instance, TableResult};

use crate::capability::bus::{host, lattice};
use crate::capability::{Bus, TargetInterface};

use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use futures::future::Shared;
use futures::FutureExt;
use tracing::instrument;
use wasmtime_wasi::preview2::{self, TableStreamExt};

impl Instance {
    /// Set [`Bus`] handler for this [Instance].
    pub fn bus(&mut self, bus: Arc<dyn Bus + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_bus(bus);
        self
    }
}

type FutureResult = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

trait TableHostExt {
    fn push_future_result(&mut self, res: FutureResult) -> TableResult<u32>;
    fn get_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
    fn delete_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
}

trait TableLatticeExt {
    fn push_interface_target(&mut self, target: TargetInterface) -> TableResult<u32>;
    fn get_interface_target(&mut self, target: u32) -> TableResult<&TargetInterface>;
    fn delete_interface_target(&mut self, target: u32) -> TableResult<TargetInterface>;
}

impl TableHostExt for preview2::Table {
    fn push_future_result(&mut self, res: FutureResult) -> TableResult<u32> {
        self.push(Box::new(res.shared()))
    }
    fn get_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>> {
        self.get(res).cloned()
    }
    fn delete_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>> {
        self.delete(res)
    }
}

impl TableLatticeExt for preview2::Table {
    fn push_interface_target(&mut self, target: TargetInterface) -> TableResult<u32> {
        self.push(Box::new(target))
    }

    fn get_interface_target(&mut self, target: u32) -> TableResult<&TargetInterface> {
        self.get(target)
    }

    fn delete_interface_target(&mut self, target: u32) -> TableResult<TargetInterface> {
        self.delete(target)
    }
}

#[async_trait]
impl host::Host for Ctx {
    #[instrument]
    async fn call(
        &mut self,
        target: Option<host::TargetEntity>,
        operation: String,
    ) -> anyhow::Result<
        Result<
            (
                host::FutureResult,
                preview2::bindings::io::streams::InputStream,
                preview2::bindings::io::streams::OutputStream,
            ),
            String,
        >,
    > {
        let target = target
            .map(TryInto::try_into)
            .transpose()
            .context("failed to parse target")?;
        match self.handler.call(target, operation).await {
            Ok((result, stdin, stdout)) => {
                let result = self
                    .table
                    .push_future_result(result)
                    .context("failed to push result to table")?;
                let stdin = self
                    .table
                    .push_output_stream(Box::new(AsyncStream(stdin)))
                    .context("failed to push stdin stream")?;
                let stdout = self
                    .table
                    .push_input_stream(Box::new(AsyncStream(stdout)))
                    .context("failed to push stdout stream")?;
                Ok(Ok((result, stdin, stdout)))
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn call_sync(
        &mut self,
        target: Option<host::TargetEntity>,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, String>> {
        let target = target
            .map(TryInto::try_into)
            .transpose()
            .context("failed to parse target")?;
        match self.handler.call_sync(target, operation, payload).await {
            Ok(res) => Ok(Ok(res)),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn listen_to_future_result(&mut self, _res: u32) -> anyhow::Result<u32> {
        bail!("not supported") // TODO: Support
    }

    #[instrument]
    async fn future_result_get(&mut self, res: u32) -> anyhow::Result<Option<Result<(), String>>> {
        let fut = self.table.get_future_result(res)?;
        if let Some(result) = fut.clone().now_or_never() {
            let fut = self.table.delete_future_result(res)?;
            drop(fut);
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    #[instrument]
    async fn drop_future_result(&mut self, res: u32) -> anyhow::Result<()> {
        let fut = self.table.delete_future_result(res)?;
        drop(fut);
        Ok(())
    }
}

#[async_trait]
impl lattice::Host for Ctx {
    #[instrument]
    async fn set_target(
        &mut self,
        target: Option<host::TargetEntity>,
        interfaces: Vec<host::TargetInterface>,
    ) -> anyhow::Result<()> {
        let interfaces = interfaces
            .into_iter()
            .map(|interface| self.table.get_interface_target(interface).copied())
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

    #[instrument]
    async fn target_wasi_keyvalue_atomic(&mut self) -> anyhow::Result<host::TargetInterface> {
        self.table
            .push_interface_target(TargetInterface::WasiKeyvalueAtomic)
            .context("failed to push target interface")
    }

    #[instrument]
    async fn target_wasi_keyvalue_readwrite(&mut self) -> anyhow::Result<host::TargetInterface> {
        self.table
            .push_interface_target(TargetInterface::WasiKeyvalueReadwrite)
            .context("failed to push target interface")
    }

    #[instrument]
    async fn target_wasi_logging_logging(&mut self) -> anyhow::Result<host::TargetInterface> {
        self.table
            .push_interface_target(TargetInterface::WasiLoggingLogging)
            .context("failed to push target interface")
    }

    #[instrument]
    async fn target_wasmcloud_blobstore_consumer(
        &mut self,
    ) -> anyhow::Result<host::TargetInterface> {
        self.table
            .push_interface_target(TargetInterface::WasmcloudBlobstoreConsumer)
            .context("failed to push target interface")
    }

    #[instrument]
    async fn target_wasmcloud_messaging_consumer(
        &mut self,
    ) -> anyhow::Result<host::TargetInterface> {
        self.table
            .push_interface_target(TargetInterface::WasmcloudMessagingConsumer)
            .context("failed to push target interface")
    }
}
