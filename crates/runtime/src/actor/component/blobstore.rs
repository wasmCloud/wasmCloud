use super::{Ctx, Instance, TableResult};

use crate::capability::blobstore::blobstore::ContainerName;
use crate::capability::blobstore::container::{Container, StreamObjectNames};
use crate::capability::blobstore::types::{
    ContainerMetadata, Error, ObjectId, ObjectMetadata, ObjectName,
};
use crate::capability::blobstore::{blobstore, container, types};
use crate::capability::Blobstore;
use crate::io::AsyncVec;

use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};
use tracing::instrument;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::preview2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::preview2::{HostOutputStream, InputStream};

type Result<T, E = Error> = core::result::Result<T, E>;

impl Instance {
    /// Set [`Blobstore`] handler for this [Instance].
    pub fn blobstore(&mut self, blobstore: Arc<dyn Blobstore + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_blobstore(blobstore);
        self
    }
}

trait TableBlobstoreExt {
    fn get_container(&self, container: Resource<Container>) -> TableResult<&String>;
    fn delete_container(&mut self, container: Resource<Container>) -> TableResult<String>;

    fn get_object_name_stream_mut(
        &mut self,
        stream: Resource<StreamObjectNames>,
    ) -> TableResult<&mut Box<dyn Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>>;

    fn push_incoming_value(
        &mut self,
        stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
        size: u64,
    ) -> TableResult<types::IncomingValue>;
    fn delete_incoming_value(
        &mut self,
        stream: Resource<types::IncomingValue>,
    ) -> TableResult<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;

    fn get_outgoing_value(&self, stream: Resource<types::OutgoingValue>) -> TableResult<&AsyncVec>;
}

impl TableBlobstoreExt for ResourceTable {
    fn get_container(&self, container: Resource<Container>) -> TableResult<&String> {
        self.get(&Resource::new_borrow(container.rep()))
    }

    fn delete_container(&mut self, container: Resource<Container>) -> TableResult<String> {
        self.delete(Resource::new_own(container.rep()))
    }

    fn get_object_name_stream_mut(
        &mut self,
        stream: Resource<StreamObjectNames>,
    ) -> TableResult<&mut Box<dyn Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>>
    {
        self.get_mut(&Resource::new_borrow(stream.rep()))
    }

    fn push_incoming_value(
        &mut self,
        stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
        size: u64,
    ) -> TableResult<types::IncomingValue> {
        let res = self.push((stream, size))?;
        // Ok(res.rep())
        todo!()
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
}

#[async_trait]
impl blobstore::Host for Ctx {
    #[instrument]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>>> {
        match self.handler.create_container(&name).await {
            Ok(()) => {
                let container = self.table.push(name).context("failed to push container")?;
                // Ok(Ok(container))
                todo!()
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>>> {
        match self.handler.container_exists(&name).await {
            Ok(true) => {
                let container = self.table.push(name).context("failed to push container")?;
                // Ok(Ok(container))
                todo!()
            }
            Ok(false) => Ok(Err("container does not exist".into())),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<()>> {
        match self.handler.delete_container(&name).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn container_exists(&mut self, name: ContainerName) -> anyhow::Result<Result<bool>> {
        match self.handler.container_exists(&name).await {
            Ok(exists) => Ok(Ok(exists)),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[allow(unused)] // TODO: Implement and remove
    #[instrument]
    async fn copy_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }

    #[allow(unused)] // TODO: Implement and remove
    #[instrument]
    async fn move_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bail!("not supported yet")
    }
}

#[async_trait]
impl container::Host for Ctx {}

#[async_trait]
impl container::HostContainer for Ctx {
    #[instrument]
    async fn name(&mut self, container: Resource<Container>) -> anyhow::Result<Result<String>> {
        let name = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        Ok(Ok(name.clone()))
    }

    #[instrument]
    async fn info(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<ContainerMetadata>> {
        let name = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.container_info(name).await {
            Ok(md) => Ok(Ok(md)),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn get_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Resource<types::IncomingValue>>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.get_data(container, name, start..=end).await {
            Ok((stream, size)) => {
                let value = self
                    .table
                    .push_incoming_value(stream, size)
                    .context("failed to push stream and size")?;
                // Ok(Ok(value))
                todo!()
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn write_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        data: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<()>> {
        let mut stream = self
            .table
            .get_outgoing_value(data)
            .context("failed to get outgoing value")?
            .clone();
        stream.rewind().await.context("failed to rewind stream")?;
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self
            .handler
            .write_data(container, name, Box::new(stream))
            .await
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn list_objects(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNames>>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.list_objects(container).await {
            Ok(stream) => {
                let stream = self
                    .table
                    .push(stream)
                    .context("failed to push object name stream")?;
                // Ok(Ok(stream))
                todo!()
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn delete_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<()>> {
        self.delete_objects(container, vec![name]).await
    }

    #[instrument]
    async fn delete_objects(
        &mut self,
        container: Resource<Container>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<()>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.delete_objects(container, names).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn has_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.has_object(container, name).await {
            Ok(exists) => Ok(Ok(exists)),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn object_info(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.object_info(container, name).await {
            Ok(info) => Ok(Ok(info)),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn clear(&mut self, container: Resource<Container>) -> anyhow::Result<Result<()>> {
        let container = self
            .table
            .get_container(container)
            .context("failed to get container")?;
        match self.handler.clear_container(container).await {
            Ok(()) => Ok(Ok(())),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    fn drop(&mut self, container: Resource<Container>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl container::HostStreamObjectNames for Ctx {
    async fn read_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNames>,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<String>, bool)>> {
        todo!()
    }

    async fn skip_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNames>,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool)>> {
        todo!()
    }

    fn drop(&mut self, stream_object_names: Resource<StreamObjectNames>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl types::Host for Ctx {}

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

    async fn size(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<u64> {
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

    async fn outgoing_value_write_body(
        &mut self,
        resource: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<std::result::Result<Resource<types::OutputStream>, ()>> {
        todo!()
    }

    fn drop(&mut self, resource: Resource<types::OutgoingValue>) -> anyhow::Result<()> {
        todo!()
    }
}
