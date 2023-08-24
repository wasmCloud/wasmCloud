use super::{Ctx, Instance};

use crate::capability::blobstore::blobstore::ContainerName;
use crate::capability::blobstore::container::{Container, StreamObjectNames};
use crate::capability::blobstore::types::{
    ContainerMetadata, Error, ObjectId, ObjectMetadata, ObjectName, ObjectSize,
};
use crate::capability::blobstore::{blobstore, container, data_blob, types};
use crate::capability::Blobstore;

use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use tracing::instrument;

type Result<T> = core::result::Result<T, Error>;

impl Instance {
    /// Set [`Blobstore`] handler for this [Instance].
    pub fn blobstore(&mut self, blobstore: Arc<dyn Blobstore + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_blobstore(blobstore);
        self
    }
}

impl types::Host for Ctx {}

#[allow(unused)] // TODO: Implement and remove
#[async_trait]
impl data_blob::Host for Ctx {
    #[instrument]
    async fn drop_data_blob(&mut self, blob: data_blob::DataBlob) -> anyhow::Result<()> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn create(
        &mut self,
        blob: data_blob::DataBlob,
    ) -> anyhow::Result<data_blob::WriteStream> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn read(
        &mut self,
        blob: data_blob::DataBlob,
    ) -> anyhow::Result<Result<data_blob::ReadStream>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn size(&mut self, blob: data_blob::DataBlob) -> anyhow::Result<Result<ObjectSize>> {
        bail!("not supported yet")
    }
}

#[allow(unused)] // TODO: Implement and remove
#[async_trait]
impl container::Host for Ctx {
    #[instrument]
    async fn drop_container(&mut self, cont: Container) -> anyhow::Result<()> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn name(&mut self, cont: Container) -> anyhow::Result<Result<String>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn info(&mut self, cont: Container) -> anyhow::Result<Result<ContainerMetadata>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn read_object(
        &mut self,
        cont: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<container::ReadStream>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn write_object(
        &mut self,
        cont: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<container::WriteStream>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn get_data(
        &mut self,
        cont: Container,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<container::DataBlob>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn write_data(
        &mut self,
        cont: Container,
        name: ObjectName,
        data: container::DataBlob,
    ) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn drop_stream_object_names(&mut self, names: StreamObjectNames) -> anyhow::Result<()> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn read_stream_object_names(
        &mut self,
        this: StreamObjectNames,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool)>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn skip_stream_object_names(
        &mut self,
        this: StreamObjectNames,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool)>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn list_objects(&mut self, cont: Container) -> anyhow::Result<Result<StreamObjectNames>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn delete_object(
        &mut self,
        cont: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn delete_objects(
        &mut self,
        cont: Container,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn has_object(
        &mut self,
        cont: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn object_info(
        &mut self,
        cont: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn clear(&mut self, cont: Container) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }
}

#[allow(unused)] // TODO: Implement and remove
#[async_trait]
impl blobstore::Host for Ctx {
    #[instrument]
    async fn create_container(&mut self, name: ContainerName) -> anyhow::Result<Result<Container>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn get_container(&mut self, name: ContainerName) -> anyhow::Result<Result<Container>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn container_exists(&mut self, name: ContainerName) -> anyhow::Result<Result<bool>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn copy_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[instrument]
    async fn move_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }
}
