use super::{Ctx, Instance, TableResult};

use crate::capability::keyvalue::{atomic, eventual, types, wasi_keyvalue_error};
use crate::capability::{KeyValueAtomic, KeyValueEventual};
use crate::io::AsyncVec;

use std::sync::Arc;

use anyhow::{anyhow, ensure, Context};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tracing::instrument;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::preview2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::preview2::{HostOutputStream, InputStream};

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

trait TableKeyValueExt {
    fn get_bucket(&self, bucket: Resource<types::Bucket>) -> TableResult<&String>;
    fn delete_incoming_value(
        &mut self,
        stream: Resource<types::IncomingValue>,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;
    fn get_outgoing_value(&self, stream: Resource<types::OutgoingValue>) -> TableResult<&AsyncVec>;
    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_keyvalue_error::Error>;
}

impl TableKeyValueExt for ResourceTable {
    fn get_bucket(&self, bucket: Resource<types::Bucket>) -> TableResult<&String> {
        self.get(&Resource::new_borrow(bucket.rep()))
    }

    fn delete_incoming_value(
        &mut self,
        stream: Resource<types::IncomingValue>,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        self.delete(Resource::new_own(stream.rep()))
    }

    fn get_outgoing_value(&self, stream: Resource<types::OutgoingValue>) -> TableResult<&AsyncVec> {
        self.get(&Resource::new_borrow(stream.rep()))
    }

    fn push_error(&mut self, error: anyhow::Error) -> TableResult<wasi_keyvalue_error::Error> {
        let res = self.push(error)?;
        //Ok(res.rep())
        todo!()
    }
}

type Result<T, E = Resource<types::Error>> = core::result::Result<T, E>;

#[async_trait]
impl atomic::Host for Ctx {
    #[instrument]
    async fn increment(
        &mut self,
        bucket: Resource<types::Bucket>,
        key: types::Key,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.increment(bucket, key, delta).await {
            Ok(new) => Ok(Ok(new)),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                // Ok(Err(err))
                todo!()
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
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.compare_and_swap(bucket, key, old, new).await {
            Ok(changed) => Ok(Ok(changed)),
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                // Ok(Err(err))
                todo!()
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
        let bucket = self
            .table
            .get_bucket(bucket)
            .context("failed to get bucket")?;
        match self.handler.get(bucket, key).await {
            Ok((stream, size)) => {
                let value = self
                    .table
                    .push((stream, size))
                    .context("failed to push stream and size")?;
                // Ok(Ok(value.rep()))
                todo!()
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                // Ok(Err(err))
                todo!()
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
                // Ok(Err(err))
                todo!()
            }
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: Resource<types::Bucket>,
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
                // Ok(Err(err))
                todo!()
            }
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: Resource<types::Bucket>,
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
                // Ok(Err(err))
                todo!()
            }
            Err(err) => {
                let err = self.table.push_error(err).context("failed to push error")?;
                // Ok(Err(err))
                todo!()
            }
        }
    }
}

#[async_trait]
impl types::Host for Ctx {}

#[async_trait]
impl types::HostBucket for Ctx {
    async fn open_bucket(
        &mut self,
        name: String,
    ) -> anyhow::Result<Result<Resource<types::Bucket>>> {
        todo!()
    }

    fn drop(&mut self, bucket: Resource<types::Bucket>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl types::HostIncomingValue for Ctx {
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<types::IncomingValueSyncBody>> {
        todo!()
    }

    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<Resource<types::IncomingValueAsyncBody>>> {
        todo!()
    }

    async fn incoming_value_size(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<u64>> {
        todo!()
    }

    fn drop(&mut self, incoming_value: Resource<types::IncomingValue>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl types::HostOutgoingValue for Ctx {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<types::OutgoingValue>> {
        todo!()
    }

    async fn outgoing_value_write_body_async(
        &mut self,
        resource: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<Resource<types::OutputStream>>> {
        todo!()
    }

    async fn outgoing_value_write_body_sync(
        &mut self,
        resource: Resource<types::OutgoingValue>,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        todo!()
    }

    fn drop(&mut self, resource: Resource<types::OutgoingValue>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl wasi_keyvalue_error::Host for Ctx {}

#[async_trait]
impl wasi_keyvalue_error::HostError for Ctx {
    #[instrument]
    async fn trace(
        &mut self,
        error: Resource<wasi_keyvalue_error::Error>,
    ) -> anyhow::Result<String> {
        todo!()
    }

    fn drop(&mut self, error: Resource<wasi_keyvalue_error::Error>) -> anyhow::Result<()> {
        todo!()
    }
}
