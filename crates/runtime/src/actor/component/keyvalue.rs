use super::{AsyncStream, Ctx, Instance, TableResult};

use crate::capability::keyvalue::{readwrite, types, wasi_cloud_error};
use crate::capability::KeyValueReadWrite;
use crate::io::AsyncVec;

use std::sync::Arc;

use anyhow::{anyhow, ensure, Context};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};
use tracing::instrument;
use wasmtime_wasi::preview2::{self, TableStreamExt};

impl Instance {
    /// Set [`KeyValueReadWrite`] handler for this [Instance].
    pub fn keyvalue_readwrite(
        &mut self,
        keyvalue_readwrite: Arc<dyn KeyValueReadWrite + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut()
            .replace_keyvalue_readwrite(keyvalue_readwrite);
        self
    }
}

trait TableKeyValueExt {
    fn push_bucket(&mut self, name: String) -> TableResult<types::Bucket>;
    fn get_bucket(&mut self, bucket: types::Bucket) -> TableResult<&String>;
    fn delete_bucket(&mut self, bucket: types::Bucket) -> TableResult<String>;

    fn push_incoming_value(
        &mut self,
        stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
        size: u64,
    ) -> TableResult<types::IncomingValue>;
    fn get_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<&(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;
    fn delete_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;

    fn push_outgoing_value(&mut self, stream: AsyncVec) -> TableResult<types::OutgoingValue>;
    fn get_outgoing_value(&mut self, stream: types::OutgoingValue) -> TableResult<&AsyncVec>;
    fn delete_outgoing_value(&mut self, stream: types::OutgoingValue) -> TableResult<AsyncVec>;

    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_cloud_error::Error>;
    fn get_error(&mut self, error: wasi_cloud_error::Error) -> TableResult<&anyhow::Error>;
    fn delete_error(&mut self, error: wasi_cloud_error::Error) -> TableResult<anyhow::Error>;
}

impl TableKeyValueExt for preview2::Table {
    fn push_bucket(&mut self, name: String) -> TableResult<types::Bucket> {
        self.push(Box::new(name))
    }

    fn get_bucket(&mut self, bucket: types::Bucket) -> TableResult<&String> {
        self.get(bucket)
    }

    fn delete_bucket(&mut self, bucket: types::Bucket) -> TableResult<String> {
        self.delete(bucket)
    }

    fn push_incoming_value(
        &mut self,
        stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
        size: u64,
    ) -> TableResult<types::IncomingValue> {
        self.push(Box::new((stream, size)))
    }

    fn get_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<&(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        self.get(stream)
    }

    fn delete_incoming_value(
        &mut self,
        stream: types::IncomingValue,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        self.delete(stream)
    }

    fn push_outgoing_value(&mut self, stream: AsyncVec) -> TableResult<types::OutgoingValue> {
        self.push(Box::new(stream))
    }

    fn get_outgoing_value(&mut self, stream: types::OutgoingValue) -> TableResult<&AsyncVec> {
        self.get(stream)
    }

    fn delete_outgoing_value(&mut self, stream: types::OutgoingValue) -> TableResult<AsyncVec> {
        self.delete(stream)
    }

    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_cloud_error::Error> {
        self.push(Box::new(error))
    }

    fn get_error(&mut self, error: wasi_cloud_error::Error) -> TableResult<&anyhow::Error> {
        self.get(error)
    }

    fn delete_error(&mut self, error: wasi_cloud_error::Error) -> TableResult<anyhow::Error> {
        self.delete(error)
    }
}

type Result<T, E = types::Error> = core::result::Result<T, E>;

#[async_trait]
impl readwrite::Host for Ctx {
    #[instrument]
    async fn get(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<types::IncomingValue>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.get(bucket, key).await {
            Ok((stream, size)) => {
                let value = self
                    .table
                    .push_incoming_value(stream, size)
                    .context("failed to push stream and size")?;
                Ok(Ok(value))
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get_outgoing_value(outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        stream.rewind().await.context("failed to rewind stream")?;
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.set(bucket, key, Box::new(stream)).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.delete(bucket, key).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: types::Bucket,
        key: types::Key,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.exists(bucket, key).await {
            Ok(true) => Ok(Ok(true)),
            Ok(false) => {
                // NOTE: This is required until
                // https://github.com/WebAssembly/wasi-keyvalue/pull/18 is merged
                let err = self
                    .table
                    .push_error(anyhow!("key does not exist"))
                    .context("failed to push error")?;
                Ok(Err(err))
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }
}

#[async_trait]
impl types::Host for Ctx {
    #[instrument]
    async fn drop_bucket(&mut self, bucket: types::Bucket) -> anyhow::Result<()> {
        self.table
            .delete_bucket(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }

    #[instrument]
    async fn open_bucket(&mut self, name: String) -> anyhow::Result<Result<types::Bucket>> {
        let bucket = self
            .table
            .push_bucket(name)
            .context("failed to open bucket")?;
        Ok(Ok(bucket))
    }

    #[instrument]
    async fn drop_outgoing_value(
        &mut self,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<()> {
        self.table
            .delete_outgoing_value(outgoing_value)
            .context("failed to delete outgoing value")?;
        Ok(())
    }

    #[instrument]
    async fn new_outgoing_value(&mut self) -> anyhow::Result<types::OutgoingValue> {
        self.table
            .push_outgoing_value(AsyncVec::default())
            .context("failed to push outgoing value")
    }

    #[instrument]
    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: types::OutgoingValue,
    ) -> anyhow::Result<Result<types::OutputStream, ()>> {
        let stream = self
            .table
            .get_outgoing_value(outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        let stream = self
            .table
            .push_output_stream(Box::new(AsyncStream(stream)))
            .context("failed to push output stream")?;
        Ok(Ok(stream))
    }

    #[instrument]
    async fn drop_incoming_value(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<()> {
        self.table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(())
    }

    #[instrument]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<Result<types::IncomingValueSyncBody>> {
        let (stream, size) = self
            .table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        let mut stream = stream.take(size);
        let size = size.try_into().context("size does not fit in `usize`")?;
        let mut buf = Vec::with_capacity(size);
        match stream.read_to_end(&mut buf).await {
            Ok(n) => {
                ensure!(n == size);
                Ok(Ok(buf))
            }
            Err(err) => {
                let err = self
                    .table
                    .push_error(anyhow!(err).context("failed to read stream"))
                    .context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: types::IncomingValue,
    ) -> anyhow::Result<Result<types::IncomingValueAsyncBody>> {
        let (stream, _) = self
            .table
            .delete_incoming_value(incoming_value)
            .context("failed to delete incoming value")?;
        let stream = self
            .table
            .push_input_stream(Box::new(AsyncStream(stream)))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }

    #[instrument]
    async fn size(&mut self, incoming_value: types::IncomingValue) -> anyhow::Result<u64> {
        let (_, size) = self
            .table
            .get_incoming_value(incoming_value)
            .context("failed to get incoming value")?;
        Ok(*size)
    }
}

#[async_trait]
impl wasi_cloud_error::Host for Ctx {
    #[instrument]
    async fn drop_error(&mut self, error: wasi_cloud_error::Error) -> anyhow::Result<()> {
        self.table
            .delete_error(error)
            .context("failed to delete error")?;
        Ok(())
    }

    #[instrument]
    async fn trace(&mut self, error: wasi_cloud_error::Error) -> anyhow::Result<String> {
        self.table
            .get_error(error)
            .context("failed to get error")
            .map(|err| format!("{err:#}"))
    }
}
