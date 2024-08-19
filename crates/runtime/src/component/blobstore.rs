use super::{Ctx, Handler, ReplacedInstanceTarget};

use crate::capability::blobstore::blobstore::ContainerName;
use crate::capability::blobstore::container::Container;
use crate::capability::blobstore::types::{
    ContainerMetadata, Error, ObjectId, ObjectMetadata, ObjectName,
};
use crate::capability::blobstore::{blobstore, container, types};
use crate::io::BufferedIncomingStream;

use core::future::Future;
use core::mem;
use core::pin::Pin;

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use futures::future::OptionFuture;
use futures::{future, FutureExt, Stream, StreamExt as _};
use tokio::sync::mpsc;
use tokio::{select, try_join};
use tokio_stream::wrappers::ReceiverStream;
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::{HostInputStream, HostOutputStream, StreamError, StreamResult, Subscribe};
use wrpc_interface_blobstore::bindings;

/// Maximum chunk size, pretty arbitrary number of bytes that should fit in a single transport
/// packet. Some profiling is due to figure out the optimal value here.
/// This should be configurable by users of this crate.
const MAX_CHUNK_SIZE: usize = 1 << 16;

type Result<T, E = Error> = core::result::Result<T, E>;

pub struct OutgoingValue {
    guest: GuestOutgoingValue,
    host: HostOutgoingValue,
}

#[derive(Default)]
pub enum GuestOutgoingValue {
    #[default]
    Corrupted,
    Init(mpsc::Sender<Bytes>),
}

#[derive(Default)]
pub enum HostOutgoingValue {
    #[default]
    Corrupted,
    Init(mpsc::Receiver<Bytes>),
    Writing {
        status: Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        io: Option<AbortOnDropJoinHandle<anyhow::Result<()>>>,
    },
}

pub struct IncomingValue {
    stream: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    status: Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    io: Option<AbortOnDropJoinHandle<anyhow::Result<()>>>,
}

pub struct StreamObjectNames {
    stream: BufferedIncomingStream<String>,
    status: future::Fuse<Pin<Box<dyn Future<Output = Result<(), String>> + Send>>>,
    io: OptionFuture<future::Fuse<AbortOnDropJoinHandle<anyhow::Result<()>>>>,
}

#[async_trait]
impl<H> container::HostContainer for Ctx<H>
where
    H: Handler,
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
        match bindings::wrpc::blobstore::blobstore::get_container_info(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            name,
        )
        .await?
        {
            Ok(bindings::wrpc::blobstore::types::ContainerMetadata { created_at }) => {
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
    ) -> anyhow::Result<Result<Resource<IncomingValue>>> {
        let container = self
            .table
            .get(&container)
            .context("failed to get container")?;
        match bindings::wrpc::blobstore::blobstore::get_container_data(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            &bindings::wasi::blobstore::types::ObjectId {
                container: container.to_string(),
                object: name,
            },
            start,
            end,
        )
        .await?
        {
            (Ok((stream, status)), io) => {
                let io = io.map(wasmtime_wasi::runtime::spawn);
                let value = self
                    .table
                    .push(IncomingValue { stream, status, io })
                    .context("failed to push stream and I/O future")?;
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
        data: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<()>> {
        let container = self
            .table
            .get(&container)
            .cloned()
            .context("failed to get container")?;
        let OutgoingValue { host, .. } = self
            .table
            .get_mut(&data)
            .context("failed to get outgoing value")?;
        let HostOutgoingValue::Init(rx) = mem::take(host) else {
            bail!("outgoing-value.write-data was already called")
        };
        match bindings::wrpc::blobstore::blobstore::write_container_data(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            &bindings::wrpc::blobstore::types::ObjectId {
                container: container.to_string(),
                object,
            },
            Box::pin(ReceiverStream::new(rx)),
        )
        .await?
        {
            (Ok(status), io) => {
                let io = io.map(wasmtime_wasi::runtime::spawn);
                *host = HostOutgoingValue::Writing { status, io };
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
        match bindings::wrpc::blobstore::blobstore::list_container_objects(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            container,
            None,
            None,
        )
        .await?
        {
            (Ok((stream, status)), io) => {
                let stream = BufferedIncomingStream::new(stream);
                let status = status.fuse();
                let io = io
                    .map(wasmtime_wasi::runtime::spawn)
                    .map(FutureExt::fuse)
                    .into();
                let stream = self
                    .table
                    .push(StreamObjectNames { stream, status, io })
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
        bindings::wrpc::blobstore::blobstore::delete_objects(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
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
        bindings::wrpc::blobstore::blobstore::has_object(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            &bindings::wrpc::blobstore::types::ObjectId {
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
        match bindings::wrpc::blobstore::blobstore::get_object_info(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            &bindings::wrpc::blobstore::types::ObjectId {
                container: container.to_string(),
                object: name.clone(),
            },
        )
        .await?
        {
            Ok(bindings::wrpc::blobstore::types::ObjectMetadata { created_at, size }) => {
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
        bindings::wrpc::blobstore::blobstore::clear_container(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreContainer),
            container,
        )
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
        let StreamObjectNames {
            stream,
            ref mut status,
            ref mut io,
        } = self
            .table
            .get_mut(&this)
            .context("failed to get object name stream")?;
        let mut names = Vec::with_capacity(len.try_into().unwrap_or(usize::MAX));
        for _ in 0..len {
            select! {
                biased;

                Some(Err(err)) = &mut *io => {
                    return Ok(Err(format!("{:#}", err.context("failed to perform async I/O"))))
                }
                Err(err) = &mut *status => {
                    return Ok(Err(err))
                }
                item = stream.next() => {
                    match item {
                        Some(name) => names.push(name),
                        None => return Ok(Ok((names, true))),
                    }
                }
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
        let StreamObjectNames { stream, status, io } = self
            .table
            .get_mut(&this)
            .context("failed to get object name stream")?;
        for i in 0..num {
            select! {
                biased;

                Some(Err(err)) = &mut *io => {
                    return Ok(Err(format!("{:#}", err.context("failed to perform async I/O"))))
                }
                Err(err) = &mut *status => {
                    return Ok(Err(err))
                }
                item = stream.next() => {
                    match item {
                        Some(_) => {}
                        None => return Ok(Ok((i, true))),
                    }
                }
            }
        }
        Ok(Ok((num, false)))
    }
}

#[derive(Default)]
enum OutputStream {
    #[default]
    Corrupted,
    Pending(mpsc::Sender<Bytes>),
    Ready(mpsc::OwnedPermit<Bytes>),
    Error(mpsc::error::SendError<()>),
}

impl HostOutputStream for OutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        match mem::take(self) {
            OutputStream::Corrupted => Err(StreamError::Trap(anyhow!(
                "corrupted output stream memory state"
            ))),
            OutputStream::Pending(sender) => {
                *self = OutputStream::Pending(sender);
                Err(StreamError::Trap(anyhow!(
                    "`check_write` was not called prior to calling `write`"
                )))
            }
            OutputStream::Ready(permit) => {
                let sender = permit.send(bytes);
                *self = OutputStream::Pending(sender);
                Ok(())
            }
            OutputStream::Error(err) => {
                *self = OutputStream::Error(err);
                Err(StreamError::LastOperationFailed(anyhow!("broken pipe")))
            }
        }
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        match self {
            OutputStream::Corrupted => Err(StreamError::Trap(anyhow!(
                "corrupted output stream memory state"
            ))),
            OutputStream::Pending(..) => Ok(0),
            OutputStream::Ready(..) => Ok(MAX_CHUNK_SIZE),
            OutputStream::Error(..) => {
                Err(StreamError::LastOperationFailed(anyhow!("broken pipe")))
            }
        }
    }
}

#[async_trait]
impl Subscribe for OutputStream {
    async fn ready(&mut self) {
        match mem::take(self) {
            OutputStream::Corrupted => {}
            OutputStream::Pending(sender) => match sender.reserve_owned().await {
                Ok(permit) => *self = OutputStream::Ready(permit),
                Err(err) => *self = OutputStream::Error(err),
            },
            OutputStream::Ready(permit) => *self = OutputStream::Ready(permit),
            OutputStream::Error(err) => *self = OutputStream::Error(err),
        }
    }
}

#[async_trait]
impl<H: Handler> types::HostOutgoingValue for Ctx<H> {
    #[instrument(skip(self))]
    fn drop(&mut self, outgoing_value: Resource<OutgoingValue>) -> anyhow::Result<()> {
        self.table
            .delete(outgoing_value)
            .context("failed to delete outgoing value")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValue>> {
        let (tx, rx) = mpsc::channel(128);
        self.table
            .push(OutgoingValue {
                guest: GuestOutgoingValue::Init(tx),
                host: HostOutgoingValue::Init(rx),
            })
            .context("failed to push outgoing value")
    }

    #[instrument(skip(self))]
    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<Resource<Box<dyn HostOutputStream>>, ()>> {
        let OutgoingValue { guest, .. } = self
            .table
            .get_mut(&outgoing_value)
            .context("failed to get outgoing value")?;
        let GuestOutgoingValue::Init(tx) = mem::take(guest) else {
            return Ok(Err(()));
        };
        let stream: Box<dyn HostOutputStream> = Box::new(OutputStream::Pending(tx));
        let stream = self
            .table
            .push_child(stream, &outgoing_value)
            .context("failed to push output stream")?;
        Ok(Ok(stream))
    }

    #[instrument(skip(self), ret)]
    async fn finish(&mut self, this: Resource<OutgoingValue>) -> anyhow::Result<Result<()>> {
        let OutgoingValue { host, .. } = self
            .table
            .delete(this)
            .context("failed to delete outgoing value")?;
        match host {
            HostOutgoingValue::Corrupted => Ok(Err("corrupted value state".to_string())),
            HostOutgoingValue::Init(..) => Ok(Ok(())),
            HostOutgoingValue::Writing { status, io } => Ok(async {
                try_join!(
                    async {
                        if let Some(io) = io {
                            io.await
                                .context("I/O task failed")
                                .map_err(|err| format!("{err:#}"))?;
                        }
                        Ok(())
                    },
                    status,
                )?;
                Ok(())
            }
            .await),
        }
    }
}

struct InputStream {
    ready: VecDeque<Bytes>,
    stream: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    status: future::Fuse<Pin<Box<dyn Future<Output = Result<(), String>> + Send>>>,
    io: OptionFuture<future::Fuse<AbortOnDropJoinHandle<anyhow::Result<()>>>>,
    error: Option<StreamError>,
    closed: bool,
}

impl HostInputStream for InputStream {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        if let Some(mut buf) = self.ready.pop_front() {
            if buf.len() > size {
                self.ready.push_front(buf.split_off(size));
            }
            Ok(buf)
        } else if self.closed {
            Err(StreamError::Closed)
        } else {
            Err(StreamError::Trap(anyhow!(
                "`ready` was not called prior to calling `read`"
            )))
        }
    }
}

#[async_trait]
impl Subscribe for InputStream {
    async fn ready(&mut self) {
        if !self.ready.is_empty() || self.closed {
            return;
        }
        select! {
            biased;

            Some(Err(err)) = &mut self.io => {
                self.error = Some(StreamError::LastOperationFailed(err.context("failed to perform async I/O")));
            }
            Err(err) = &mut self.status => {
                self.error = Some(StreamError::LastOperationFailed(anyhow!(err)));
            }
            item = self.stream.next() => {
                if let Some(buf) = item {
                    self.ready.push_back(buf);
                } else {
                    self.closed = true;
                }
            }
        }
    }
}

#[async_trait]
impl<H: Handler> types::HostIncomingValue for Ctx<H> {
    #[instrument(skip(self))]
    fn drop(&mut self, incoming_value: Resource<IncomingValue>) -> anyhow::Result<()> {
        let _ = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Vec<u8>>> {
        let IncomingValue { stream, status, io } = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        Ok(async {
            let (buf, (), ()) = try_join!(
                async {
                    Ok(stream
                        .fold(Vec::default(), |mut buf, chunk| async move {
                            buf.extend_from_slice(&chunk);
                            buf
                        })
                        .await)
                },
                status,
                async {
                    if let Some(io) = io {
                        io.await
                            .context("failed to perform async I/O")
                            .map_err(|err| format!("{err:#}"))?;
                    }
                    Ok(())
                },
            )?;
            Ok(buf)
        }
        .await)
    }

    #[instrument(skip(self))]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Resource<wasmtime_wasi::InputStream>>> {
        let IncomingValue { stream, status, io } = self
            .table
            .delete(incoming_value)
            .context("failed to delete incoming value")?;
        let stream = self
            .table
            .push(wasmtime_wasi::InputStream::Host(Box::new(InputStream {
                ready: VecDeque::default(),
                stream,
                status: status.fuse(),
                io: io.map(FutureExt::fuse).into(),
                error: None,
                closed: false,
            })))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }

    #[instrument(skip(self))]
    async fn size(&mut self, _incoming_value: Resource<IncomingValue>) -> anyhow::Result<u64> {
        bail!("size unknown")
    }
}

impl<H: Handler> types::Host for Ctx<H> {}

#[async_trait]
impl<H> blobstore::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(skip(self))]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>>> {
        match bindings::wrpc::blobstore::blobstore::create_container(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
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
        match bindings::wrpc::blobstore::blobstore::container_exists(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
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
        bindings::wrpc::blobstore::blobstore::delete_container(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
            &name,
        )
        .await
    }

    #[instrument(skip(self))]
    async fn container_exists(&mut self, name: ContainerName) -> anyhow::Result<Result<bool>> {
        bindings::wrpc::blobstore::blobstore::container_exists(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
            &name,
        )
        .await
    }

    #[instrument(skip(self))]
    async fn copy_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bindings::wrpc::blobstore::blobstore::copy_object(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
            &bindings::wasi::blobstore::types::ObjectId {
                container: src.container,
                object: src.object,
            },
            &bindings::wasi::blobstore::types::ObjectId {
                container: dest.container,
                object: dest.object,
            },
        )
        .await
    }

    #[instrument(skip(self))]
    async fn move_object(&mut self, src: ObjectId, dest: ObjectId) -> anyhow::Result<Result<()>> {
        bindings::wrpc::blobstore::blobstore::move_object(
            &self.handler,
            Some(ReplacedInstanceTarget::BlobstoreBlobstore),
            &bindings::wasi::blobstore::types::ObjectId {
                container: src.container,
                object: src.object,
            },
            &bindings::wasi::blobstore::types::ObjectId {
                container: dest.container,
                object: dest.object,
            },
        )
        .await
    }
}

#[async_trait]
impl<H> container::Host for Ctx<H> where H: Handler {}
