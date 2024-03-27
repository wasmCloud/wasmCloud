use super::{Ctx, Instance};

use crate::capability::keyvalue::{atomic, eventual, types, wasi_keyvalue_error};
use crate::capability::{KeyValueAtomic, KeyValueEventual};
use crate::io::{AsyncVec, IncomingInputStreamReader};

use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt as _;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::{HostOutputStream, InputStream};

impl Instance {
    /// Set [`KeyValueAtomic`] handler for this [Instance].
    pub fn keyvalue_atomic(
        &mut self,
        keyvalue_atomic: Arc<dyn KeyValueAtomic + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_keyvalue_atomic(keyvalue_atomic);
        self
    }

    /// Set [`KeyValueEventual`] handler for this [Instance].
    pub fn keyvalue_eventual(
        &mut self,
        keyvalue_eventual: Arc<dyn KeyValueEventual + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut()
            .replace_keyvalue_eventual(keyvalue_eventual);
        self
    }
}

type Result<T, E = Resource<wasi_keyvalue_error::Error>> = core::result::Result<T, E>;

#[async_trait]
impl atomic::Host for Ctx {
    #[instrument]
    async fn increment(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.increment(bucket, key, delta).await {
            Ok(new) => Ok(Ok(new)),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn compare_and_swap(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
        old: u64,
        new: u64,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.compare_and_swap(bucket, key, old, new).await {
            Ok(changed) => Ok(Ok(changed)),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }
}

#[async_trait]
impl eventual::Host for Ctx {
    #[instrument]
    async fn get(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
    ) -> anyhow::Result<Result<Option<Resource<types::IncomingValue>>>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.get(bucket, key).await {
            Ok(Some(stream)) => {
                let value = self.table.push(stream).context("failed to push stream")?;
                Ok(Ok(Some(value)))
            }
            Ok(None) => Ok(Ok(None)),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
        outgoing_value: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get::<AsyncVec>(&outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        stream.rewind().await.context("failed to rewind stream")?;
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.set(bucket, key, Box::new(stream)).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.delete(bucket, key).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match self.handler.exists(bucket, key).await {
            Ok(exists) => Ok(Ok(exists)),
            Err(err) => {
                let err = self.table.push(err).context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }
}

#[async_trait]
impl types::HostBucket for Ctx {
    #[instrument]
    fn drop(&mut self, bucket: Resource<types::Bucket>) -> anyhow::Result<()> {
        self.table
            .delete(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }

    #[instrument]
    async fn open_bucket(
        &mut self,
        name: String,
    ) -> anyhow::Result<Result<Resource<types::Bucket>>> {
        let bucket = self
            .table
            .push(Arc::new(name))
            .context("failed to open bucket")?;
        Ok(Ok(bucket))
    }
}

#[async_trait]
impl types::HostOutgoingValue for Ctx {
    #[instrument]
    fn drop(&mut self, outgoing_value: Resource<types::OutgoingValue>) -> anyhow::Result<()> {
        self.table
            .delete(outgoing_value)
            .context("failed to delete outgoing value")?;
        Ok(())
    }

    #[instrument]
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<types::OutgoingValue>> {
        self.table
            .push(AsyncVec::default())
            .context("failed to push outgoing value")
    }

    #[instrument]
    async fn outgoing_value_write_body_sync(
        &mut self,
        outgoing_value: Resource<types::OutgoingValue>,
        body: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get::<AsyncVec>(&outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        stream
            .write_all(&body)
            .await
            .context("failed to write body")?;
        Ok(Ok(()))
    }

    #[instrument]
    async fn outgoing_value_write_body_async(
        &mut self,
        outgoing_value: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<Resource<types::OutputStream>>> {
        let stream = self
            .table
            .get::<AsyncVec>(&outgoing_value)
            .context("failed to get outgoing value")?
            .clone();
        let stream: Box<dyn HostOutputStream> = Box::new(AsyncWriteStream::new(1 << 16, stream));
        let stream = self
            .table
            .push(stream)
            .context("failed to push output stream")?;
        Ok(Ok(stream))
    }
}

#[async_trait]
impl types::HostIncomingValue for Ctx {
    #[instrument]
    fn drop(&mut self, incoming_value: Resource<types::IncomingValue>) -> anyhow::Result<()> {
        let _ = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(())
    }

    #[instrument]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<types::IncomingValueSyncBody>> {
        let stream = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        match stream.try_collect::<Vec<Bytes>>().await {
            Ok(bufs) => Ok(Ok(bufs.concat())),
            Err(err) => {
                let err = self
                    .table
                    .push(anyhow!(err).context("failed to read stream"))
                    .context("failed to push error")?;
                Ok(Err(err))
            }
        }
    }

    #[instrument]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<Resource<InputStream>>> {
        let stream = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        let stream = self
            .table
            .push(InputStream::Host(Box::new(AsyncReadStream::new(
                IncomingInputStreamReader::new(stream),
            ))))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }

    #[instrument]
    async fn incoming_value_size(
        &mut self,
        _incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<u64>> {
        let err = self
            .table
            .push(anyhow!("size unknown"))
            .context("failed to push error")?;
        Ok(Err(err))
    }
}

impl types::Host for Ctx {}

#[async_trait]
impl wasi_keyvalue_error::HostError for Ctx {
    #[instrument]
    fn drop(&mut self, error: Resource<wasi_keyvalue_error::Error>) -> anyhow::Result<()> {
        let _: anyhow::Error = self.table.delete(error).context("failed to delete error")?;
        Ok(())
    }

    #[instrument]
    async fn trace(
        &mut self,
        error: Resource<wasi_keyvalue_error::Error>,
    ) -> anyhow::Result<String> {
        self.table
            .get(&error)
            .context("failed to get error")
            .map(|err: &anyhow::Error| format!("{err:#}"))
    }
}

impl wasi_keyvalue_error::Host for Ctx {}
