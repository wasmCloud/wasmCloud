//! The storage layer over async `wasmcloud:blobstore`. Everything the registry
//! persists lives as an object in a single container, keyed by repository name
//! (see [`crate::keys`] for the naming scheme).

use crate::Container;
use crate::bindings;
use crate::bindings::wasmcloud::blobstore::blobstore;
use crate::bindings::wasmcloud::blobstore::types::{Error as BlobError, ObjectId};

/// All registry objects live in a single blobstore container, keyed by
/// repository name (e.g. `library/nginx/blobs/sha256_<hex>`).
const CONTAINER_NAME: &str = "oci-registry";

/// Fetch (or create) the registry container.
pub(crate) async fn ensure_container() -> Result<Container, String> {
    if blobstore::container_exists(CONTAINER_NAME.to_string())
        .await
        .map_err(blob_err)?
    {
        return blobstore::get_container(CONTAINER_NAME.to_string())
            .await
            .map_err(blob_err);
    }
    match blobstore::create_container(CONTAINER_NAME.to_string()).await {
        Ok(container) => Ok(container),
        // A concurrent request created it between the check and here (e.g. oras
        // uploads a manifest's blobs in parallel); fetch the existing one.
        Err(BlobError::ContainerAlreadyExists) => {
            blobstore::get_container(CONTAINER_NAME.to_string())
                .await
                .map_err(blob_err)
        }
        Err(e) => Err(blob_err(e)),
    }
}

pub(crate) async fn object_size(container: &Container, key: &str) -> Result<u64, String> {
    Ok(container
        .object_info(key.to_string())
        .await
        .map_err(blob_err)?
        .size)
}

pub(crate) async fn has_object(container: &Container, key: &str) -> Result<bool, String> {
    container
        .has_object(key.to_string())
        .await
        .map_err(blob_err)
}

pub(crate) async fn delete_object(container: &Container, key: &str) -> Result<(), String> {
    container
        .delete_object(key.to_string())
        .await
        .map_err(blob_err)
}

pub(crate) async fn list_keys(container: &Container) -> Result<Vec<String>, String> {
    let stream = container.list_objects().await.map_err(blob_err)?;
    Ok(stream.collect().await)
}

/// Read an entire object into memory. Returns `None` if the object is absent.
pub(crate) async fn read_object(
    container: &Container,
    key: &str,
) -> Result<Option<Vec<u8>>, String> {
    if !container
        .has_object(key.to_string())
        .await
        .map_err(blob_err)?
    {
        return Ok(None);
    }
    let size = container
        .object_info(key.to_string())
        .await
        .map_err(blob_err)?
        .size;
    if size == 0 {
        return Ok(Some(Vec::new()));
    }
    // Offsets are inclusive, so the last byte is at `size - 1`.
    let stream = container
        .get_data(key.to_string(), 0, size - 1)
        .await
        .map_err(blob_err)?;
    Ok(Some(stream.collect().await))
}

/// Open a blobstore object as a byte stream for forwarding into a response body
/// (a 0-byte object yields an already-closed empty stream).
pub(crate) async fn open_object_stream(
    container: &Container,
    key: &str,
    size: u64,
) -> Result<wit_bindgen::StreamReader<u8>, String> {
    if size == 0 {
        let (writer, reader) = bindings::wit_stream::new();
        drop(writer);
        return Ok(reader);
    }
    // Offsets are inclusive, so the last byte is at `size - 1`.
    container
        .get_data(key.to_string(), 0, size - 1)
        .await
        .map_err(blob_err)
}

/// Create or replace an object, streaming `data` to the blobstore.
pub(crate) async fn write_object(
    container: &Container,
    key: &str,
    data: Vec<u8>,
) -> Result<(), String> {
    let (mut tx, rx) = bindings::wit_stream::new();
    wit_bindgen::spawn_local(async move {
        if !data.is_empty() {
            tx.write_all(data).await;
        }
        drop(tx);
    });
    container
        .write_data(key.to_string(), rx)
        .await
        .map_err(blob_err)
}

/// Copy an object within the registry container (used by cross-repo mount).
pub(crate) async fn copy_object(source: &str, dest: &str) -> Result<(), String> {
    blobstore::copy_object(object_id(source), object_id(dest))
        .await
        .map_err(blob_err)
}

/// An `object-id` in the single registry container.
fn object_id(object: &str) -> ObjectId {
    ObjectId {
        container: CONTAINER_NAME.to_string(),
        object: object.to_string(),
    }
}

pub(crate) fn blob_err(error: BlobError) -> String {
    match error {
        BlobError::NoSuchContainer => "no such container".to_string(),
        BlobError::ContainerAlreadyExists => "container already exists".to_string(),
        BlobError::NoSuchObject => "no such object".to_string(),
        BlobError::AccessDenied => "access denied".to_string(),
        BlobError::Timeout => "blobstore timeout".to_string(),
        BlobError::StoreUnavailable => "blobstore unavailable".to_string(),
        BlobError::QuotaExceeded => "blobstore quota exceeded".to_string(),
        BlobError::Other(message) => message,
    }
}
