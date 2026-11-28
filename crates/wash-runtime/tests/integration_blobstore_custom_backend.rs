#![cfg(all(
    feature = "wasi-blobstore",
    feature = "wasm_component_model_implements"
))]
//! An embedder plugs its own storage into the multiplexed blobstore plugins by
//! implementing [`BlobBackend`] and registering a [`BackendProvider`] for it.
//!
//! This lives outside the crate on purpose: it only compiles if every type the
//! trait's signatures name is reachable through wash-runtime's public API.
//! The backend scopes each workload-visible container under a host-chosen
//! prefix on an inner store, so it builds the [`ContainerInfo`] and
//! [`ObjectInfo`] it returns rather than passing the inner store's through.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::sync::Arc;

use wash_runtime::plugin::PluginEgressPolicy;
use wash_runtime::plugin::multiplex::BackendProvider;
use wash_runtime::plugin::wasi_blobstore::{
    BlobBackend, BlobBackendError, BlobId, ContainerInfo, InMemoryBackend,
    MultiplexedAsyncBlobstore, MultiplexedBlobstore, ObjectInfo,
};
use wash_runtime::wit::{Config, WitInterface};

const BACKEND_TYPE: &str = "prefixed";

/// Stores every container as `{prefix}{name}` on an inner backend, and reports
/// containers and objects under the names the workload uses.
struct PrefixedBackend {
    prefix: String,
    inner: InMemoryBackend,
}

impl PrefixedBackend {
    fn scoped(&self, container: &str) -> String {
        format!("{}{container}", self.prefix)
    }

    /// Maps an inner error back onto the workload's container name.
    fn unscope(&self, error: BlobBackendError) -> BlobBackendError {
        match error {
            BlobBackendError::NoSuchContainer(name) => BlobBackendError::NoSuchContainer(
                name.strip_prefix(&self.prefix).unwrap_or(&name).to_string(),
            ),
            BlobBackendError::ContainerAlreadyExists(name) => {
                BlobBackendError::ContainerAlreadyExists(
                    name.strip_prefix(&self.prefix).unwrap_or(&name).to_string(),
                )
            }
            other => other,
        }
    }
}

#[async_trait::async_trait]
impl BlobBackend for PrefixedBackend {
    async fn create_container(&self, name: &str) -> Result<(), BlobBackendError> {
        self.inner
            .create_container(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn get_container(&self, name: &str) -> Result<(), BlobBackendError> {
        self.inner
            .get_container(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn delete_container(&self, name: &str) -> Result<(), BlobBackendError> {
        self.inner
            .delete_container(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn container_exists(&self, name: &str) -> Result<bool, BlobBackendError> {
        self.inner
            .container_exists(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn container_info(&self, name: &str) -> Result<ContainerInfo, BlobBackendError> {
        let info = self
            .inner
            .container_info(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))?;
        Ok(ContainerInfo {
            name: name.to_string(),
            created_at: info.created_at,
        })
    }

    async fn clear_container(&self, name: &str) -> Result<(), BlobBackendError> {
        self.inner
            .clear_container(&self.scoped(name))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn get_data(
        &self,
        container: &str,
        object: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobBackendError> {
        self.inner
            .get_data(&self.scoped(container), object, start, end)
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn write_data(
        &self,
        container: &str,
        object: &str,
        data: Vec<u8>,
    ) -> Result<(), BlobBackendError> {
        self.inner
            .write_data(&self.scoped(container), object, data)
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn list_objects(&self, container: &str) -> Result<Vec<String>, BlobBackendError> {
        self.inner
            .list_objects(&self.scoped(container))
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn delete_object(&self, container: &str, object: &str) -> Result<(), BlobBackendError> {
        self.inner
            .delete_object(&self.scoped(container), object)
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn delete_objects(
        &self,
        container: &str,
        objects: &[String],
    ) -> Result<(), BlobBackendError> {
        self.inner
            .delete_objects(&self.scoped(container), objects)
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn has_object(&self, container: &str, object: &str) -> Result<bool, BlobBackendError> {
        self.inner
            .has_object(&self.scoped(container), object)
            .await
            .map_err(|error| self.unscope(error))
    }

    async fn object_info(
        &self,
        container: &str,
        object: &str,
    ) -> Result<ObjectInfo, BlobBackendError> {
        let info = self
            .inner
            .object_info(&self.scoped(container), object)
            .await
            .map_err(|error| self.unscope(error))?;
        Ok(ObjectInfo {
            name: info.name,
            container: container.to_string(),
            created_at: info.created_at,
            size: info.size,
        })
    }

    async fn copy_object(
        &self,
        src_container: &str,
        src_object: &str,
        dest_container: &str,
        dest_object: &str,
    ) -> Result<(), BlobBackendError> {
        self.inner
            .copy_object(
                &self.scoped(src_container),
                src_object,
                &self.scoped(dest_container),
                dest_object,
            )
            .await
            .map_err(|error| self.unscope(error))
    }
}

/// Builds a [`PrefixedBackend`] whose prefix is the binding's `prefix` key.
struct PrefixedProvider;

#[async_trait::async_trait]
impl BackendProvider<BlobId> for PrefixedProvider {
    fn backend_type(&self) -> &'static str {
        BACKEND_TYPE
    }

    async fn instantiate(
        &self,
        config: &Config,
        _egress: Option<&PluginEgressPolicy>,
    ) -> anyhow::Result<BlobId> {
        Ok(Arc::new(PrefixedBackend {
            prefix: config.get("prefix").unwrap_or_default().to_string(),
            inner: InMemoryBackend::new(),
        }))
    }

    fn check_egress(&self, _config: &Config, _policy: &PluginEgressPolicy) -> anyhow::Result<()> {
        Ok(())
    }
}

fn blob_iface(package_namespace: &str, interface: &str, name: &str) -> WitInterface {
    WitInterface {
        namespace: package_namespace.to_string(),
        package: "blobstore".to_string(),
        interfaces: [interface.to_string()].into_iter().collect(),
        version: None,
        config: Config::from_iter([
            ("backend".to_string(), BACKEND_TYPE.to_string()),
            ("prefix".to_string(), "tenant-a/".to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

/// Drives a routed backend through the calls that return [`ContainerInfo`] and
/// [`ObjectInfo`], and checks they carry the workload's names.
async fn assert_reports_unscoped_names(backend: &BlobId) {
    backend.create_container("photos").await.unwrap();
    backend
        .write_data("photos", "cat.png", b"meow".to_vec())
        .await
        .unwrap();

    let container = backend.container_info("photos").await.unwrap();
    assert_eq!(container.name, "photos");

    let object = backend.object_info("photos", "cat.png").await.unwrap();
    assert_eq!(object.name, "cat.png");
    assert_eq!(object.container, "photos");
    assert_eq!(object.size, 4);

    assert!(
        matches!(
            backend.container_info("missing").await,
            Err(BlobBackendError::NoSuchContainer(name)) if name == "missing"
        ),
        "an unknown container should be reported under the workload's name"
    );
}

#[tokio::test]
async fn wasi_blobstore_routes_to_an_out_of_crate_backend() {
    let plugin = MultiplexedBlobstore::new().with_provider(Arc::new(PrefixedProvider));
    let interfaces = HashSet::from([blob_iface("wasi", "blobstore", "media")]);

    let registry = plugin.build_registry(&interfaces).await.unwrap();
    let backend = registry.get("media").expect("media routed");

    assert_reports_unscoped_names(backend).await;
}

#[tokio::test]
async fn wasmcloud_blobstore_routes_to_an_out_of_crate_backend() {
    let plugin = MultiplexedAsyncBlobstore::new().with_provider(Arc::new(PrefixedProvider));
    let interfaces = HashSet::from([blob_iface("wasmcloud", "blobstore", "media")]);

    let registry = plugin.build_registry(&interfaces).await.unwrap();
    let backend = registry.get("media").expect("media routed");

    assert_reports_unscoped_names(backend).await;
}
