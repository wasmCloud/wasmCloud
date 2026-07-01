//! # Multiplexed WASI Blobstore (implements-routed)
//!
//! Binds `wasi:blobstore` via the component-model `(implements ..)` /
//! `named_imports` mechanism so a single component can import the same
//! interface multiple times and have each import backed by a *different*
//! backend (e.g. one `wasi:blobstore/blobstore` import backed by the filesystem
//! and another by NATS object-store).
//!
//! Each named `wasi:blobstore/blobstore` import is resolved by the
//! [`Multiplexer`] to a [`BlobBackend`] (the wasmtime "implements id") for the
//! container-lifecycle calls. The `container` and `types` interfaces are bound
//! *standalone* (not per-label): a guest imports them unlabeled via
//! `blobstore`'s `use container`/`use types`, and their resource methods route
//! through the backend stored on the resource (`BlobContainer`/`OutgoingBlob`),
//! so they need no label of their own.
//!
//! The same [`BlobBackend`] trait also backs the async `wasmcloud:blobstore`
//! surface (see [`super::multiplexed_async`]); this module owns the trait,
//! the backends/providers, and the `wasi:blobstore` (wasi:io-based) host layer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use wasmtime::component::Resource;
use wasmtime_wasi::p2::{
    InputStream, OutputStream,
    pipe::{MemoryInputPipe, MemoryOutputPipe},
};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::{BackendProvider, Multiplexer};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

mod bindings {
    wasmtime::component::bindgen!({
        world: "blobstore",
        imports: { default: async | trappable | tracing },
        named_imports: {
            "wasi:blobstore/blobstore": super::BlobId,
            "wasi:blobstore/container": super::BlobId,
            "wasi:blobstore/types": super::BlobId,
        },
        with: {
            "wasi:io": ::wasmtime_wasi_io::bindings::wasi::io,
            "wasi:blobstore/container.container": super::BlobContainer,
            "wasi:blobstore/container.stream-object-names": super::ObjectNameStream,
            "wasi:blobstore/types.incoming-value": super::IncomingBlob,
            "wasi:blobstore/types.outgoing-value": super::OutgoingBlob,
        },
    });
}

use bindings::wasi::blobstore::types::{
    ContainerMetadata, ContainerName, Error as BlobError, ObjectId, ObjectMetadata, ObjectName,
};

/// The "implements id" threaded through every blobstore host method: the backend
/// a given named import is bound to. `Arc` so it is cheaply `Clone`d into each
/// per-import closure, as `named_imports` requires.
pub type BlobId = Arc<dyn BlobBackend>;

/// Default ceiling on the size of a single object written through an
/// `outgoing-value`'s `MemoryOutputPipe`. Generous but bounded so a runaway
/// guest cannot exhaust host memory through one write.
const DEFAULT_MAX_OBJECT_SIZE: usize = 64 * 1024 * 1024;

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_BLOBSTORE_ID: &str = "wasi-blobstore-multiplexed";

/// Backend-agnostic container metadata; converted into the per-surface bindgen
/// types by the host layers.
#[derive(Clone, Debug)]
pub struct ContainerInfo {
    pub name: String,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
}

/// Backend-agnostic object metadata.
#[derive(Clone, Debug)]
pub struct ObjectInfo {
    pub name: String,
    pub container: String,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    pub size: u64,
}

/// The unified error surface for a [`BlobBackend`]. Named cases map cleanly onto
/// the async `wasmcloud:blobstore` `variant error`; the `wasi:blobstore` layer
/// flattens them to its `string` error via [`Display`].
#[derive(Clone, Debug)]
pub enum BlobBackendError {
    NoSuchContainer(String),
    ContainerAlreadyExists(String),
    NoSuchObject(String),
    Other(String),
}

impl BlobBackendError {
    pub fn other(e: impl std::fmt::Display) -> Self {
        Self::Other(e.to_string())
    }
}

impl std::fmt::Display for BlobBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchContainer(c) => write!(f, "container '{c}' does not exist"),
            Self::ContainerAlreadyExists(c) => write!(f, "container '{c}' already exists"),
            Self::NoSuchObject(o) => write!(f, "object '{o}' does not exist"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

type BlobResult<T> = Result<T, BlobBackendError>;

/// A blobstore backend (in-memory, filesystem, NATS object-store, ...).
///
/// Operations are keyed by container name plus object name; the backend owns its
/// own state/connection. Object bodies are passed as whole `Vec<u8>` buffers —
/// the host layers buffer guest streams into memory before handing them off, so
/// backends do not deal with streaming directly. This is the unified surface
/// that both the `wasi:blobstore` and async `wasmcloud:blobstore` host layers
/// dispatch onto.
#[async_trait::async_trait]
pub trait BlobBackend: Send + Sync {
    async fn create_container(&self, name: &str) -> BlobResult<()>;
    async fn get_container(&self, name: &str) -> BlobResult<()>;
    async fn delete_container(&self, name: &str) -> BlobResult<()>;
    async fn container_exists(&self, name: &str) -> BlobResult<bool>;
    async fn container_info(&self, name: &str) -> BlobResult<ContainerInfo>;
    async fn clear_container(&self, name: &str) -> BlobResult<()>;

    /// Read object bytes in the inclusive byte range `[start, end]`, clamped to
    /// the object's length.
    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> BlobResult<Vec<u8>>;
    async fn write_data(&self, container: &str, object: &str, data: Vec<u8>) -> BlobResult<()>;
    async fn list_objects(&self, container: &str) -> BlobResult<Vec<String>>;
    async fn delete_object(&self, container: &str, object: &str) -> BlobResult<()>;
    async fn delete_objects(&self, container: &str, objects: &[String]) -> BlobResult<()>;
    async fn has_object(&self, container: &str, object: &str) -> BlobResult<bool>;
    async fn object_info(&self, container: &str, object: &str) -> BlobResult<ObjectInfo>;
    /// Copy an object. NOTE: both containers are resolved on the *same* backend.
    /// `copy`/`move` cannot cross backends, because the WIT object ids are plain
    /// strings carrying no backend, so a  component that maps two `(implements ..)`
    /// labels to different backends cannot copy between them via this API.
    async fn copy_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> BlobResult<()>;
    async fn move_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> BlobResult<()> {
        self.copy_object(src_container, src_object, dest_container, dest_object)
            .await?;
        self.delete_object(src_container, src_object).await
    }
}

/// A container resource. Remembers the backend it was opened through so every
/// subsequent operation routes to that backend, regardless of which named
/// interface the call arrived on. Shared by the `wasi:blobstore` and async
/// `wasmcloud:blobstore` host layers (each is a distinct WIT resource backed by
/// this same Rust type).
pub struct BlobContainer {
    backend: BlobId,
    name: String,
}

impl BlobContainer {
    pub(crate) fn new(backend: BlobId, name: String) -> Self {
        Self { backend, name }
    }

    pub(crate) fn backend(&self) -> &BlobId {
        &self.backend
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// An `incoming-value` resource: the bytes read by `get-data`, buffered.
pub type IncomingBlob = Vec<u8>;

/// An `outgoing-value` resource: a memory pipe the guest writes into, plus the
/// destination (set at `write-data`) and backend used to persist at `finish`.
pub struct OutgoingBlob {
    pipe: MemoryOutputPipe,
    backend: Option<BlobId>,
    container: Option<String>,
    object: Option<String>,
}

/// A `stream-object-names` resource: a paged snapshot of object names.
pub struct ObjectNameStream {
    names: Vec<String>,
    position: usize,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Clamp a `[start, end]` (inclusive) request to the half-open slice bounds of a
/// buffer of length `len`.
fn clamp_range(start: u64, end: u64, len: usize) -> std::ops::Range<usize> {
    let len = len as u64;
    let start = start.min(len);
    let end_excl = end.saturating_add(1).min(len);
    (start as usize)..(end_excl.max(start) as usize)
}

impl<'a> bindings::named_imports::wasi::blobstore::blobstore::Host for ActiveCtx<'a> {
    async fn create_container(
        &mut self,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<BlobContainer>, BlobError>> {
        if let Err(e) = id.create_container(&name).await {
            return Ok(Err(e.to_string()));
        }
        Ok(Ok(self.table.push(BlobContainer { backend: id, name })?))
    }

    async fn get_container(
        &mut self,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<BlobContainer>, BlobError>> {
        if let Err(e) = id.get_container(&name).await {
            return Ok(Err(e.to_string()));
        }
        Ok(Ok(self.table.push(BlobContainer { backend: id, name })?))
    }

    async fn delete_container(
        &mut self,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        Ok(id.delete_container(&name).await.map_err(|e| e.to_string()))
    }

    async fn container_exists(
        &mut self,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<bool, BlobError>> {
        Ok(id.container_exists(&name).await.map_err(|e| e.to_string()))
    }

    async fn copy_object(
        &mut self,
        id: BlobId,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        Ok(id
            .copy_object(&src.container, &src.object, &dest.container, &dest.object)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn move_object(
        &mut self,
        id: BlobId,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        Ok(id
            .move_object(&src.container, &src.object, &dest.container, &dest.object)
            .await
            .map_err(|e| e.to_string()))
    }
}

impl<'a> bindings::wasi::blobstore::container::HostContainer for ActiveCtx<'a> {
    async fn name(
        &mut self,
        container: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<String, BlobError>> {
        let c = self.table.get(&container)?;
        Ok(Ok(c.name.clone()))
    }

    async fn info(
        &mut self,
        container: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<ContainerMetadata, BlobError>> {
        let c = self.table.get(&container)?;
        match c.backend.container_info(&c.name).await {
            Ok(info) => Ok(Ok(ContainerMetadata {
                name: info.name,
                created_at: info.created_at,
            })),
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    async fn get_data(
        &mut self,
        container: Resource<BlobContainer>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> wasmtime::Result<Result<Resource<IncomingBlob>, BlobError>> {
        let c = self.table.get(&container)?;
        match c.backend.get_data(&c.name, &name, start, end).await {
            Ok(bytes) => Ok(Ok(self.table.push(bytes)?)),
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    async fn write_data(
        &mut self,
        container: Resource<BlobContainer>,
        name: ObjectName,
        data: Resource<OutgoingBlob>,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        let c = self.table.get(&container)?;
        let (backend, container_name) = (c.backend.clone(), c.name.clone());
        let handle = self.table.get_mut(&data)?;
        handle.backend = Some(backend);
        handle.container = Some(container_name);
        handle.object = Some(name);
        Ok(Ok(()))
    }

    async fn list_objects(
        &mut self,
        container: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<Resource<ObjectNameStream>, BlobError>> {
        let c = self.table.get(&container)?;
        match c.backend.list_objects(&c.name).await {
            Ok(names) => Ok(Ok(self
                .table
                .push(ObjectNameStream { names, position: 0 })?)),
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    async fn delete_object(
        &mut self,
        container: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        let c = self.table.get(&container)?;
        Ok(c.backend
            .delete_object(&c.name, &name)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn delete_objects(
        &mut self,
        container: Resource<BlobContainer>,
        names: Vec<ObjectName>,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        let c = self.table.get(&container)?;
        Ok(c.backend
            .delete_objects(&c.name, &names)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn has_object(
        &mut self,
        container: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<bool, BlobError>> {
        let c = self.table.get(&container)?;
        Ok(c.backend
            .has_object(&c.name, &name)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn object_info(
        &mut self,
        container: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<ObjectMetadata, BlobError>> {
        let c = self.table.get(&container)?;
        match c.backend.object_info(&c.name, &name).await {
            Ok(info) => Ok(Ok(ObjectMetadata {
                name: info.name,
                container: info.container,
                created_at: info.created_at,
                size: info.size,
            })),
            Err(e) => Ok(Err(e.to_string())),
        }
    }

    async fn clear(
        &mut self,
        container: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        let c = self.table.get(&container)?;
        Ok(c.backend
            .clear_container(&c.name)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn drop(&mut self, rep: Resource<BlobContainer>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::container::HostStreamObjectNames for ActiveCtx<'a> {
    async fn read_stream_object_names(
        &mut self,
        stream: Resource<ObjectNameStream>,
        len: u64,
    ) -> wasmtime::Result<Result<(Vec<ObjectName>, bool), BlobError>> {
        let s = self.table.get_mut(&stream)?;
        let remaining = s.names.len().saturating_sub(s.position);
        let to_read = (len as usize).min(remaining);
        let page = s
            .names
            .get(s.position..s.position + to_read)
            .unwrap_or_default()
            .to_vec();
        s.position += to_read;
        let is_end = s.position >= s.names.len();
        Ok(Ok((page, is_end)))
    }

    async fn skip_stream_object_names(
        &mut self,
        stream: Resource<ObjectNameStream>,
        num: u64,
    ) -> wasmtime::Result<Result<(u64, bool), BlobError>> {
        let s = self.table.get_mut(&stream)?;
        let remaining = s.names.len().saturating_sub(s.position);
        let to_skip = (num as usize).min(remaining);
        s.position += to_skip;
        let is_end = s.position >= s.names.len();
        Ok(Ok((to_skip as u64, is_end)))
    }

    async fn drop(&mut self, rep: Resource<ObjectNameStream>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::HostOutgoingValue for ActiveCtx<'a> {
    async fn new_outgoing_value(&mut self) -> wasmtime::Result<Resource<OutgoingBlob>> {
        Ok(self.table.push(OutgoingBlob {
            pipe: MemoryOutputPipe::new(DEFAULT_MAX_OBJECT_SIZE),
            backend: None,
            container: None,
            object: None,
        })?)
    }

    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingBlob>,
    ) -> wasmtime::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>>
    {
        let handle = self.table.get_mut(&outgoing_value)?;
        // The guest writes into the same pipe we read back in `finish`.
        let boxed: Box<dyn OutputStream> = Box::new(handle.pipe.clone());
        Ok(Ok(self.table.push(boxed)?))
    }

    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingBlob>,
    ) -> wasmtime::Result<Result<(), BlobError>> {
        let handle = self.table.delete(outgoing_value)?;
        let (Some(backend), Some(container), Some(object)) =
            (handle.backend, handle.container, handle.object)
        else {
            // `finish` without a prior `write-data` has nothing to persist.
            return Ok(Ok(()));
        };
        let data = handle.pipe.contents().to_vec();
        Ok(backend
            .write_data(&container, &object, data)
            .await
            .map_err(|e| e.to_string()))
    }

    async fn drop(&mut self, rep: Resource<OutgoingBlob>) -> wasmtime::Result<()> {
        // Dropping flushes any pending write, matching the standalone backends.
        self.finish(rep).await?.ok();
        Ok(())
    }
}

impl<'a> bindings::wasi::blobstore::types::HostIncomingValue for ActiveCtx<'a> {
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingBlob>,
    ) -> wasmtime::Result<Result<Vec<u8>, BlobError>> {
        let data = self.table.delete(incoming_value)?;
        Ok(Ok(data))
    }

    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingBlob>,
    ) -> wasmtime::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobError>,
    > {
        // `incoming-value-consume-async` consumes `this` (owned, per the WIT), so
        // remove it from the table.
        let data = self.table.delete(incoming_value)?;
        let stream: Box<dyn InputStream> = Box::new(MemoryInputPipe::new(data));
        Ok(Ok(self.table.push(stream)?))
    }

    async fn size(&mut self, incoming_value: Resource<IncomingBlob>) -> wasmtime::Result<u64> {
        Ok(self.table.get(&incoming_value)?.len() as u64)
    }

    async fn drop(&mut self, rep: Resource<IncomingBlob>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

// `Host` marker traits combine the resource-method traits above.
impl<'a> bindings::wasi::blobstore::container::Host for ActiveCtx<'a> {}
impl<'a> bindings::wasi::blobstore::types::Host for ActiveCtx<'a> {}

mod filesystem;
mod in_memory;
mod nats;

pub use filesystem::{FilesystemBackend, FilesystemProvider};
pub use in_memory::{InMemoryBackend, InMemoryProvider};
pub use nats::{NatsBlobBackend, NatsBlobProvider};

/// A blobstore backend provider: a [`BackendProvider`] producing [`BlobId`]s.
pub type BlobProvider = dyn BackendProvider<BlobId>;

/// A blobstore [`HostPlugin`] that multiplexes `wasi:blobstore` across backends
/// selected per `(implements ..)` import. Register the backend providers you
/// want to support via [`MultiplexedBlobstore::with_provider`].
pub struct MultiplexedBlobstore {
    mux: Multiplexer<BlobId>,
}

impl Default for MultiplexedBlobstore {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedBlobstore {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasi", "blobstore", DEFAULT_BACKEND),
        }
    }

    /// Register a backend provider keyed by its `backend_type()`.
    pub fn with_provider(mut self, provider: Arc<BlobProvider>) -> Self {
        self.mux = self.mux.with_provider(provider);
        self
    }

    /// Build the routing registry (host-interface name -> backend) for a
    /// component from its matched blobstore host interfaces.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<HashMap<String, BlobId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedBlobstore {
    fn id(&self) -> &'static str {
        MULTIPLEXED_BLOBSTORE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:blobstore/blobstore,container,types@0.2.0-draft",
            )]),
            ..Default::default()
        }
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasi", "blobstore", &[]) {
            return Ok(());
        }

        let registry = self.build_registry(interfaces.iter()).await?;
        // Clone the component (cheap, Arc-backed) so the immutable borrow ends
        // before we take the mutable linker borrow.
        let component = item.component().clone();
        let linker = item.linker();

        // `blobstore` (create/get container, copy/move) is routed per `(implements
        // ..)` label so each label lands on its own backend. `container` and
        // `types` are bound standalone: a guest imports them *unlabeled* via
        // `blobstore`'s `use container`/`use types` (WIT forbids two interfaces
        // sharing a label), and their methods route through the resource's stored
        // backend (the container/outgoing-value), not a label.
        bindings::named_imports::wasi::blobstore::blobstore::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_iface(name: Option<&str>, backend: Option<&str>, root: Option<&str>) -> WitInterface {
        let mut config = HashMap::new();
        if let Some(b) = backend {
            config.insert("backend".to_string(), b.to_string());
        }
        if let Some(r) = root {
            config.insert("root".to_string(), r.to_string());
        }
        WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["blobstore".to_string()].into_iter().collect(),
            version: None,
            config,
            name: name.map(String::from),
        }
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "wash-fs-blob-{tag}-{}-{unique}",
            std::process::id()
        ))
    }

    /// A backend round-trips a write through `get-data`, and reports object
    /// metadata/listing — the core object lifecycle, proven on in-memory.
    async fn assert_object_lifecycle(backend: &BlobId) {
        backend.create_container("bucket").await.unwrap();
        assert!(backend.container_exists("bucket").await.unwrap());

        backend
            .write_data("bucket", "greeting", b"hello world".to_vec())
            .await
            .unwrap();
        assert!(backend.has_object("bucket", "greeting").await.unwrap());
        assert_eq!(
            backend
                .get_data("bucket", "greeting", 0, u64::MAX)
                .await
                .unwrap(),
            b"hello world".to_vec()
        );
        // Inclusive byte range.
        assert_eq!(
            backend.get_data("bucket", "greeting", 0, 4).await.unwrap(),
            b"hello".to_vec()
        );
        assert_eq!(
            backend
                .object_info("bucket", "greeting")
                .await
                .unwrap()
                .size,
            11
        );
        assert_eq!(
            backend.list_objects("bucket").await.unwrap(),
            vec!["greeting".to_string()]
        );

        backend
            .copy_object("bucket", "greeting", "bucket", "copy")
            .await
            .unwrap();
        assert_eq!(
            backend
                .get_data("bucket", "copy", 0, u64::MAX)
                .await
                .unwrap(),
            b"hello world".to_vec()
        );

        backend.delete_object("bucket", "greeting").await.unwrap();
        assert!(!backend.has_object("bucket", "greeting").await.unwrap());
    }

    #[tokio::test]
    async fn in_memory_object_lifecycle() {
        let backend: BlobId = Arc::new(InMemoryBackend::new());
        assert_object_lifecycle(&backend).await;
    }

    #[tokio::test]
    async fn filesystem_object_lifecycle() {
        let dir = tmp_dir("lifecycle");
        let backend: BlobId = Arc::new(FilesystemBackend::new(&dir));
        assert_object_lifecycle(&backend).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn in_memory_backend_instances_are_isolated() {
        let a = InMemoryBackend::new();
        a.create_container("bucket").await.unwrap();
        a.write_data("bucket", "k", b"v1".to_vec()).await.unwrap();

        let b = InMemoryBackend::new();
        // `b` never saw `bucket`.
        assert!(!b.container_exists("bucket").await.unwrap());
    }

    /// The decisive case: two named interfaces of the *same* backend type route
    /// to independent backends, proven with in-memory.
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedBlobstore::new().with_provider(Arc::new(InMemoryProvider));
        let interfaces = HashSet::from([
            blob_iface(Some("team-a"), Some("in-memory"), None),
            blob_iface(Some("team-b"), Some("in-memory"), None),
        ]);

        let registry = plugin.build_registry(&interfaces).await.unwrap();
        let a = registry.get("team-a").expect("team-a routed").clone();
        let b = registry.get("team-b").expect("team-b routed").clone();

        a.create_container("bucket").await.unwrap();
        a.write_data("bucket", "k", b"v".to_vec()).await.unwrap();
        assert!(
            !b.container_exists("bucket").await.unwrap(),
            "backends leaked"
        );
        assert_eq!(
            a.get_data("bucket", "k", 0, u64::MAX).await.unwrap(),
            b"v".to_vec()
        );
    }

    /// The filesystem provider routes a named interface to an on-disk backend,
    /// proving it's wired into the multiplexer and data actually hits disk.
    #[tokio::test]
    async fn filesystem_backend_persists_through_the_multiplexer() {
        let dir = tmp_dir("mux");
        let iface = blob_iface(
            Some("team-fs"),
            Some("filesystem"),
            Some(&dir.to_string_lossy()),
        );

        let plugin = MultiplexedBlobstore::new().with_provider(Arc::new(FilesystemProvider));
        let registry = plugin
            .build_registry(&HashSet::from([iface]))
            .await
            .unwrap();
        let backend = registry.get("team-fs").expect("team-fs routed").clone();

        backend.create_container("bucket").await.unwrap();
        backend
            .write_data("bucket", "k", b"v".to_vec())
            .await
            .unwrap();
        assert_eq!(
            backend.get_data("bucket", "k", 0, u64::MAX).await.unwrap(),
            b"v".to_vec()
        );
        // It actually hit disk.
        assert!(dir.join("bucket").join("k").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn build_registry_errors_on_unregistered_backend() {
        let plugin = MultiplexedBlobstore::new(); // no providers registered
        let interfaces = HashSet::from([blob_iface(Some("x"), Some("nats"), None)]);
        let err = plugin
            .build_registry(&interfaces)
            .await
            .err()
            .expect("expected error for unregistered backend");
        assert!(err.to_string().contains("nats"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn create_existing_container_errors() {
        let backend = InMemoryBackend::new();
        backend.create_container("dup").await.unwrap();
        assert!(matches!(
            backend.create_container("dup").await,
            Err(BlobBackendError::ContainerAlreadyExists(_))
        ));
    }
}
