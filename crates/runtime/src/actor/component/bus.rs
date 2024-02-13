use super::{Ctx, Instance, TableResult};

use crate::capability::bus::{guest_config, host, lattice};
use crate::capability::{Bus, TargetInterface};

use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use futures::future::Shared;
use futures::FutureExt;
use tracing::instrument;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::preview2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::preview2::{HostOutputStream, InputStream, OutputStream, Pollable};

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
    fn get_future_result(&self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
    fn delete_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
}

impl TableHostExt for ResourceTable {
    fn push_future_result(&mut self, res: FutureResult) -> TableResult<u32> {
        let res = self.push(Box::new(res.shared()))?;
        Ok(res.rep())
    }
    fn get_future_result(&self, res: u32) -> TableResult<Box<Shared<FutureResult>>> {
        self.get(&Resource::new_borrow(res)).cloned()
    }
    fn delete_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>> {
        self.delete(Resource::new_own(res))
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
                Resource<InputStream>,
                Resource<OutputStream>,
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
                let stdin: Box<dyn HostOutputStream> =
                    Box::new(AsyncWriteStream::new(1 << 16, stdin));
                let stdin = self
                    .table
                    .push(stdin)
                    .context("failed to push stdin stream")?;
                let stdout = self
                    .table
                    .push(InputStream::Host(Box::new(AsyncReadStream::new(stdout))))
                    .context("failed to push stdout stream")?;
                Ok(Ok((result, stdout, stdin)))
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument(skip(self, payload))]
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
    async fn listen_to_future_result(&mut self, _res: u32) -> anyhow::Result<Resource<Pollable>> {
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
    async fn set_target(
        &mut self,
        target: Option<host::TargetEntity>,
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
