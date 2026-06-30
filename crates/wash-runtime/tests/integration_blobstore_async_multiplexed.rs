#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end test for the multiplexed async `wasmcloud:blobstore` NATS backend,
//! routed through `MultiplexedAsyncBlobstore`'s provider/registry path.
//!
//! Builds a registry from a named `wasmcloud:blobstore` host interface with
//! `backend = nats`, then drives the *real* NATS JetStream object-store backend
//! through the full `BlobBackend` surface (container lifecycle, object
//! read/write, listing, copy, delete, clear), asserting the named import routed
//! to a working object store.
//!
//! Requires Docker (NATS with JetStream); marked `#[ignore]`, so it runs only
//! under `cargo test --include-ignored` (CI's Linux leg) and not a plain
//! `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::wasi_blobstore::{MultiplexedAsyncBlobstore, NatsBlobProvider};
use wash_runtime::wit::WitInterface;

/// A named `wasmcloud:blobstore` interface routed to the NATS backend.
fn nats_blob_iface(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "blobstore".to_string(),
        interfaces: ["blobstore".to_string(), "container".to_string()]
            .into_iter()
            .collect(),
        version: None,
        config: HashMap::from([
            ("backend".to_string(), "nats".to_string()),
            ("url".to_string(), url.to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

/// `BlobBackendError` is not `std::error::Error`; stringify it for `?`/`anyhow`.
fn err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("blobstore backend error: {e:?}")
}

#[tokio::test]
#[ignore = "requires Docker (NATS JetStream); run with `cargo test --include-ignored`"]
async fn async_multiplexed_blobstore_routes_to_nats() -> Result<()> {
    // --- NATS container (JetStream enabled for the object store) ---
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_port = nats.get_host_port_ipv4(4222).await?;
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // --- build the routing registry from the named host interface ---
    let plugin = MultiplexedAsyncBlobstore::new().with_provider(Arc::new(NatsBlobProvider));
    let registry = plugin
        .build_registry(&[nats_blob_iface("nats-blob", &nats_url)])
        .await
        .context("build registry")?;
    let be = registry.get("nats-blob").expect("nats-blob routed").clone();

    // --- container lifecycle ---
    be.create_container("photos").await.map_err(err)?;
    assert!(be.container_exists("photos").await.map_err(err)?);
    // Creating an existing container is an error.
    assert!(
        be.create_container("photos").await.is_err(),
        "re-creating a container must error"
    );

    // --- object write / read round-trip ---
    be.write_data("photos", "cat.png", b"meow".to_vec())
        .await
        .map_err(err)?;
    assert!(be.has_object("photos", "cat.png").await.map_err(err)?);
    assert_eq!(
        be.get_data("photos", "cat.png", 0, u64::MAX)
            .await
            .map_err(err)?,
        b"meow".to_vec()
    );
    // Inclusive byte range [0, 1].
    assert_eq!(
        be.get_data("photos", "cat.png", 0, 1).await.map_err(err)?,
        b"me".to_vec()
    );
    assert_eq!(
        be.object_info("photos", "cat.png").await.map_err(err)?.size,
        4
    );

    // --- listing ---
    be.write_data("photos", "dog.png", b"woof".to_vec())
        .await
        .map_err(err)?;
    let mut names = be.list_objects("photos").await.map_err(err)?;
    names.sort();
    assert_eq!(names, vec!["cat.png".to_string(), "dog.png".to_string()]);

    // --- copy ---
    be.copy_object("photos", "cat.png", "photos", "cat-copy.png")
        .await
        .map_err(err)?;
    assert_eq!(
        be.get_data("photos", "cat-copy.png", 0, u64::MAX)
            .await
            .map_err(err)?,
        b"meow".to_vec()
    );

    // --- delete + clear ---
    be.delete_object("photos", "dog.png").await.map_err(err)?;
    assert!(!be.has_object("photos", "dog.png").await.map_err(err)?);
    // Deleting a missing object is idempotent.
    be.delete_object("photos", "dog.png").await.map_err(err)?;

    be.clear_container("photos").await.map_err(err)?;
    assert!(be.list_objects("photos").await.map_err(err)?.is_empty());

    // --- container deletion ---
    be.delete_container("photos").await.map_err(err)?;
    assert!(!be.container_exists("photos").await.map_err(err)?);
    // Reading a deleted container surfaces an error.
    assert!(
        be.get_data("photos", "cat.png", 0, u64::MAX).await.is_err(),
        "reading from a deleted container must error"
    );

    Ok(())
}
