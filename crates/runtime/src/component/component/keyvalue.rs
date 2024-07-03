use super::{Ctx, Instance};

use crate::capability::keyvalue::{atomics, store};
use crate::capability::{KeyValueAtomics, KeyValueStore};

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use tracing::instrument;
use wasmtime::component::Resource;

impl Instance {
    /// Set [`KeyValueAtomics`] handler for this [Instance].
    pub fn keyvalue_atomics(
        &mut self,
        keyvalue_atomics: Arc<dyn KeyValueAtomics + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut()
            .replace_keyvalue_atomics(keyvalue_atomics);
        self
    }

    /// Set [`KeyValueStore`] handler for this [Instance].
    pub fn keyvalue_store(
        &mut self,
        keyvalue_store: Arc<dyn KeyValueStore + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_keyvalue_store(keyvalue_store);
        self
    }
}

type Result<T, E = store::Error> = core::result::Result<T, E>;

#[async_trait]
impl atomics::Host for Ctx {
    #[instrument]
    async fn increment(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .increment(bucket, key, delta)
            .await
            .context("failed to invoke method")
    }
}

#[async_trait]
impl store::Host for Ctx {
    #[instrument]
    async fn open(&mut self, name: String) -> anyhow::Result<Result<Resource<store::Bucket>>> {
        let bucket = self
            .table
            .push(Arc::new(name))
            .context("failed to open bucket")?;
        Ok(Ok(bucket))
    }
}

#[async_trait]
impl store::HostBucket for Ctx {
    #[instrument]
    async fn get(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .get(bucket, key)
            .await
            .context("failed to invoke method")
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .set(bucket, key, outgoing_value)
            .await
            .context("failed to invoke method")
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .delete(bucket, key)
            .await
            .context("failed to invoke method")
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .exists(bucket, key)
            .await
            .context("failed to invoke method")
    }

    #[instrument]
    async fn list_keys(
        &mut self,
        bucket: Resource<store::Bucket>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        self.handler
            .list_keys(bucket, cursor)
            .await
            .context("failed to invoke method")
    }

    #[instrument]
    fn drop(&mut self, bucket: Resource<store::Bucket>) -> anyhow::Result<()> {
        self.table
            .delete(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }
}
