use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::sync::Arc;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::plugin::WorkloadTracker;
use crate::wit::{WitInterface, WitWorld};
use anyhow::Context;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::{ObjectStore, ObjectStoreExt};
use tokio::sync::RwLock;
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::pipe::{AsyncWriteStream, MemoryInputPipe};
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

/// Container representation backed by an S3 bucket.
/// Each container maps to a distinct S3 bucket; the `AmazonS3` client
/// inside is configured for that specific bucket.
#[derive(Clone)]
pub struct ContainerData {
    pub name: String,
    pub store: Arc<AmazonS3>,
}

/// Per-workload authorization data, read from the workload interface config.
pub struct WorkloadData {
    /// Which buckets (containers) are accessible to this workload.
    pub buckets: HashSet<String>,
    /// Whether the blobstore is read-only for this workload.
    pub read_only: bool,
    /// Cancellation token for any ongoing operations.
    pub cancel_token: tokio_util::sync::CancellationToken,
}

/// Per-component data holding the environment variables used to build S3
/// clients, plus a cache of already-built clients keyed by bucket name.
pub struct ComponentData {
    pub env: HashMap<String, String>,
    pub stores: HashMap<String, Arc<AmazonS3>>,
}

/// Resource representation for an incoming value (data being read).
pub struct IncomingValueHandle {
    pub data: Vec<u8>,
}

/// Resource representation for an outgoing value (data being written).
/// The actual upload to S3 is deferred until `finish` is called.
pub struct OutgoingValueHandle {
    pub temp_file: tempfile::NamedTempFile,
    pub container: Option<ContainerData>,
    pub object_name: Option<String>,
}

/// Resource representation for streaming object names.
pub struct StreamObjectNamesHandle {
    pub objects: VecDeque<String>,
}

/// S3-backed blobstore plugin using the `object_store` crate.
///
/// Authorization (buckets, read_only) is configured per-workload via interface
/// config in `on_workload_bind`. Each component gets its own set of S3 clients
/// (one per bucket), lazily created from the component's
/// `local_resources.environment` variables:
///
/// | Environment variable      | Description                                             |
/// |---------------------------|---------------------------------------------------------|
/// | `AWS_ACCESS_KEY_ID`       | AWS access key ID                                       |
/// | `AWS_SECRET_ACCESS_KEY`   | AWS secret access key                                   |
/// | `AWS_SESSION_TOKEN`       | Optional session token for temporary credentials        |
/// | `AWS_REGION`              | AWS region (default: `us-east-1`)                       |
/// | `AWS_ENDPOINT_URL`        | Custom S3 endpoint (for MinIO, LocalStack, etc.)        |
/// | `AWS_ALLOW_HTTP`          | Set to `"true"` to allow non-TLS connections            |
/// | `AWS_FORCE_PATH_STYLE`    | Set to `"true"` for path-style access (MinIO, etc.)     |
///
/// The bucket name comes from the container name passed to `create_container`
/// or `get_container`, so a single component can work with multiple buckets.
#[derive(Clone, Default)]
pub struct S3Blobstore {
    tracker: Arc<RwLock<WorkloadTracker<WorkloadData, ComponentData>>>,
}

impl S3Blobstore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check whether the workload is allowed to access the given container.
    /// Returns the cancellation token on success.
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
                    if !data.buckets.contains(container_name) {
                        return None;
                    }
                    if is_write && data.read_only {
                        return None;
                    }
                    return Some(data.cancel_token.clone());
                }
                None
            }
            None => None,
        }
    }

    /// Get or lazily create an S3 client for the given component and bucket.
    /// The client is cached so subsequent calls for the same bucket reuse it.
    async fn get_or_create_store(
        &self,
        component_id: &str,
        bucket: &str,
    ) -> Result<Option<Arc<AmazonS3>>, anyhow::Error> {
        // Fast path: read lock
        {
            let tracker = self.tracker.read().await;
            if let Some(data) = tracker.get_component_data(component_id) {
                if let Some(store) = data.stores.get(bucket) {
                    return Ok(Some(store.clone()));
                }
            } else {
                return Ok(None);
            }
        }

        // Slow path: write lock to build and cache a new store
        let mut tracker = self.tracker.write().await;
        let Some(data) = tracker.get_component_data_mut(component_id) else {
            return Ok(None);
        };

        // Double-check after acquiring write lock
        if let Some(store) = data.stores.get(bucket) {
            return Ok(Some(store.clone()));
        }

        let store = Arc::new(build_s3_from_env(&data.env, bucket)?);
        data.stores.insert(bucket.to_string(), store.clone());
        Ok(Some(store))
    }
}

/// Build an `AmazonS3` client from environment variables for the given bucket.
fn build_s3_from_env(env: &HashMap<String, String>, bucket: &str) -> anyhow::Result<AmazonS3> {
    let mut builder = AmazonS3Builder::new().with_bucket_name(bucket);

    if let Some(region) = env.get("AWS_REGION") {
        builder = builder.with_region(region);
    }

    if let Some(endpoint) = env.get("AWS_ENDPOINT_URL") {
        builder = builder.with_endpoint(endpoint);
    }

    if let Some(access_key) = env.get("AWS_ACCESS_KEY_ID") {
        builder = builder.with_access_key_id(access_key);
    }

    if let Some(secret_key) = env.get("AWS_SECRET_ACCESS_KEY") {
        builder = builder.with_secret_access_key(secret_key);
    }

    if let Some(token) = env.get("AWS_SESSION_TOKEN") {
        builder = builder.with_token(token);
    }

    if env
        .get("AWS_ALLOW_HTTP")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        builder = builder.with_allow_http(true);
    }

    if env
        .get("AWS_FORCE_PATH_STYLE")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        builder = builder.with_virtual_hosted_style_request(false);
    }

    builder.build().context("failed to build S3 client")
}

/// Convert an object name into an `object_store::path::Path`.
fn object_path(object_name: &str) -> object_store::path::Path {
    object_store::path::Path::from(object_name)
}

// Implementation for the main blobstore interface
impl<'a> bindings::wasi::blobstore::blobstore::Host for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin.workload_permit(&self.workload_id, &name, true).await {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        let store = match plugin.get_or_create_store(&self.component_id, &name).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!(
                    "failed to create S3 client for bucket '{name}': {e}"
                )));
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
    ) -> anyhow::Result<Result<Resource<ContainerData>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        let store = match plugin.get_or_create_store(&self.component_id, &name).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!(
                    "failed to create S3 client for bucket '{name}': {e}"
                )));
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
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _workload_permit = match plugin.workload_permit(&self.workload_id, &name, true).await {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized".to_string()));
            }
        };

        let store = match plugin.get_or_create_store(&self.component_id, &name).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!(
                    "failed to create S3 client for bucket '{name}': {e}"
                )));
            }
        };

        // Delete all objects in the bucket.
        let list_result = store.list(None);

        use futures::TryStreamExt;
        let objects: Vec<_> = match list_result.try_collect().await {
            Ok(objects) => objects,
            Err(e) => {
                return Ok(Err(format!("failed to list objects for deletion: {e}")));
            }
        };

        for obj in objects {
            if let Err(e) = store.delete(&obj.location).await {
                return Ok(Err(format!(
                    "failed to delete object '{}': {e}",
                    obj.location
                )));
            }
        }

        Ok(Ok(()))
    }

    #[instrument(skip(self))]
    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        let store = match plugin.get_or_create_store(&self.component_id, &name).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!(
                    "failed to create S3 client for bucket '{name}': {e}"
                )));
            }
        };

        // Try to list with a max of 1 result to see if the bucket is accessible.
        use futures::TryStreamExt;
        match store.list(None).try_next().await {
            Ok(_) => Ok(Ok(true)),
            Err(object_store::Error::NotFound { .. }) => Ok(Ok(false)),
            // Bucket exists but might be empty or accessible
            Err(_) => Ok(Ok(true)),
        }
    }

    #[instrument(skip(self))]
    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        let src_store = match plugin
            .get_or_create_store(&self.component_id, &src.container)
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!("failed to create S3 client: {e}")));
            }
        };

        let src_path = object_path(&src.object);

        if src.container == dest.container {
            // Same bucket: use native S3 copy
            let dest_path = object_path(&dest.object);
            match src_store.copy(&src_path, &dest_path).await {
                Ok(_) => Ok(Ok(())),
                Err(e) => Ok(Err(format!("failed to copy object: {e}"))),
            }
        } else {
            // Cross-bucket: download from src, upload to dest
            let dest_store = match plugin
                .get_or_create_store(&self.component_id, &dest.container)
                .await
            {
                Ok(Some(s)) => s,
                Ok(None) => {
                    return Ok(Err(
                        "S3 client not configured for this component".to_string()
                    ));
                }
                Err(e) => {
                    return Ok(Err(format!("failed to create S3 client: {e}")));
                }
            };

            let data = match src_store.get(&src_path).await {
                Ok(result) => match result.bytes().await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        return Ok(Err(format!("failed to read source object: {e}")));
                    }
                },
                Err(e) => {
                    return Ok(Err(format!("failed to get source object: {e}")));
                }
            };

            let dest_path = object_path(&dest.object);
            match dest_store.put(&dest_path, data.into()).await {
                Ok(_) => Ok(Ok(())),
                Err(e) => Ok(Err(format!("failed to write destination object: {e}"))),
            }
        }
    }

    #[instrument(skip(self))]
    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let _write_permit = match plugin
            .workload_permit(&self.workload_id, &src.container, true)
            .await
        {
            Some(token) => token,
            None => {
                return Ok(Err("unauthorized delete".to_string()));
            }
        };

        // copy_object handles its own permits for dest
        let copy = self.copy_object(src.clone(), dest.clone()).await?;
        if let Err(e) = copy {
            return Ok(Err(format!("failed to copy object during move: {e}")));
        }

        // Delete the source
        let src_store = match plugin
            .get_or_create_store(&self.component_id, &src.container)
            .await
        {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Ok(Err(
                    "S3 client not configured for this component".to_string()
                ));
            }
            Err(e) => {
                return Ok(Err(format!("failed to create S3 client: {e}")));
            }
        };

        let src_path = object_path(&src.object);
        match src_store.delete(&src_path).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!(
                "failed to delete source object after move: {e}"
            ))),
        }
    }
}

impl<'a> bindings::wasi::blobstore::container::HostContainer for ActiveCtx<'a> {
    async fn name(
        &mut self,
        container: Resource<ContainerData>,
    ) -> anyhow::Result<Result<String, ContainerError>> {
        let container_data = self.table.get(&container)?;
        Ok(Ok(container_data.name.clone()))
    }

    async fn info(
        &mut self,
        container: Resource<ContainerData>,
    ) -> anyhow::Result<Result<ContainerMetadata, ContainerError>> {
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
    ) -> anyhow::Result<Result<Resource<IncomingValueHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?;

        let path = object_path(&name);

        let get_result = if start > 0 || end < u64::MAX {
            container_data
                .store
                .get_range(&path, start..end)
                .await
                .map(|bytes| bytes.to_vec())
        } else {
            match container_data.store.get(&path).await {
                Ok(result) => result.bytes().await.map(|b| b.to_vec()),
                Err(e) => Err(e),
            }
        };

        let data = match get_result {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(
                    container = container_data.name,
                    object = name,
                    "Failed to get object from S3: {e}"
                );
                return Ok(Err(format!("object '{name}' does not exist")));
            }
        };

        let resource = self.table.push(IncomingValueHandle { data })?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self, container, data))]
    async fn write_data(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
        data: Resource<OutgoingValueHandle>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        // Prepare the write operation — actual upload happens on `finish`.
        let handle = self.table.get_mut(&data)?;
        handle.container = Some(container_data);
        handle.object_name = Some(name.as_str().to_string());

        Ok(Ok(()))
    }

    #[instrument(skip(self, container))]
    async fn list_objects(
        &mut self,
        container: Resource<ContainerData>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNamesHandle>, ContainerError>> {
        let container_data = self.table.get(&container)?;

        let list_result = container_data.store.list(None);

        use futures::TryStreamExt;
        let objects: Vec<_> = match list_result.try_collect().await {
            Ok(objects) => objects,
            Err(e) => {
                return Ok(Err(format!(
                    "failed to list objects in container '{}': {e}",
                    container_data.name
                )));
            }
        };

        let names: VecDeque<String> = objects
            .into_iter()
            .map(|obj| obj.location.to_string())
            .collect();

        let resource = self
            .table
            .push(StreamObjectNamesHandle { objects: names })?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self, container))]
    async fn delete_object(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        let path = object_path(&name);
        match container_data.store.delete(&path).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to delete object: {e}"))),
        }
    }

    #[instrument(skip(self, container, names))]
    async fn delete_objects(
        &mut self,
        container: Resource<ContainerData>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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
            let path = object_path(&name);
            if let Err(e) = container_data.store.delete(&path).await {
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
    ) -> anyhow::Result<Result<bool, ContainerError>> {
        let container_data = self.table.get(&container)?;
        let path = object_path(&name);

        match container_data.store.head(&path).await {
            Ok(_) => Ok(Ok(true)),
            Err(object_store::Error::NotFound { .. }) => Ok(Ok(false)),
            Err(e) => Ok(Err(format!("failed to check object existence: {e}"))),
        }
    }

    #[instrument(skip(self, container))]
    async fn object_info(
        &mut self,
        container: Resource<ContainerData>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, ContainerError>> {
        let container_data = self.table.get(&container)?;
        let path = object_path(&name);

        match container_data.store.head(&path).await {
            Ok(meta) => Ok(Ok(ObjectMetadata {
                name,
                container: container_data.name.clone(),
                created_at: meta
                    .last_modified
                    .signed_duration_since(chrono::DateTime::UNIX_EPOCH)
                    .num_seconds()
                    .max(0) as u64,
                size: meta.size,
            })),
            Err(e) => Ok(Err(format!("object '{name}' does not exist: {e}"))),
        }
    }

    #[instrument(skip(self, container))]
    async fn clear(
        &mut self,
        container: Resource<ContainerData>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let Some(plugin) = self.get_plugin::<S3Blobstore>(PLUGIN_BLOBSTORE_ID) else {
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

        let list_result = container_data.store.list(None);

        use futures::TryStreamExt;
        let objects: Vec<_> = match list_result.try_collect().await {
            Ok(objects) => objects,
            Err(e) => {
                return Ok(Err(format!(
                    "failed to list objects in container '{}': {e}",
                    container_data.name
                )));
            }
        };

        for obj in objects {
            if let Err(e) = container_data.store.delete(&obj.location).await {
                return Ok(Err(format!(
                    "failed to delete object '{}': {e}",
                    obj.location
                )));
            }
        }

        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<ContainerData>) -> anyhow::Result<()> {
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
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool), ContainerError>> {
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
    ) -> anyhow::Result<Result<(u64, bool), ContainerError>> {
        let stream_handle = self.table.get_mut(&stream)?;

        for i in 0..num {
            if stream_handle.objects.pop_front().is_none() {
                return Ok(Ok((i, true)));
            }
        }

        Ok(Ok((num, false)))
    }

    async fn drop(&mut self, rep: Resource<StreamObjectNamesHandle>) -> anyhow::Result<()> {
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
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueHandle>> {
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
    ) -> anyhow::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>> {
        let handle = self.table.get_mut(&outgoing_value)?;

        let file_wrapper = tokio::fs::File::from_std(handle.temp_file.reopen()?);
        let stream = AsyncWriteStream::new(8192, file_wrapper);

        let boxed: Box<dyn OutputStream> = Box::new(stream);
        let resource = self.table.push(boxed)?;
        Ok(Ok(resource))
    }

    #[instrument(skip(self))]
    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let mut handle = self.table.delete(outgoing_value)?;
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

        handle.temp_file.flush()?;

        // Read the temp file contents and upload to S3
        let data = tokio::fs::read(handle.temp_file.path())
            .await
            .context("failed to read temp file")?;

        let path = object_path(&object_name);

        match container_data
            .store
            .put(&path, bytes::Bytes::from(data).into())
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("failed to write object data to S3: {e}"))),
        }
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueHandle>) -> anyhow::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::HostIncomingValue for ActiveCtx<'a> {
    #[instrument(skip(self))]
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> anyhow::Result<Result<Vec<u8>, BlobstoreError>> {
        let data = self.table.delete(incoming_value)?;
        Ok(Ok(data.data))
    }

    #[instrument(skip(self))]
    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> anyhow::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobstoreError>,
    > {
        let data = self.table.delete(incoming_value)?;

        let stream: Box<dyn InputStream> = Box::new(MemoryInputPipe::new(data.data));
        let stream = self.table.push(stream)?;

        Ok(Ok(stream))
    }

    async fn size(&mut self, incoming_value: Resource<IncomingValueHandle>) -> anyhow::Result<u64> {
        let data = self.table.get(&incoming_value)?;
        Ok(data.data.len() as u64)
    }

    async fn drop(&mut self, rep: Resource<IncomingValueHandle>) -> anyhow::Result<()> {
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

        let env = component_handle.local_resources().environment.clone();

        tracing::debug!(
            workload_id = component_handle.workload_id(),
            component_id = component_handle.id(),
            "Adding S3 blobstore interfaces (per-component credentials from environment)"
        );

        // Store the per-component env vars; S3 clients are created lazily per bucket
        {
            let mut tracker = self.tracker.write().await;
            let workload_id = component_handle.workload_id().to_string();
            let component_id = component_handle.id().to_string();
            let item = tracker.workloads.entry(workload_id).or_insert_with(|| {
                crate::plugin::WorkloadTrackerItem {
                    workload_data: None,
                    components: HashMap::new(),
                }
            });
            item.components.insert(
                component_id.clone(),
                ComponentData {
                    env,
                    stores: HashMap::new(),
                },
            );
            tracker
                .components
                .insert(component_id, component_handle.workload_id().to_string());
        }

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
