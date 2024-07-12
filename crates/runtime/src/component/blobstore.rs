use super::{Ctx, Handler};

use crate::capability::blobstore::blobstore::ContainerName;
use crate::capability::blobstore::container::{Container, StreamObjectNames};
use crate::capability::blobstore::types::{
    ContainerMetadata, Error, ObjectId, ObjectMetadata, ObjectName,
};
use crate::capability::blobstore::{blobstore, container, types};
use crate::capability::wrpc;
use crate::io::{AsyncVec, BufferedIncomingStream};

use std::sync::Arc;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt};
use tokio_util::io::StreamReader;
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::{HostOutputStream, InputStream};

type Result<T, E = Error> = core::result::Result<T, E>;

#[async_trait]
impl<H> container::HostContainer for Ctx<H>
where
    H: Handler,
    H::Context: Clone,
{
    #[instrument(skip(self))]
    fn drop(&mut self, container: Resource<Container>) -> anyhow::Result<()> {
        self.table
            .delete(container)
            .context("failed to delete container")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn name(&mut self, container: Resource<Container>) -> anyhow::Result<Result<String>> {
        let name = self
            .table
            .get(&container)
            .context("failed to get container")?;
        Ok(Ok(name.to_string()))
    }

    #[instrument(skip(self))]
    async fn info(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<ContainerMetadata>> {
        let name = self
            .table
            .get(&container)
            .context("failed to get container")?;
        match wrpc::wrpc::blobstore::blobstore::get_container_info(
            &self.handler,
            self.cx.clone(),
            name,
        )
        .await?
        {
            Ok(wrpc::wrpc::blobstore::types::ContainerMetadata { created_at }) => {
                Ok(Ok(ContainerMetadata {
                    name: name.to_string(),
                    created_at,
                }))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn get_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Resource<types::IncomingValue>>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        match wrpc::wrpc::blobstore::blobstore::get_container_data(
            &self.handler,
            self.cx.clone(),
            &wrpc::wasi::blobstore::types::ObjectId {
                container: container.to_string(),
                object: name,
            },
            start,
            end,
        )
        .await?
        {
            (Ok(stream), io) => {
                if let Some(io) = io {
                    // TODO: Move this into the runtime
                    io.await.context("failed to perform async I/O")?;
                }
                let value = self
                    .table
                    .push(stream)
                    .context("failed to push stream and size")?;
                Ok(Ok(value))
            }
            (Err(err), _) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn write_data(
        &mut self,
        container: Resource<Container>,
        object: ObjectName,
        data: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<()>> {
        // TODO: Stream data
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        let mut data = self
            .table
            .get::<AsyncVec>(&data)
            .context("failed to get outgoing value")?
            .clone();
        data.rewind().await.context("failed to rewind stream")?;
        let mut buf = vec![];
        data.read_to_end(&mut buf).await?;
        match wrpc::wrpc::blobstore::blobstore::write_container_data(
            &self.handler,
            self.cx.clone(),
            &wrpc::wrpc::blobstore::types::ObjectId {
                container: container.to_string(),
                object,
            },
            Box::pin(stream::iter([buf.into()])),
        )
        .await?
        {
            (Ok(()), io) => {
                if let Some(io) = io {
                    // TODO: Move this into the runtime
                    io.await.context("failed to perform async I/O")?;
                }
                Ok(Ok(()))
            }
            (Err(err), _) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn list_objects(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNames>>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        // TODO: implement a stream with limit and offset
        match wrpc::wrpc::blobstore::blobstore::list_container_objects(
            &self.handler,
            self.cx.clone(),
            container,
            None,
            None,
        )
        .await?
        {
            (Ok(stream), io) => {
                if let Some(io) = io {
                    // TODO: Move this into the runtime
                    io.await.context("failed to perform async I/O")?;
                }
                let stream = self
                    .table
                    .push(BufferedIncomingStream::new(stream))
                    .context("failed to push object name stream")?;
                Ok(Ok(stream))
            }
            (Err(err), _) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn delete_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<()>> {
        self.delete_objects(container, vec![name]).await
    }

    #[instrument(skip(self))]
    async fn delete_objects(
        &mut self,
        container: Resource<Container>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<()>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        wrpc::wrpc::blobstore::blobstore::delete_objects(
            &self.handler,
            self.cx.clone(),
            container,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await
    }

    #[instrument(skip(self))]
    async fn has_object(
        &mut self,
        container: Resource<Container>,
        object: ObjectName,
    ) -> anyhow::Result<Result<bool>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        wrpc::wrpc::blobstore::blobstore::has_object(
            &self.handler,
            self.cx.clone(),
            &wrpc::wrpc::blobstore::types::ObjectId {
                container: container.to_string(),
                object,
            },
        )
        .await
    }

    #[instrument(skip(self))]
    async fn object_info(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        match wrpc::wrpc::blobstore::blobstore::get_object_info(
            &self.handler,
            self.cx.clone(),
            &wrpc::wrpc::blobstore::types::ObjectId {
                container: container.to_string(),
                object: name.clone(),
            },
        )
        .await?
        {
            Ok(wrpc::wrpc::blobstore::types::ObjectMetadata { created_at, size }) => {
                Ok(Ok(ObjectMetadata {
                    name,
                    container: container.to_string(),
                    created_at,
                    size,
                }))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn clear(&mut self, container: Resource<Container>) -> anyhow::Result<Result<()>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        wrpc::wrpc::blobstore::blobstore::clear_container(&self.handler, self.cx.clone(), container)
            .await
    }
}

#[async_trait]
impl<H: Handler> container::HostStreamObjectNames for Ctx<H> {
    #[instrument(skip(self))]
    fn drop(&mut self, names: Resource<StreamObjectNames>) -> anyhow::Result<()> {
        let _ = self
            .table
            .delete(names)
            .context("failed to delete object name stream")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn read_stream_object_names(
        &mut self,
        this: Resource<StreamObjectNames>,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool)>> {
        let stream = self
            .table
            .get_mut(&this)
            .context("failed to get object name stream")?;
        let mut names = Vec::with_capacity(len.try_into().unwrap_or(usize::MAX));
        for _ in 0..len {
            match stream.next().await {
                Some(name) => names.push(name),
                None => return Ok(Ok((names, true))),
            }
        }
        Ok(Ok((names, false)))
    }

    #[instrument(skip(self))]
    async fn skip_stream_object_names(
        &mut self,
        this: Resource<StreamObjectNames>,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool)>> {
        let stream = self
            .table
            .get_mut(&this)
            .context("failed to get object name stream")?;
        for i in 0..num {
            match stream.next().await {
                Some(_) => {}
                None => return Ok(Ok((i, true))),
            }
        }
        Ok(Ok((num, false)))
    }
}

#[async_trait]
impl<H: Handler> types::HostOutgoingValue for Ctx<H> {
    #[instrument(skip(self))]
    fn drop(&mut self, outgoing_value: Resource<types::OutgoingValue>) -> anyhow::Result<()> {
        self.table
            .delete(outgoing_value)
            .context("failed to delete outgoing value")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<types::OutgoingValue>> {
        self.table
            .push(AsyncVec::default())
            .context("failed to push outgoing value")
    }

    #[instrument(skip(self))]
    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<types::OutgoingValue>,
    ) -> anyhow::Result<Result<Resource<Box<dyn HostOutputStream>>, ()>> {
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
impl<H: Handler> types::HostIncomingValue for Ctx<H> {
    #[instrument(skip(self))]
    fn drop(&mut self, incoming_value: Resource<types::IncomingValue>) -> anyhow::Result<()> {
        let _ = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<Result<types::IncomingValueSyncBody>> {
        let stream = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(Ok(stream.collect::<Vec<Bytes>>().await.concat()))
    }

    #[instrument(skip(self))]
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
                StreamReader::new(stream.map(std::io::Result::Ok)),
            ))))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }

    #[instrument(skip(self))]
    async fn size(
        &mut self,
        _incoming_value: Resource<types::IncomingValue>,
    ) -> anyhow::Result<u64> {
        bail!("size unknown")
    }
}

impl<H: Handler> types::Host for Ctx<H> {}

#[async_trait]
impl<H> blobstore::Host for Ctx<H>
where
    H: Handler,
    H::Context: Clone,
{
    #[instrument(skip(self))]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>>> {
        match wrpc::wrpc::blobstore::blobstore::create_container(
            &self.handler,
            self.cx.clone(),
            &name,
        )
        .await?
        {
            Ok(()) => {
                let container = self
                    .table
                    .push(Arc::from(name))
                    .context("failed to push container")?;
                Ok(Ok(container))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>>> {
        match wrpc::wrpc::blobstore::blobstore::container_exists(
            &self.handler,
            self.cx.clone(),
            &name,
        )
        .await?
        {
            Ok(true) => {
                let container = self
                    .table
                    .push(Arc::from(name))
                    .context("failed to push container")?;
                Ok(Ok(container))
            }
            Ok(false) => Ok(Err("container does not exist".into())),
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<()>> {
        wrpc::wrpc::blobstore::blobstore::delete_container(&self.handler, self.cx.clone(), &name)
            .await
    }

    #[instrument(skip(self))]
    async fn container_exists(&mut self, name: ContainerName) -> anyhow::Result<Result<bool>> {
        wrpc::wrpc::blobstore::blobstore::container_exists(&self.handler, self.cx.clone(), &name)
            .await
    }

    #[instrument(skip(self))]
    async fn copy_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        wrpc::wrpc::blobstore::blobstore::copy_object(
            &self.handler,
            self.cx.clone(),
            &wrpc::wasi::blobstore::types::ObjectId {
                container: src.container,
                object: src.object,
            },
            &wrpc::wasi::blobstore::types::ObjectId {
                container: dest.container,
                object: dest.object,
            },
        )
        .await
    }

    #[instrument(skip(self))]
    async fn move_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        wrpc::wrpc::blobstore::blobstore::move_object(
            &self.handler,
            self.cx.clone(),
            &wrpc::wasi::blobstore::types::ObjectId {
                container: src.container,
                object: src.object,
            },
            &wrpc::wasi::blobstore::types::ObjectId {
                container: dest.container,
                object: dest.object,
            },
        )
        .await
    }
}

#[async_trait]
impl<H> container::Host for Ctx<H>
where
    H: Handler,
    H::Context: Clone,
{
}
