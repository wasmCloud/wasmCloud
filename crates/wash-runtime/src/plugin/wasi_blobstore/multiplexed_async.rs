//! # Multiplexed async `wasmcloud:blobstore` (implements-routed)
//!
//! The async-native counterpart to [`super::multiplexed`]. Binds
//! `wasmcloud:blobstore@0.1.0` whose object bodies are native component-model
//! `stream<u8>` values (wasip3 style, no `wasi:io`) via the same
//! `(implements ..)` / `named_imports` mechanism, routing each named import to a
//! [`BlobBackend`].
//!
//! Because the WIT methods are `async func`s returning/consuming `stream<u8>`,
//! the generated host traits use wasmtime's *concurrent* ABI: methods are
//! `async fn`s taking an [`Accessor`] (rather than the `&mut ActiveCtx` style of
//! the `wasi:blobstore` layer). `get-data`/`list-objects` build a
//! [`StreamReader`] from a buffered `Vec` (the backend reads whole objects into
//! memory); `write-data` drains the guest's `stream<u8>` into a `Vec` via a
//! [`StreamConsumer`] before handing it to the backend.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{Accessor, Resource, Source, StreamConsumer, StreamReader, StreamResult};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::Multiplexer;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

use super::multiplexed::{BlobBackendError, BlobContainer, BlobId, BlobProvider};

mod bindings {
    wasmtime::component::bindgen!({
        world: "async-blobstore",
        imports: { default: async | trappable | tracing },
        named_imports: {
            "wasmcloud:blobstore/blobstore": crate::plugin::wasi_blobstore::multiplexed::BlobId,
            "wasmcloud:blobstore/container": crate::plugin::wasi_blobstore::multiplexed::BlobId,
        },
        with: {
            "wasmcloud:blobstore/container.container":
                crate::plugin::wasi_blobstore::multiplexed::BlobContainer,
        },
    });
}

use bindings::wasmcloud::blobstore::types::{
    ContainerMetadata, ContainerName, Error as AsyncError, ObjectId, ObjectMetadata, ObjectName,
};

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_ASYNC_BLOBSTORE_ID: &str = "wasmcloud-blobstore-multiplexed";

impl From<BlobBackendError> for AsyncError {
    fn from(e: BlobBackendError) -> Self {
        match e {
            BlobBackendError::NoSuchContainer(_) => AsyncError::NoSuchContainer,
            BlobBackendError::ContainerAlreadyExists(_) => AsyncError::ContainerAlreadyExists,
            BlobBackendError::NoSuchObject(_) => AsyncError::NoSuchObject,
            BlobBackendError::Other(s) => AsyncError::Other(s),
        }
    }
}

/// Read `(backend, container-name)` out of a container resource. Kept tiny so it
/// can run inside an [`Accessor::with`] closure without holding the store borrow
/// across an await.
fn container_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    container: &Resource<BlobContainer>,
) -> wasmtime::Result<(BlobId, String)> {
    let c = access.get().table.get(container)?;
    Ok((c.backend().clone(), c.name().to_string()))
}

impl<T: 'static + Send> bindings::named_imports::wasmcloud::blobstore::blobstore::HostWithStore<T>
    for SharedCtx
{
    async fn create_container(
        accessor: &Accessor<T, Self>,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<BlobContainer>, AsyncError>> {
        if let Err(e) = id.create_container(&name).await {
            return Ok(Err(e.into()));
        }
        let resource = accessor.with(|mut a| a.get().table.push(BlobContainer::new(id, name)))?;
        Ok(Ok(resource))
    }

    async fn get_container(
        accessor: &Accessor<T, Self>,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<Resource<BlobContainer>, AsyncError>> {
        if let Err(e) = id.get_container(&name).await {
            return Ok(Err(e.into()));
        }
        let resource = accessor.with(|mut a| a.get().table.push(BlobContainer::new(id, name)))?;
        Ok(Ok(resource))
    }

    async fn delete_container(
        _accessor: &Accessor<T, Self>,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        Ok(id.delete_container(&name).await.map_err(Into::into))
    }

    async fn container_exists(
        _accessor: &Accessor<T, Self>,
        id: BlobId,
        name: ContainerName,
    ) -> wasmtime::Result<Result<bool, AsyncError>> {
        Ok(id.container_exists(&name).await.map_err(Into::into))
    }

    async fn copy_object(
        _accessor: &Accessor<T, Self>,
        id: BlobId,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        Ok(id
            .copy_object(&src.container, &src.object, &dest.container, &dest.object)
            .await
            .map_err(Into::into))
    }

    async fn move_object(
        _accessor: &Accessor<T, Self>,
        id: BlobId,
        src: ObjectId,
        dest: ObjectId,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        Ok(id
            .move_object(&src.container, &src.object, &dest.container, &dest.object)
            .await
            .map_err(Into::into))
    }
}

impl bindings::named_imports::wasmcloud::blobstore::blobstore::Host for ActiveCtx<'_> {}

// The `container` interface is bound as a *standalone* (non-implements)
// interface rather than per-label. A `wasmcloud:blobstore/blobstore` `use
// container.{container}` drags the `container` interface into the guest as an
// *unlabeled* transitive import (WIT forbids two interfaces sharing one
// `(implements ..)` label), so it cannot be routed per-label. It does not need
// to be: every `BlobContainer` resource already carries the backend it was
// opened through (set by `blobstore.create-container`, which *is* label-routed),
// so the container methods route via the resource regardless of the import.

impl<T: 'static + Send> bindings::wasmcloud::blobstore::container::HostContainerWithStore<T>
    for SharedCtx
{
    async fn name(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<String, AsyncError>> {
        let (_backend, name) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        Ok(Ok(name))
    }

    async fn info(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<ContainerMetadata, AsyncError>> {
        let (backend, name) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        match backend.container_info(&name).await {
            Ok(info) => Ok(Ok(ContainerMetadata {
                name: info.name,
                created_at: info.created_at,
            })),
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn get_data(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> wasmtime::Result<Result<StreamReader<u8>, AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        let bytes = match backend.get_data(&container, &name, start, end).await {
            Ok(bytes) => bytes,
            Err(e) => return Ok(Err(e.into())),
        };
        let reader = accessor.with(|mut a| StreamReader::new(&mut a, bytes))?;
        Ok(Ok(reader))
    }

    async fn write_data(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        name: ObjectName,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        // Drain the guest's stream into memory, then hand the whole object to the
        // backend. `CollectConsumer` sends the collected bytes when the stream
        // ends (it is dropped by the runtime at end-of-stream).
        let (tx, rx) = oneshot::channel::<Vec<u8>>();
        accessor.with(|mut a| {
            data.pipe(
                &mut a,
                CollectConsumer {
                    buf: Vec::new(),
                    done: Some(tx),
                },
            )
        })?;
        let bytes = rx.await.unwrap_or_default();
        Ok(backend
            .write_data(&container, &name, bytes)
            .await
            .map_err(Into::into))
    }

    async fn list_objects(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<StreamReader<ObjectName>, AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        let names = match backend.list_objects(&container).await {
            Ok(names) => names,
            Err(e) => return Ok(Err(e.into())),
        };
        let reader = accessor.with(|mut a| StreamReader::new(&mut a, names))?;
        Ok(Ok(reader))
    }

    async fn delete_object(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        Ok(backend
            .delete_object(&container, &name)
            .await
            .map_err(Into::into))
    }

    async fn delete_objects(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        names: Vec<ObjectName>,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        Ok(backend
            .delete_objects(&container, &names)
            .await
            .map_err(Into::into))
    }

    async fn has_object(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<bool, AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        Ok(backend
            .has_object(&container, &name)
            .await
            .map_err(Into::into))
    }

    async fn object_info(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
        name: ObjectName,
    ) -> wasmtime::Result<Result<ObjectMetadata, AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        match backend.object_info(&container, &name).await {
            Ok(info) => Ok(Ok(ObjectMetadata {
                name: info.name,
                container: info.container,
                created_at: info.created_at,
                size: info.size,
            })),
            Err(e) => Ok(Err(e.into())),
        }
    }

    async fn clear(
        accessor: &Accessor<T, Self>,
        self_: Resource<BlobContainer>,
    ) -> wasmtime::Result<Result<(), AsyncError>> {
        let (backend, container) = accessor.with(|mut a| container_ref(&mut a, &self_))?;
        Ok(backend
            .clear_container(&container)
            .await
            .map_err(Into::into))
    }
}

impl bindings::wasmcloud::blobstore::container::HostContainer for ActiveCtx<'_> {
    async fn drop(&mut self, rep: Resource<BlobContainer>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::wasmcloud::blobstore::container::Host for ActiveCtx<'_> {}

/// A [`StreamConsumer`] that accumulates every byte the guest writes and hands
/// the buffer back once the stream ends. The runtime drops the consumer at
/// end-of-stream, which fires [`Drop`] and delivers the bytes over `done`.
struct CollectConsumer {
    buf: Vec<u8>,
    done: Option<oneshot::Sender<Vec<u8>>>,
}

impl Drop for CollectConsumer {
    fn drop(&mut self) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(std::mem::take(&mut self.buf));
        }
    }
}

impl<D> StreamConsumer<D> for CollectConsumer {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            // No items offered (count == 0). This is an unbounded in-memory sink,
            // so it is always ready to accept. The actual end-of-stream is observed via `Drop`.
            return Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        let n = bytes.len();
        this.buf.extend_from_slice(bytes);
        src.mark_read(n);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// A blobstore [`HostPlugin`] that multiplexes async `wasmcloud:blobstore`
/// across backends selected per `(implements ..)` import. Shares the
/// [`BlobBackend`] providers with [`super::multiplexed::MultiplexedBlobstore`].
pub struct MultiplexedAsyncBlobstore {
    mux: Multiplexer<BlobId>,
}

impl Default for MultiplexedAsyncBlobstore {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedAsyncBlobstore {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasmcloud", "blobstore", DEFAULT_BACKEND),
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
    ) -> anyhow::Result<std::collections::HashMap<String, BlobId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedAsyncBlobstore {
    fn id(&self) -> &'static str {
        MULTIPLEXED_ASYNC_BLOBSTORE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:blobstore/blobstore,container,types@0.1.0",
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
        if !interfaces.contains("wasmcloud", "blobstore", &[]) {
            return Ok(());
        }

        let registry = self.build_registry(interfaces.iter()).await?;
        let component = item.component().clone();
        let linker = item.linker();

        // `blobstore` (create/get/delete container, copy/move) is routed per
        // `(implements ..)` label so each label lands on its own backend.
        bindings::named_imports::wasmcloud::blobstore::blobstore::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |name| self.mux.resolve(&registry, name),
            extract_active_ctx,
        )?;
        // `container` is bound standalone (see the container host impls): a guest
        // imports it *unlabeled* via `blobstore`'s `use container`, and its
        // methods route through the resource's stored backend, not a label.
        bindings::wasmcloud::blobstore::container::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::wasi_blobstore::InMemoryProvider;

    fn blob_iface(name: &str) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["blobstore".to_string()].into_iter().collect(),
            version: None,
            config: std::collections::HashMap::from([(
                "backend".to_string(),
                "in-memory".to_string(),
            )]),
            name: Some(name.to_string()),
        }
    }

    /// The async blobstore plugin routes two named `wasmcloud:blobstore`
    /// interfaces to distinct backends, proving the `("wasmcloud", "blobstore")`
    /// multiplexer is wired and isolation holds over the shared `BlobBackend`.
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedAsyncBlobstore::new().with_provider(Arc::new(InMemoryProvider));
        let interfaces = HashSet::from([blob_iface("team-a"), blob_iface("team-b")]);

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

    #[tokio::test]
    async fn async_error_maps_from_backend_error() {
        assert!(matches!(
            AsyncError::from(BlobBackendError::NoSuchContainer("c".into())),
            AsyncError::NoSuchContainer
        ));
        assert!(matches!(
            AsyncError::from(BlobBackendError::ContainerAlreadyExists("c".into())),
            AsyncError::ContainerAlreadyExists
        ));
        assert!(matches!(
            AsyncError::from(BlobBackendError::NoSuchObject("o".into())),
            AsyncError::NoSuchObject
        ));
        assert!(matches!(
            AsyncError::from(BlobBackendError::Other("x".into())),
            AsyncError::Other(_)
        ));
    }
}
