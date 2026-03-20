use std::collections::HashSet;
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::plugin::WorkloadTracker;
use crate::wit::{WitInterface, WitWorld};
use async_nats::jetstream::object_store::{self, List, Object, ObjectStore};
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime::error::Context;
use wasmtime_wasi::p2::pipe::AsyncReadStream;
use wasmtime_wasi::p2::{InputStream, OutputStream};
use wasmtime_wasi_io::poll::Pollable;
use wasmtime_wasi_io::streams::{StreamError, StreamResult};

const PLUGIN_BLOBSTORE_ID: &str = "wasi-blobstore";

mod bindings {
    wasmtime::component::bindgen!({
        world: "blobstore",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:io": ::wasmtime_wasi_io::bindings::wasi::io,
            "wasi:blobstore/container.container": crate::plugin::wasi_blobstore::nats::ContainerData,
            "wasi:blobstore/container.stream-object-names": crate::plugin::wasi_blobstore::nats::StreamObjectNamesHandle,
            "wasi:blobstore/types.incoming-value": crate::plugin::wasi_blobstore::nats::IncomingValueHandle,
            "wasi:blobstore/types.outgoing-value": crate::plugin::wasi_blobstore::nats::OutgoingValueHandle,
        },
    });
}

use bindings::wasi::blobstore::container::Error as ContainerError;
use bindings::wasi::blobstore::types::{
    ContainerMetadata, ContainerName, Error as BlobstoreError, ObjectId, ObjectMetadata, ObjectName,
};

/// In-memory container representation
#[derive(Clone)]
pub struct ContainerData {
    pub name: String,
    pub store: ObjectStore,
}

pub struct WorkloadData {
    /// Which buckets (containers) are accessible to this workload
    pub buckets: HashSet<String>,
    /// Whether the blobstore is read-only for this workload
    pub read_only: bool,
    /// Cancellation token for any ongoing operations
    pub cancel_token: tokio_util::sync::CancellationToken,
}

/// Resource representation for an incoming value (data being read)
pub struct IncomingValueHandle {
    pub object: Object,
}

/// Resource representation for an outgoing value (data being written)
/// The operation is delayed until `finish` is called
pub struct OutgoingValueHandle {
    pub temp_file: tempfile::NamedTempFile,
    pub container: Option<ContainerData>,
    pub object_name: Option<String>,
}

/// An `OutputStream` that writes synchronously to a file.
///
/// Unlike `AsyncWriteStream`, this does not spawn a background task, so there
/// is no race between the writer and a subsequent `finish()` that reopens
/// the file. Tempfile I/O is typically very fast (often backed by tmpfs).
struct TempFileOutputStream {
    file: std::fs::File,
}

#[async_trait::async_trait]
impl Pollable for TempFileOutputStream {
    async fn ready(&mut self) {}
}

#[async_trait::async_trait]
impl OutputStream for TempFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        use std::io::Write;
        self.file
            .write_all(&bytes)
            .map_err(|e| StreamError::LastOperationFailed(e.into()))
    }

    fn flush(&mut self) -> StreamResult<()> {
        use std::io::Write;
        self.file
            .flush()
            .map_err(|e| StreamError::LastOperationFailed(e.into()))
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024 * 1024) // 1MB at a time
    }
}

/// Resource representation for streaming object names
pub struct StreamObjectNamesHandle {
    pub objects: List,
}

/// NATS blobstore plugin
#[derive(Clone)]
pub struct NatsBlobstore {
    client: Arc<async_nats::jetstream::Context>,
    tracker: Arc<RwLock<WorkloadTracker<WorkloadData, ()>>>,
}

impl NatsBlobstore {
    pub fn new(client: &async_nats::Client) -> Self {
        Self {
            client: async_nats::jetstream::new(client.clone()).into(),
            tracker: Arc::default(),
        }
    }

    async fn workload_permit(
        &self,
        workload_id: &str,
        container_name: &str,
        is_write: bool,
    ) -> Option<tokio_util::sync::CancellationToken> {
        let tracker = self.tracker.read().await;
        match tracker.workloads.get(workload_id) {
            Some(item) => {
                if let Some(data) = &item.workload_data {
                    if data.buckets.contains(container_name) {
                        return Some(data.cancel_token.clone());
                    }
                    if is_write && data.read_only {
                        return None;
                    }
                }
                None
            }
            None => None,
        }
    }
}

// Implementation for the main blobstore interface
impl<'a> bindings::wasi::blobstore::blobstore::Host for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin.workload_permit(&self.workload_id, &name, true).await {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        let store = match plugin
            .client
            .create_object_store(object_store::Config {
                bucket: name.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(format!("failed to create bucket: {e}")));
            }
        };

        let container_data = ContainerData {
            name: name.clone(),
            store,
        };

        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin
            .workload_permit(&self.workload_id, &name, false)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        let store = match plugin.client.get_object_store(name.to_string()).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(format!("failed to get bucket: {e}")));
            }
        };

        let container_data = ContainerData {
            name: name.clone(),
            store,
        };

        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn delete_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin.workload_permit(&self.workload_id, &name, true).await {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        if let Err(e) = plugin.client.delete_object_store(name.to_string()).await {
            return Ok(Err(format!("failed to delete bucket: {e}")));
        };

        Ok(Ok(()))
    }

    #[instrument(skip(self))]
    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<bool, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin.workload_permit(&self.workload_id, &name, true).await {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        match plugin.client.get_object_store(name.to_string()).await {
            Ok(_) => Ok(Ok(true)),
            Err(_) => Ok(Ok(false)),
        }
    }

    #[instrument(skip(self))]
    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _read_permit = match plugin
            .workload_permit(&self.workload_id, &src.container, false)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized read".to_string()));
            }
        };

        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &dest.container, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized write".to_string()));
            }
        };

        let read_store = match plugin.client.get_object_store(src.container.clone()).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(format!("failed to get source bucket: {e}")));
            }
        };

        let write_store = match plugin.client.get_object_store(dest.container.clone()).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(format!("failed to get destination bucket: {e}")));
            }
        };

        match read_store.get(&src.object).await {
            Ok(mut object) => match write_store.put(dest.object.as_str(), &mut object).await {
                Ok(_) => Ok(Ok(())),
                Err(e) => Ok(Err(format!(
                    "failed to write data to destination object: {e}"
                ))),
            },
            Err(e) => Ok(Err(format!("failed to get source object: {e}"))),
        }
    }

    #[instrument(skip(self))]
    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        // requires an extra write permit on the src container to delete the object after copy
        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &src.container, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized delete".to_string()));
            }
        };
        let delete_store = match plugin.client.get_object_store(src.container.clone()).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(format!("failed to get source bucket: {e}")));
            }
        };

        let copy = self.copy_object(src.clone(), dest.clone()).await?;
        if let Err(e) = copy {
            return Ok(Err(format!("failed to copy object during move: {e}")));
        }

        match delete_store.delete(src.object.as_str()).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to delete source object: {e}"))),
        }
    }
}

impl<'a> bindings::wasi::blobstore::container::HostContainer for ActiveCtx<'a> {
    async fn name(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<String, ContainerError>> {
        let container_data = self.table.get(&container)?;
        Ok(Ok(container_data.name.clone()))
    }

    async fn info(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<ContainerMetadata, ContainerError>> {
        let container_data = self.table.get(&container)?;

        Ok(Ok(ContainerMetadata {
            name: container_data.name.clone(),
            created_at: 0,
        }))
    }

    #[instrument(skip(self, container))]
    async fn get_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        _start: u64,
        _end: u64,
    ) -> wasmtime::Result<Result<Resource<IncomingValueHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?;

        let object = match container_data.store.get(name.as_str()).await {
            Ok(obj) => obj,
            Err(e) => {
                tracing::warn!(
                    container = container_data.name,
                    object = name,
                    "Failed to get object from store: {e}"
                );
                return Ok(Err(format!("object '{name}' does not exist")));
            }
        };

        let resource = self.table.push(IncomingValueHandle { object })?;

        Ok(Ok(resource))
    }

    #[instrument(skip(self, container, data))]
    async fn write_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        data: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let container_data = self.table.get(&container).cloned()?;
        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &container_data.name, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized write".to_string()));
            }
        };

        // prepare the write operation
        // it actually happens on 'finish'
        let handle = self.table.get_mut(&data)?;
        handle.container = Some(container_data);
        handle.object_name = Some(name.as_str().to_string());

        Ok(Ok(()))
    }

    #[instrument(skip(self, container))]
    async fn list_objects(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<Resource<StreamObjectNamesHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?;

        let list_names = match container_data.store.list().await {
            Ok(names) => names,
            Err(e) => {
                return Ok(Err(format!(
                    "failed to list objects in container '{}': {e}",
                    container_data.name
                )));
            }
        };

        let resource = self.table.push(StreamObjectNamesHandle {
            objects: list_names,
        })?;

        Ok(Ok(resource))
    }

    #[instrument(skip(self, container))]
    async fn delete_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let container_data = self.table.get(&container)?;
        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &container_data.name, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized write".to_string()));
            }
        };

        match container_data.store.delete(name.as_str()).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to delete object: {e}"))),
        }
    }

    #[instrument(skip(self, container, names))]
    async fn delete_objects(
        &mut self,
        container: Resource<ContainerData>,
        names: Vec<ObjectName>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let container_data = self.table.get(&container)?;
        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &container_data.name, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized write".to_string()));
            }
        };

        for name in names {
            if let Err(e) = container_data.store.delete(name.as_str()).await {
                return Ok(Err(format!("failed to delete object: {e}")));
            }
        }

        Ok(Ok(()))
    }

    #[instrument(skip(self, container))]
    async fn has_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<bool, ContainerError>> {
        let container_data = self.table.get(&container)?;
        match container_data.store.info(name.as_str()).await {
            Ok(_) => Ok(Ok(true)),
            Err(_) => Ok(Ok(false)),
        }
    }

    #[instrument(skip(self, container))]
    async fn object_info(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<ObjectMetadata, ContainerError>> {
        let container_data = self.table.get(&container)?;
        match container_data.store.info(name.as_str()).await {
            Ok(info) => Ok(Ok(ObjectMetadata {
                name: info.name,
                container: container_data.name.clone(),
                created_at: 0,
                size: info.size as u64,
            })),
            Err(_) => Ok(Err(format!("object '{name}' does not exist"))),
        }
    }

    #[instrument(skip(self, container))]
    async fn clear(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<NatsBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let container_data = self.table.get(&container)?;
        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &container_data.name, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized delete".to_string()));
            }
        };

        let mut object_list = match container_data.store.list().await {
            Ok(list) => list,
            Err(e) => {
                return Ok(Err(format!(
                    "failed to list objects in container '{}': {e}",
                    container_data.name
                )));
            }
        };

        while let Some(object) = object_list.next().await {
            match object {
                Ok(obj) => {
                    if let Err(e) = container_data.store.delete(&obj.name).await {
                        return Ok(Err(format!(
                            "failed to delete object '{}' in container '{}': {e}",
                            obj.name, container_data.name
                        )));
                    }
                }
                Err(e) => {
                    return Ok(Err(format!(
                        "failed to list objects in container '{}': {e}",
                        container_data.name
                    )));
                }
            };
        }

        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<ContainerData>) -> wasmtime::Result<()> {
        // Container resource cleanup - resource table handles this automatically
        tracing::debug!(
            workload_id = self.workload_id.as_ref(),
            component_id = self.component_id.as_ref(),
            resource_id = ?rep,
            "Dropping container resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::container::HostStreamObjectNames for ActiveCtx<'a> {
    async fn read_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNamesHandle>,
        len: u64,
    ) -> wasmtime::Result<Result<(Vec<ObjectName>, bool), ContainerError>> {
        let stream_handle = self.table.get_mut(&stream)?;

        let mut objects = Vec::<ObjectName>::new();
        for _ in 0..len {
            match stream_handle.objects.next().await {
                Some(Ok(obj)) => {
                    objects.push(obj.name);
                }
                Some(Err(e)) => {
                    return Ok(Err(format!("failed to read object name from stream: {e}")));
                }
                None => {
                    return Ok(Ok((objects, true)));
                }
            }
        }

        Ok(Ok((objects, false)))
    }

    async fn skip_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNamesHandle>,
        num: u64,
    ) -> wasmtime::Result<Result<(u64, bool), ContainerError>> {
        let stream_handle = self.table.get_mut(&stream)?;

        for i in 0..num {
            match stream_handle.objects.next().await {
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    return Ok(Err(format!("failed to read object name from stream: {e}")));
                }
                None => {
                    return Ok(Ok((i, true)));
                }
            }
        }

        Ok(Ok((num, false)))
    }

    async fn drop(&mut self, rep: Resource<StreamObjectNamesHandle>) -> wasmtime::Result<()> {
        // StreamObjectNames resource cleanup
        tracing::debug!(
            workload_id = self.workload_id.as_ref(),
            component_id = self.component_id.as_ref(),
            resource_id = ?rep,
            "Dropping StreamObjectNames resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::HostOutgoingValue for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn new_outgoing_value(&mut self) -> wasmtime::Result<Resource<OutgoingValueHandle>> {
        let temp_file = tempfile::Builder::new()
            .tempfile()
            .context("failed to create buffer file")?;

        let handle = OutgoingValueHandle {
            temp_file,
            container: None,
            object_name: None,
        };

        let resource = self.table.push(handle)?;
        Ok(resource)
    }

    #[instrument(skip(self))]
    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>>
    {
        let handle = self.table.get_mut(&outgoing_value)?;

        // Reopen the tempfile so the OutputStream has its own file descriptor.
        // TempFileOutputStream writes synchronously — no background task, no race.
        let file = handle.temp_file.reopen()?;
        let stream = TempFileOutputStream { file };

        let boxed: Box<dyn OutputStream> = Box::new(stream);
        let resource = self.table.push(boxed)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let handle = self.table.delete(outgoing_value)?;
        let container_data = match handle.container {
            Some(data) => data,
            None => {
                return Ok(Err(
                    "outgoing value not associated with a container".to_string()
                ));
            }
        };

        let object_name = match handle.object_name {
            Some(name) => name,
            None => {
                return Ok(Err(
                    "outgoing value not associated with an object name".to_string()
                ));
            }
        };

        let mut file = tokio::fs::File::from_std(handle.temp_file.reopen()?);

        match container_data
            .store
            .put(object_name.as_str(), &mut file)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                return Ok(Err(format!("failed to write object data: {e}")));
            }
        }

        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueHandle>) -> wasmtime::Result<()> {
        match self.finish(rep).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a> bindings::wasi::blobstore::types::HostIncomingValue for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<Result<Vec<u8>, BlobstoreError>> {
        let mut data = self.table.delete(incoming_value)?;
        let mut buf = Vec::new();

        match data.object.read(&mut buf).await {
            Ok(_) => Ok(Ok(buf)),
            Err(e) => Ok(Err(format!("failed to read object data: {e}"))),
        }
    }

    #[instrument(skip(self))]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobstoreError>,
    > {
        let data = self.table.delete(incoming_value)?;

        let stream: Box<dyn InputStream> = Box::new(AsyncReadStream::new(data.object));
        let stream = self.table.push(stream)?;

        Ok(Ok(stream))
    }

    async fn size(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<u64> {
        let data = self.table.get(&incoming_value)?;
        Ok(data.object.info().size as u64)
    }

    async fn drop(&mut self, rep: Resource<IncomingValueHandle>) -> wasmtime::Result<()> {
        tracing::debug!(
            workload_id = self.workload_id.as_ref(),
            resource_id = ?rep,
            "Dropping IncomingValue resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Implement the main types Host trait that combines all resource types
impl<'a> bindings::wasi::blobstore::types::Host for ActiveCtx<'a> {}

// Implement the main container Host trait that combines all resource types
impl<'a> bindings::wasi::blobstore::container::Host for ActiveCtx<'a> {}

#[async_trait::async_trait]
impl HostPlugin for NatsBlobstore {
    fn id(&self) -> &'static str {
        PLUGIN_BLOBSTORE_ID
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:blobstore/blobstore,container,types@0.2.0-draft",
            )]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(_interface) = interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "blobstore")
        else {
            return Ok(());
        };

        tracing::debug!(
            workload_id = component_handle.workload_id(),
            component_id = component_handle.id(),
            "Adding blobstore interfaces"
        );
        let linker = component_handle.linker();

        bindings::wasi::blobstore::blobstore::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasi::blobstore::container::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasi::blobstore::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;

        Ok(())
    }

    async fn on_workload_bind(
        &self,
        workload: &crate::engine::workload::UnresolvedWorkload,
        host_interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(interface) = host_interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "blobstore")
        else {
            return Ok(());
        };

        let buckets = match interface.config.get("buckets") {
            Some(buckets) => buckets.split(',').map(|s| s.to_string()).collect(),
            None => vec![],
        };

        let read_only = interface
            .config
            .get("read_only")
            .is_some_and(|v| v == "true");

        self.tracker.write().await.add_unresolved_workload(
            workload,
            WorkloadData {
                buckets: HashSet::from_iter(buckets),
                read_only,
                cancel_token: tokio_util::sync::CancellationToken::new(),
            },
        );

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let workload_cleanup = |workload_data: Option<WorkloadData>| async {
            if let Some(data) = workload_data {
                data.cancel_token.cancel();
            }
        };
        let component_cleanup = |_| async move {};

        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(workload_id, workload_cleanup, component_cleanup)
            .await;

        Ok(())
    }
}
