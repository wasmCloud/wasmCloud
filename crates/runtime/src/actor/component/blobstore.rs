use super::{Ctx, Instance};

use crate::capability::blobstore::{consumer, types};
use crate::capability::Blobstore;

use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use tracing::instrument;

impl Instance {
    /// Set [`Blobstore`] handler for this [Instance].
    pub fn blobstore(&mut self, blobstore: Arc<dyn Blobstore + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_blobstore(blobstore);
        self
    }
}

#[async_trait]
impl types::Host for Ctx {}

#[allow(unused)] // TODO: Implement and remove
#[async_trait]
impl consumer::Host for Ctx {
    #[instrument]
    async fn container_exists(&mut self, container_id: String) -> anyhow::Result<bool> {
        bail!("unsupported")
    }

    #[instrument]
    async fn create_container(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn remove_container(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn get_container_info(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<Option<types::ContainerInfo>, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn get_object_info(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<Option<types::ObjectInfo>, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn remove_object(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<bool, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn put_object(
        &mut self,
        chunk: types::Chunk,
        content_type: String,
        content_encoding: String,
    ) -> anyhow::Result<Result<String, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn put_chunk(
        &mut self,
        stream_id: String,
        chunk: types::Chunk,
        cancel: bool,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn stream_object(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }
}
