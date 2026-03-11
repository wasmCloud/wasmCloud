#![deny(clippy::all)]
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::WorkloadTracker;
use crate::plugin::{HostPlugin, lock_root};
use crate::wit::{WitInterface, WitWorld};

use tokio::sync::RwLock;
use tracing::{debug, instrument};
use wasmtime::component::Resource;
use wasmtime::error::Context as _;
use wasmtime_wasi::p2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::p2::{InputStream, OutputStream};

const PLUGIN_BLOBSTORE_ID: &str = "wasi-blobstore";

mod bindings {
    wasmtime::component::bindgen!({
        world: "blobstore",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:io": ::wasmtime_wasi_io::bindings::wasi::io,
            "wasi:blobstore/container.container": crate::plugin::wasi_blobstore::filesystem::ContainerData,
            "wasi:blobstore/container.stream-object-names": crate::plugin::wasi_blobstore::filesystem::StreamObjectNamesHandle,
            "wasi:blobstore/types.incoming-value": crate::plugin::wasi_blobstore::filesystem::IncomingValueHandle,
            "wasi:blobstore/types.outgoing-value": crate::plugin::wasi_blobstore::filesystem::OutgoingValueHandle,
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
    pub root: PathBuf,
}

pub struct WorkloadData {
    /// Cancellation token for any ongoing operations
    pub cancel_token: tokio_util::sync::CancellationToken,
}

/// Resource representation for an incoming value (data being read)
pub struct IncomingValueHandle {
    pub file: PathBuf,
    pub start: u64,
    pub end: u64,
}

/// Resource representation for an outgoing value (data being written)
/// The operation is delayed until `finish` is called
pub struct OutgoingValueHandle {
    pub temp_file: tempfile::NamedTempFile,
    pub container: Option<ContainerData>,
    pub object_name: Option<String>,
}

/// Resource representation for streaming object names
pub struct StreamObjectNamesHandle {
    pub objects: VecDeque<String>,
}

/// Filesystem blobstore plugin
#[derive(Clone)]
pub struct FilesystemBlobstore {
    root: PathBuf,
    tracker: Arc<RwLock<WorkloadTracker<WorkloadData, ()>>>,
}

impl FilesystemBlobstore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            tracker: Arc::default(),
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
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let root = lock_root(&plugin.root, &name)
            .map_err(|e| wasmtime::format_err!("invalid container name: {e}"))?;

        if let Err(e) = tokio::fs::create_dir_all(&root).await {
            return Ok(Err(format!("failed to create bucket directory: {e}")));
        }

        let container_data = ContainerData {
            name: name.clone(),
            root,
        };

        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let Ok(root) = lock_root(&plugin.root, &name) else {
            return Ok(Err("invalid container name".to_string()));
        };

        if !root.exists() {
            return Ok(Err(format!("bucket '{}' does not exist", name)));
        }

        let container_data = ContainerData {
            name: name.clone(),
            root,
        };

        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn delete_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let Ok(path) = lock_root(&plugin.root, &name) else {
            return Ok(Err("invalid container name".to_string()));
        };
        if !path.exists() {
            return Ok(Err(format!("bucket '{}' does not exist", name)));
        }

        if let Err(e) = tokio::fs::remove_dir_all(&path).await {
            return Ok(Err(format!("failed to delete bucket directory: {e}")));
        }

        Ok(Ok(()))
    }

    #[instrument(skip(self))]
    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<bool, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let Ok(path) = lock_root(&plugin.root, &name) else {
            return Ok(Err("invalid container name".to_string()));
        };

        Ok(Ok(path.exists()))
    }

    #[instrument(skip(self))]
    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let Ok(read_container) = lock_root(&plugin.root, &src.container) else {
            return Ok(Err("invalid source container name".to_string()));
        };

        let Ok(write_container) = lock_root(&plugin.root, &dest.container) else {
            return Ok(Err("invalid destination container name".to_string()));
        };

        let Ok(read_object) = lock_root(read_container, &src.object) else {
            return Ok(Err("invalid source object name".to_string()));
        };

        let Ok(write_object) = lock_root(write_container, &dest.object) else {
            return Ok(Err("invalid destination object name".to_string()));
        };

        if let Some(parent) = write_object.parent()
            && !parent.exists()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return Ok(Err(format!(
                "failed to create parent directories for destination object: {e}"
            )));
        }

        match tokio::fs::copy(&read_object, &write_object).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to copy object data: {e}"))),
        }
    }

    #[instrument(skip(self))]
    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemBlobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let Ok(read_container) = lock_root(&plugin.root, &src.container) else {
            return Ok(Err("invalid source container name".to_string()));
        };

        let Ok(write_container) = lock_root(&plugin.root, &dest.container) else {
            return Ok(Err("invalid destination container name".to_string()));
        };

        let Ok(read_object) = lock_root(read_container, &src.object) else {
            return Ok(Err("invalid source object name".to_string()));
        };

        let Ok(write_object) = lock_root(write_container, &dest.object) else {
            return Ok(Err("invalid destination object name".to_string()));
        };

        if let Some(parent) = write_object.parent()
            && !parent.exists()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return Ok(Err(format!(
                "failed to create parent directories for destination object: {e}"
            )));
        }

        match tokio::fs::rename(&read_object, &write_object).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to copy object data: {e}"))),
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
        start: u64,
        end: u64,
    ) -> wasmtime::Result<Result<Resource<IncomingValueHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?;

        let Ok(file) = lock_root(&container_data.root, name.as_str()) else {
            return Ok(Err("invalid object name".to_string()));
        };

        if !file.exists() {
            return Ok(Err(format!("object '{}' does not exist", name)));
        }

        let resource = self.table.push(IncomingValueHandle { file, start, end })?;

        Ok(Ok(resource))
    }

    #[instrument(skip(self, container, data))]
    async fn write_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        data: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container).cloned()?;

        if lock_root(&container_data.root, name.as_str()).is_err() {
            return Ok(Err("invalid object name".to_string()));
        }

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

        let mut list_names = Vec::new();
        if let Err(e) = list_files_recursively(&container_data.root, &mut list_names) {
            return Ok(Err(format!(
                "failed to list objects in container '{}': {e}",
                container_data.name
            )));
        }

        let resource = self.table.push(StreamObjectNamesHandle {
            objects: list_names
                .iter()
                .map(|p| {
                    p.strip_prefix(&container_data.root)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .to_string()
                })
                .collect(),
        })?;

        Ok(Ok(resource))
    }

    #[instrument(skip(self, container))]
    async fn delete_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?;
        let Ok(path) = lock_root(&container_data.root, name.as_str()) else {
            return Ok(Err(format!("invalid object name: {}", name)));
        };

        match tokio::fs::remove_file(path).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to delete object: {e}"))),
        }
    }

    #[instrument(skip(self, container))]
    async fn delete_objects(
        &mut self,
        container: Resource<ContainerData>,
        names: Vec<ObjectName>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?;

        for name in names {
            let Ok(path) = lock_root(&container_data.root, name.as_str()) else {
                return Ok(Err(format!("invalid object name: {}", name)));
            };
            if let Err(e) = tokio::fs::remove_file(path).await {
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
        let Ok(path) = lock_root(&container_data.root, name.as_str()) else {
            return Ok(Err("invalid object name".to_string()));
        };

        Ok(Ok(path.exists()))
    }

    async fn object_info(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<ObjectMetadata, ContainerError>> {
        let container_data = self.table.get(&container)?;
        let Ok(path) = lock_root(&container_data.root, name.as_str()) else {
            return Ok(Err("invalid object name".to_string()));
        };

        let Ok(metadata) = tokio::fs::metadata(&path).await else {
            return Ok(Err(format!("object '{name}' does not exist")));
        };

        Ok(Ok(ObjectMetadata {
            name,
            container: container_data.name.clone(),
            created_at: 0,
            size: metadata.len(),
        }))
    }

    #[instrument(skip(self, container))]
    async fn clear(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?;
        match tokio::fs::remove_dir_all(&container_data.root).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!(
                "failed to clear container '{}': {e}",
                container_data.name
            ))),
        }
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
            match stream_handle.objects.pop_front() {
                Some(obj) => {
                    objects.push(obj);
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
            if stream_handle.objects.pop_front().is_none() {
                return Ok(Ok((i, true)));
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

    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>>
    {
        let handle = self.table.get_mut(&outgoing_value)?;

        let file_wrapper = tokio::fs::File::from_std(handle.temp_file.reopen()?);
        let stream = AsyncWriteStream::new(8192, file_wrapper);

        // Return the pipe as the output stream - this is the same pipe that will be
        // read in finish()
        let boxed: Box<dyn OutputStream> = Box::new(stream);

        let resource = self.table.push(boxed)?;
        Ok(Ok(resource))
    }

    #[instrument(skip_all)]
    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        debug!("Finishing outgoing value {:?}", outgoing_value);
        let mut handle = self.table.delete(outgoing_value)?;
        let container_data = match handle.container {
            Some(data) => data,
            None => {
                return Ok(Err(
                    "outgoing value not associated with a container".to_string()
                ));
            }
        };

        let Some(handle_name) = handle.object_name else {
            return Ok(Err(
                "outgoing value not associated with an object name".to_string()
            ));
        };

        let Ok(dest_file) = lock_root(&container_data.root, &handle_name) else {
            return Ok(Err("invalid object name".to_string()));
        };

        if let Some(parent) = dest_file.parent()
            && !parent.exists()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return Ok(Err(format!(
                "failed to create parent directories for destination object: {e}"
            )));
        }

        debug!("Flushing {:?}", dest_file.as_path());

        handle.temp_file.flush()?;

        match tokio::fs::copy(handle.temp_file.path(), &dest_file).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to write object data: {e}"))),
        }
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueHandle>) -> wasmtime::Result<()> {
        match self.finish(rep).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a> bindings::wasi::blobstore::types::HostIncomingValue for ActiveCtx<'a> {
    #[instrument(skip_all)]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<Result<Vec<u8>, BlobstoreError>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let data = self.table.delete(incoming_value)?;

        let mut file = match tokio::fs::File::open(&data.file).await {
            Ok(f) => f,
            Err(e) => return Ok(Err(format!("failed to open object file: {e}"))),
        };

        let metadata = match file.metadata().await {
            Ok(m) => m,
            Err(e) => return Ok(Err(format!("failed to get file metadata: {e}"))),
        };

        let file_size = metadata.len();

        // Calculate effective range (inclusive)
        let start = data.start.min(file_size);
        let end = data.end.min(file_size.saturating_sub(1));

        if start > end {
            return Ok(Ok(Vec::new()));
        }

        let length = (end - start + 1) as usize; // +1 because range is inclusive

        let mut buf = vec![0u8; length];

        if let Err(e) = file.seek(std::io::SeekFrom::Start(start)).await {
            return Ok(Err(format!("failed to seek in object file: {e}")));
        }

        if let Err(e) = file.read_exact(&mut buf).await {
            return Ok(Err(format!("failed to read object data: {e}")));
        }

        Ok(Ok(buf))
    }

    #[instrument(skip_all)]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobstoreError>,
    > {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let data = self.table.delete(incoming_value)?;

        let mut file = tokio::fs::File::open(&data.file)
            .await
            .context("failed to open object file")?;

        let metadata = file
            .metadata()
            .await
            .context("failed to get file metadata")?;
        let file_size = metadata.len();

        // Calculate effective range (inclusive)
        let start = data.start.min(file_size);
        let end = data.end.min(file_size.saturating_sub(1));

        file.seek(std::io::SeekFrom::Start(start))
            .await
            .context("failed to seek in object file")?;

        let length = if start > end { 0 } else { end - start + 1 };

        let limited = file.take(length);
        let stream: Box<dyn InputStream> = Box::new(AsyncReadStream::new(limited));
        let stream = self.table.push(stream)?;

        Ok(Ok(stream))
    }

    async fn size(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<u64> {
        let data = self.table.get(&incoming_value)?;

        let metadata = tokio::fs::metadata(&data.file)
            .await
            .context("failed to get object file metadata")?;

        let file_size = metadata.len();
        let start = data.start.min(file_size);
        let end = data.end.min(file_size.saturating_sub(1));

        if start > end {
            Ok(0)
        } else {
            Ok(end - start + 1)
        }
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
impl HostPlugin for FilesystemBlobstore {
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
        let Some(_interface) = host_interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "blobstore")
        else {
            return Ok(());
        };

        self.tracker.write().await.add_unresolved_workload(
            workload,
            WorkloadData {
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

fn list_files_recursively(path: impl AsRef<Path>, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    // Read the entries in the current directory
    for entry_result in std::fs::read_dir(path)? {
        let entry = entry_result?;
        let full_path = entry.path();
        let meta = std::fs::metadata(&full_path)?;

        if meta.is_dir() {
            // If it's a directory, recurse into it
            list_files_recursively(&full_path, files)?;
        } else if meta.is_file() {
            // If it's a file, add it to our list
            files.push(full_path);
        }
    }
    Ok(())
}
