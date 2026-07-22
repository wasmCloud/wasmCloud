#![deny(clippy::all)]
//! Real AWS S3-backed implementation of `wasi:blobstore`.
//!
//! S3 has no real notion of a "container"/subdirectory — everything is a flat key in one
//! bucket. This plugin maps a `wasi:blobstore` container name onto a key prefix
//! (`<container>/<object>`) within one fixed, pre-configured bucket. `create_container` /
//! `get_container` / `container_exists` are therefore logical, not physical operations — a
//! prefix always "exists" the moment you use it, the same simplification most S3-backed
//! blobstore adapters make. This is documented here rather than left implicit.
use std::collections::HashSet;
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use tokio::sync::RwLock;
use tracing::{debug, instrument};
use wasmtime::component::Resource;
use wasmtime_wasi::p2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::p2::{InputStream, OutputStream};

const PLUGIN_BLOBSTORE_ID: &str = "wasi-blobstore";

mod bindings {
    wasmtime::component::bindgen!({
        world: "blobstore",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:io": ::wasmtime_wasi_io::bindings::wasi::io,
            "wasi:blobstore/container.container": crate::plugin::wasi_blobstore::s3::ContainerData,
            "wasi:blobstore/container.stream-object-names": crate::plugin::wasi_blobstore::s3::StreamObjectNamesHandle,
            "wasi:blobstore/types.incoming-value": crate::plugin::wasi_blobstore::s3::IncomingValueHandle,
            "wasi:blobstore/types.outgoing-value": crate::plugin::wasi_blobstore::s3::OutgoingValueHandle,
        },
    });
}

use bindings::wasi::blobstore::container::Error as ContainerError;
use bindings::wasi::blobstore::types::{
    ContainerMetadata, ContainerName, Error as BlobstoreError, ObjectId, ObjectMetadata, ObjectName,
};

#[derive(Clone)]
pub struct ContainerData {
    pub name: String,
    /// The S3 key prefix this container maps to, e.g. `"file-uploader/"`.
    pub prefix: String,
}

/// Downloaded to a local temp file on `get_data` so the sync/async consume paths can reuse
/// the exact same file-streaming logic the filesystem plugin already proved works, instead
/// of inventing a second way to adapt a byte stream into a WASI input-stream.
pub struct IncomingValueHandle {
    pub temp_file: tempfile::NamedTempFile,
    pub size: u64,
}

/// Buffered locally, uploaded to S3 only on `finish` — mirrors the filesystem plugin's
/// write path exactly, just with an S3 PutObject instead of a local file copy at the end.
pub struct OutgoingValueHandle {
    pub temp_file: tempfile::NamedTempFile,
    pub bucket: Option<String>,
    pub key: Option<String>,
}

pub struct StreamObjectNamesHandle {
    pub objects: std::collections::VecDeque<String>,
}

pub struct WorkloadData {
    pub cancel_token: tokio_util::sync::CancellationToken,
}

/// Real AWS S3-backed blobstore plugin.
#[derive(Clone)]
pub struct S3Blobstore {
    client: Arc<S3Client>,
    bucket: String,
    tracker: Arc<RwLock<WorkloadTracker<WorkloadData, ()>>>,
}

impl S3Blobstore {
    pub fn new(client: S3Client, bucket: impl Into<String>) -> Self {
        Self {
            client: Arc::new(client),
            bucket: bucket.into(),
            tracker: Arc::default(),
        }
    }

    fn prefix_for(container_name: &str) -> String {
        format!("{container_name}/")
    }

    fn key_for(prefix: &str, object_name: &str) -> String {
        format!("{prefix}{object_name}")
    }
}

impl<'a> bindings::wasi::blobstore::blobstore::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.blobstore.s3.create_container", skip(self))]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        // No physical operation needed: an S3 prefix exists the moment an object is written
        // under it. See the module-level doc comment.
        let container_data = ContainerData {
            name: name.clone(),
            prefix: S3Blobstore::prefix_for(&name),
        };
        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(name = "wasi.blobstore.s3.get_container", skip(self))]
    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let container_data = ContainerData {
            name: name.clone(),
            prefix: S3Blobstore::prefix_for(&name),
        };
        let resource = self.table.push(container_data)?;
        Ok(Ok(resource))
    }

    #[instrument(name = "wasi.blobstore.s3.delete_container", skip(self))]
    async fn delete_container(
        &mut self,
        name: ContainerName,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let prefix = S3Blobstore::prefix_for(&name);
        if let Err(e) = delete_all_under_prefix(&plugin.client, &plugin.bucket, &prefix).await {
            return Ok(Err(format!("failed to delete container '{name}': {e}")));
        }
        Ok(Ok(()))
    }

    #[instrument(name = "wasi.blobstore.s3.container_exists", skip(self))]
    async fn container_exists(
        &mut self,
        _name: ContainerName,
    ) -> wasmtime::Result<Result<bool, BlobstoreError>> {
        // Logical containers always exist in the flat S3 key space — see module doc comment.
        Ok(Ok(true))
    }

    #[instrument(name = "wasi.blobstore.s3.copy_object", skip(self))]
    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let src_key = S3Blobstore::key_for(&S3Blobstore::prefix_for(&src.container), &src.object);
        let dest_key =
            S3Blobstore::key_for(&S3Blobstore::prefix_for(&dest.container), &dest.object);
        let copy_source = format!("{}/{src_key}", plugin.bucket);

        match plugin
            .client
            .copy_object()
            .bucket(&plugin.bucket)
            .copy_source(copy_source)
            .key(dest_key)
            .send()
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to copy object in S3: {e}"))),
        }
    }

    #[instrument(name = "wasi.blobstore.s3.move_object", skip(self))]
    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let src_key = S3Blobstore::key_for(&S3Blobstore::prefix_for(&src.container), &src.object);
        let dest_key =
            S3Blobstore::key_for(&S3Blobstore::prefix_for(&dest.container), &dest.object);
        let copy_source = format!("{}/{src_key}", plugin.bucket);

        if let Err(e) = plugin
            .client
            .copy_object()
            .bucket(&plugin.bucket)
            .copy_source(copy_source)
            .key(&dest_key)
            .send()
            .await
        {
            return Ok(Err(format!("failed to copy object in S3: {e}")));
        }

        if let Err(e) = plugin
            .client
            .delete_object()
            .bucket(&plugin.bucket)
            .key(&src_key)
            .send()
            .await
        {
            return Ok(Err(format!(
                "copied object but failed to delete source in S3: {e}"
            )));
        }

        Ok(Ok(()))
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

    #[instrument(name = "wasi.blobstore.s3.get_data", skip(self, container))]
    async fn get_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> wasmtime::Result<Result<Resource<IncomingValueHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let key = S3Blobstore::key_for(&container_data.prefix, &name);

        let range = format!("bytes={start}-{end}");
        let output = match plugin
            .client
            .get_object()
            .bucket(&plugin.bucket)
            .key(&key)
            .range(range)
            .send()
            .await
        {
            Ok(o) => o,
            Err(e) => return Ok(Err(format!("object '{name}' could not be read from S3: {e}"))),
        };

        let mut temp_file = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => return Ok(Err(format!("failed to create local buffer file: {e}"))),
        };

        let mut body = output.body;
        let mut size: u64 = 0;
        loop {
            match body.try_next().await {
                Ok(Some(chunk)) => {
                    size += chunk.len() as u64;
                    if let Err(e) = std::io::Write::write_all(&mut temp_file, &chunk) {
                        return Ok(Err(format!("failed to buffer S3 object locally: {e}")));
                    }
                }
                Ok(None) => break,
                Err(e) => return Ok(Err(format!("failed to read S3 object body: {e}"))),
            }
        }

        let resource = self.table.push(IncomingValueHandle { temp_file, size })?;
        Ok(Ok(resource))
    }

    #[instrument(name = "wasi.blobstore.s3.write_data", skip(self, container, data))]
    async fn write_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        data: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let key = S3Blobstore::key_for(&container_data.prefix, &name);

        // Prepare only. The actual PutObject happens in `finish`, same deferred pattern the
        // filesystem plugin uses.
        let handle = self.table.get_mut(&data)?;
        handle.bucket = Some(plugin.bucket.clone());
        handle.key = Some(key);

        Ok(Ok(()))
    }

    #[instrument(name = "wasi.blobstore.s3.list_objects", skip(self, container))]
    async fn list_objects(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<Resource<StreamObjectNamesHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;

        let mut names = Vec::new();
        let mut continuation_token: Option<String> = None;
        loop {
            let mut req = plugin
                .client
                .list_objects_v2()
                .bucket(&plugin.bucket)
                .prefix(&container_data.prefix);
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let output = match req.send().await {
                Ok(o) => o,
                Err(e) => {
                    return Ok(Err(format!(
                        "failed to list objects in container '{}': {e}",
                        container_data.name
                    )));
                }
            };

            for object in output.contents() {
                if let Some(key) = object.key() {
                    if let Some(stripped) = key.strip_prefix(&container_data.prefix) {
                        names.push(stripped.to_string());
                    }
                }
            }

            match output.next_continuation_token() {
                Some(token) => continuation_token = Some(token.to_string()),
                None => break,
            }
        }

        let resource = self.table.push(StreamObjectNamesHandle {
            objects: names.into(),
        })?;
        Ok(Ok(resource))
    }

    #[instrument(name = "wasi.blobstore.s3.delete_object", skip(self, container))]
    async fn delete_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let key = S3Blobstore::key_for(&container_data.prefix, &name);

        match plugin
            .client
            .delete_object()
            .bucket(&plugin.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to delete object in S3: {e}"))),
        }
    }

    #[instrument(name = "wasi.blobstore.s3.delete_objects", skip(self, container))]
    async fn delete_objects(
        &mut self,
        container: Resource<ContainerData>,
        names: Vec<ObjectName>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;

        for name in names {
            let key = S3Blobstore::key_for(&container_data.prefix, &name);
            if let Err(e) = plugin
                .client
                .delete_object()
                .bucket(&plugin.bucket)
                .key(key)
                .send()
                .await
            {
                return Ok(Err(format!("failed to delete object '{name}' in S3: {e}")));
            }
        }

        Ok(Ok(()))
    }

    #[instrument(name = "wasi.blobstore.s3.has_object", skip(self, container))]
    async fn has_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<bool, ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let key = S3Blobstore::key_for(&container_data.prefix, &name);

        match plugin
            .client
            .head_object()
            .bucket(&plugin.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(Ok(true)),
            Err(e) if e.as_service_error().is_some_and(|se| se.is_not_found()) => Ok(Ok(false)),
            Err(e) => Ok(Err(format!("failed to check object existence in S3: {e}"))),
        }
    }

    #[instrument(name = "wasi.blobstore.s3.object_info", skip(self, container))]
    async fn object_info(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<ObjectMetadata, ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;
        let key = S3Blobstore::key_for(&container_data.prefix, &name);

        let output = match plugin
            .client
            .head_object()
            .bucket(&plugin.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(o) => o,
            Err(e) => return Ok(Err(format!("object '{name}' does not exist in S3: {e}"))),
        };

        let size = u64::try_from(output.content_length().unwrap_or(0)).unwrap_or(0);
        let created_at = output
            .last_modified()
            .and_then(|t| u64::try_from(t.secs()).ok())
            .unwrap_or(0);

        Ok(Ok(ObjectMetadata {
            name,
            container: container_data.name.clone(),
            created_at,
            size,
        }))
    }

    #[instrument(name = "wasi.blobstore.s3.clear", skip(self, container))]
    async fn clear(
        &mut self,
        container: Resource<ContainerData>,
    ) -> wasmtime::Result<Result<(), ContainerError>> {
        let container_data = self.table.get(&container)?.clone();
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;

        if let Err(e) =
            delete_all_under_prefix(&plugin.client, &plugin.bucket, &container_data.prefix).await
        {
            return Ok(Err(format!(
                "failed to clear container '{}': {e}",
                container_data.name
            )));
        }
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<ContainerData>) -> wasmtime::Result<()> {
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
                Some(obj) => objects.push(obj),
                None => return Ok(Ok((objects, true))),
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
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::HostOutgoingValue for ActiveCtx<'a> {
    #[instrument(name = "wasi.blobstore.s3.new_outgoing_value", skip(self))]
    async fn new_outgoing_value(&mut self) -> wasmtime::Result<Resource<OutgoingValueHandle>> {
        let temp_file = tempfile::Builder::new()
            .tempfile()
            .map_err(|e| wasmtime::format_err!("failed to create buffer file: {e}"))?;

        let handle = OutgoingValueHandle {
            temp_file,
            bucket: None,
            key: None,
        };
        let resource = self.table.push(handle)?;
        Ok(resource)
    }

    #[instrument(name = "wasi.blobstore.s3.outgoing_value_write_body", skip(self))]
    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>>
    {
        let handle = self.table.get_mut(&outgoing_value)?;
        let file_wrapper = tokio::fs::File::from_std(
            handle
                .temp_file
                .reopen()
                .map_err(|e| wasmtime::format_err!("failed to reopen buffer file: {e}"))?,
        );
        let stream = AsyncWriteStream::new(8192, file_wrapper);
        let boxed: Box<dyn OutputStream> = Box::new(stream);
        let resource = self.table.push(boxed)?;
        Ok(Ok(resource))
    }

    #[instrument(name = "wasi.blobstore.s3.finish", skip_all)]
    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> wasmtime::Result<Result<(), BlobstoreError>> {
        let mut handle = self.table.delete(outgoing_value)?;
        let plugin = self.try_get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID)?;

        let Some(bucket) = handle.bucket else {
            return Ok(Err(
                "outgoing value not associated with a container".to_string()
            ));
        };
        let Some(key) = handle.key else {
            return Ok(Err(
                "outgoing value not associated with an object name".to_string()
            ));
        };

        if let Err(e) = std::io::Write::flush(&mut handle.temp_file) {
            return Ok(Err(format!("failed to flush local buffer: {e}")));
        }

        let bytes = match tokio::fs::read(handle.temp_file.path()).await {
            Ok(b) => b,
            Err(e) => return Ok(Err(format!("failed to read local buffer for upload: {e}"))),
        };
        let body = ByteStream::from(bytes);

        debug!(bucket = %bucket, key = %key, "uploading object to S3");

        match plugin
            .client
            .put_object()
            .bucket(&bucket)
            .key(&key)
            .body(body)
            .send()
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to write object data to S3: {e}"))),
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
    #[instrument(name = "wasi.blobstore.s3.incoming_value_consume_sync", skip_all)]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<Result<Vec<u8>, BlobstoreError>> {
        let data = self.table.delete(incoming_value)?;
        match tokio::fs::read(data.temp_file.path()).await {
            Ok(bytes) => Ok(Ok(bytes)),
            Err(e) => Ok(Err(format!("failed to read buffered S3 object: {e}"))),
        }
    }

    #[instrument(name = "wasi.blobstore.s3.incoming_value_consume_async", skip_all)]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> wasmtime::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobstoreError>,
    > {
        let data = self.table.delete(incoming_value)?;
        let file = match tokio::fs::File::open(data.temp_file.path()).await {
            Ok(f) => f,
            Err(e) => return Ok(Err(format!("failed to open buffered S3 object: {e}"))),
        };

        let stream: Box<dyn InputStream> = Box::new(AsyncReadStream::new(file));
        let stream = self.table.push(stream)?;
        Ok(Ok(stream))
    }

    async fn size(&mut self, incoming_value: Resource<IncomingValueHandle>) -> wasmtime::Result<u64> {
        let data = self.table.get(&incoming_value)?;
        Ok(data.size)
    }

    async fn drop(&mut self, rep: Resource<IncomingValueHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::Host for ActiveCtx<'a> {}
impl<'a> bindings::wasi::blobstore::container::Host for ActiveCtx<'a> {}

#[async_trait::async_trait]
impl HostPlugin for S3Blobstore {
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
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasi", "blobstore", &[]) {
            return Ok(());
        }

        tracing::debug!(
            workload_id = component_handle.workload_id(),
            component_id = component_handle.id(),
            bucket = %self.bucket,
            "Adding S3-backed blobstore interfaces"
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
        host_interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !host_interfaces.contains("wasi", "blobstore", &[]) {
            return Ok(());
        }
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
        _interfaces: WitInterfaces<'_>,
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

async fn delete_all_under_prefix(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
) -> anyhow::Result<()> {
    let mut continuation_token: Option<String> = None;
    loop {
        let mut req = client.list_objects_v2().bucket(bucket).prefix(prefix);
        if let Some(token) = &continuation_token {
            req = req.continuation_token(token);
        }
        let output = req.send().await?;

        for object in output.contents() {
            if let Some(key) = object.key() {
                client.delete_object().bucket(bucket).key(key).send().await?;
            }
        }

        match output.next_continuation_token() {
            Some(token) => continuation_token = Some(token.to_string()),
            None => break,
        }
    }
    Ok(())
}
