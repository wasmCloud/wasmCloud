//! Object storage related functionality and helper methods

use anyhow::{anyhow, Result};
use bytes::Bytes;

use crate::blobstore;
use crate::blobstore::Container;
use crate::wasi::blobstore::types::{IncomingValue, OutgoingValue};
use crate::wasi::logging::logging::{log, Level};
use crate::wasmcloud::bus::lattice::{self, CallTargetInterface};
use crate::MAX_WRITE_BYTES;

/// A helper that will automatically create a container if it doesn't exist and returns an owned copy of the name for immediate use
pub(crate) fn ensure_container(name: &String) -> Result<Container> {
    if blobstore::container_exists(name)
        .map_err(|e| anyhow!("error checking for container: {e}"))?
    {
        return blobstore::get_container(name).map_err(|e| anyhow!("failed to get container: {e}"));
    }
    log(
        Level::Info,
        "handle",
        format!("creating missing container/bucket [{name}]").as_str(),
    );
    blobstore::create_container(name).map_err(|e| anyhow!("failed to create container: {e}"))
}

/// Write a binary blob to object storage
pub(crate) fn write_object(
    object_bytes: Bytes,
    link_name: &str,
    bucket: &str,
    key: &String,
) -> Result<()> {
    lattice::set_link_name(
        &link_name,
        vec![CallTargetInterface::new("wasi", "blobstore", "blobstore")],
    );

    let container = ensure_container(&String::from(bucket))?;

    let data = OutgoingValue::new_outgoing_value();
    let data_body = data
        .outgoing_value_write_body()
        .map_err(|()| anyhow!("failed to get data output stream"))?;
    for chunk in object_bytes.chunks(MAX_WRITE_BYTES) {
        data_body
            .blocking_write_and_flush(chunk)
            .map_err(|e| anyhow!("failed to write chunk: {e}"))?;
    }
    container
        .write_data(key, &data)
        .map_err(|e| anyhow!("failed to write data: {e}"))?;

    lattice::set_link_name(
        "default",
        vec![CallTargetInterface::new("wasi", "blobstore", "blobstore")],
    );

    Ok(())
}

/// Read a binary blob from object storage
pub(crate) fn read_object(link_name: &str, bucket: &str, key: &str) -> Result<Bytes> {
    lattice::set_link_name(
        &link_name,
        vec![CallTargetInterface::new("wasi", "blobstore", "blobstore")],
    );

    let key = &String::from(key);
    let container = ensure_container(&String::from(bucket))?;
    let metadata = container
        .object_info(key)
        .map_err(|e| anyhow!("failed to get object metadata: {e}"))?;
    let incoming = container
        .get_data(key, 0, metadata.size)
        .map_err(|e| anyhow!("failed to get data: {e}"))?;
    let body = IncomingValue::incoming_value_consume_sync(incoming)
        .map_err(|e| anyhow!("failed to consume incoming value: {e}"))?;

    lattice::set_link_name(
        "default",
        vec![CallTargetInterface::new("wasi", "blobstore", "blobstore")],
    );

    Ok(Bytes::from(body))
}
