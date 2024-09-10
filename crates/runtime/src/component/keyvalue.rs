use super::{Ctx, Handler, ReplacedInstanceTarget};

use crate::capability::keyvalue::{atomics, store, watcher};
use crate::capability::wrpc;

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use bytes::Bytes;
use tracing::instrument;
use wasmtime::component::Resource;

type Result<T, E = store::Error> = core::result::Result<T, E>;

impl From<wrpc::wrpc::keyvalue::store::Error> for store::Error {
    fn from(value: wrpc::wrpc::keyvalue::store::Error) -> Self {
        match value {
            wrpc::wrpc::keyvalue::store::Error::NoSuchStore => Self::NoSuchStore,
            wrpc::wrpc::keyvalue::store::Error::AccessDenied => Self::AccessDenied,
            wrpc::wrpc::keyvalue::store::Error::Other(other) => Self::Other(other),
        }
    }
}

#[async_trait]
impl<H> atomics::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn increment(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::atomics::increment(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueAtomics),
            bucket,
            &key,
            delta,
        )
        .await?
        {
            Ok(n) => Ok(Ok(n)),
            Err(err) => Ok(Err(err.into())),
        }
    }
}

#[async_trait]
impl<H> store::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn open(&mut self, name: String) -> anyhow::Result<Result<Resource<store::Bucket>>> {
        let bucket = self
            .table
            .push(Arc::from(name))
            .context("failed to open bucket")?;
        Ok(Ok(bucket))
    }
}

#[async_trait]
impl<H> store::HostBucket for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn get(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::get(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(buf) => Ok(Ok(buf.map(Into::into))),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::set(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
            &Bytes::from(outgoing_value),
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::delete(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::exists(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(ok) => Ok(Ok(ok)),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn list_keys(
        &mut self,
        bucket: Resource<store::Bucket>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::list_keys(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            cursor,
        )
        .await?
        {
            Ok(wrpc::wrpc::keyvalue::store::KeyResponse { keys, cursor }) => {
                Ok(Ok(store::KeyResponse { keys, cursor }))
            }
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    fn drop(&mut self, bucket: Resource<store::Bucket>) -> anyhow::Result<()> {
        self.table
            .delete(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }
}

#[async_trait]
impl<H> watcher::Host for Ctx<H>
where
    H: Handler,
{
    async fn on_set(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<()> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        wrpc::wrpc::keyvalue::watcher::on_set(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueWatch),
            bucket,
            &key,
            &Bytes::copy_from_slice(&value.as_slice()),
        )
        .await?;
        Ok(())
    }

    async fn on_delete(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<()> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        wrpc::wrpc::keyvalue::watcher::on_delete(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueWatch),
            bucket,
            &key,
        )
        .await?;
        Ok(())
    }
}
