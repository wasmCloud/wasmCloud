//! Blobstore helper for stashing the fetched response body.

use anyhow::{Context, Result, anyhow};

use crate::bindings::wasi::blobstore::{
    blobstore::{Container, create_container, get_container},
    types::OutgoingValue,
};

pub(crate) const CONTAINER_NAME: &str = "http-responses";
pub(crate) const OBJECT_KEY: &str = "example-com-response";

pub(crate) fn store_response_in_blobstore(body: &[u8]) -> Result<()> {
    let container = open_or_create_container(CONTAINER_NAME)?;

    let outgoing = OutgoingValue::new_outgoing_value();
    let stream = outgoing
        .outgoing_value_write_body()
        .map_err(|()| anyhow!("failed to open blobstore output stream"))?;
    stream
        .blocking_write_and_flush(body)
        .context("failed to write blob bytes")?;
    drop(stream);

    container
        .write_data(OBJECT_KEY, &outgoing)
        .map_err(|e| anyhow!("failed to write blob {OBJECT_KEY}: {e}"))?;
    OutgoingValue::finish(outgoing).map_err(|e| anyhow!("failed to finish blob value: {e}"))?;
    Ok(())
}

/// Try to open the container; if that fails, log the original error and
/// attempt to create it. Logging the original keeps an auth/transport failure
/// visible instead of being silently masked by a subsequent create failure.
fn open_or_create_container(name: &str) -> Result<Container> {
    match get_container(name) {
        Ok(c) => Ok(c),
        Err(get_err) => {
            eprintln!(
                "blobstore get_container({name}) failed: {get_err}; attempting create_container"
            );
            create_container(name)
                .map_err(|e| anyhow!("failed to create blobstore container {name}: {e}"))
        }
    }
}
