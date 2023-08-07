use super::{AsyncStream, Ctx, Instance, TableResult};

use crate::capability::bus::host;
use crate::capability::Bus;

use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use futures::future::Shared;
use futures::FutureExt;
use tracing::instrument;
use wasmtime_wasi::preview2;
use wasmtime_wasi::preview2::stream::TableStreamExt;

impl Instance {
    /// Set [`Bus`] handler for this [Instance].
    pub fn bus(&mut self, bus: Arc<dyn Bus + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_bus(bus);
        self
    }
}

type FutureResult = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

trait TableFutureResultExt {
    fn push_future_result(&mut self, res: FutureResult) -> TableResult<u32>;
    fn get_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
    fn delete_future_result(&mut self, res: u32) -> TableResult<Box<Shared<FutureResult>>>;
}

impl TableFutureResultExt for preview2::Table {
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

#[async_trait]
impl host::Host for Ctx {
    #[instrument]
    async fn call(
        &mut self,
        operation: String,
    ) -> anyhow::Result<
        Result<
            (
                host::FutureResult,
                preview2::wasi::io::streams::InputStream,
                preview2::wasi::io::streams::OutputStream,
            ),
            String,
        >,
    > {
        match self.handler.call(operation).await {
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
    async fn listen_to_future_result(&mut self, _res: u32) -> anyhow::Result<u32> {
        bail!("unsupported") // TODO: Support
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
