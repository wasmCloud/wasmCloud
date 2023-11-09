// You strike me as a message that has never been chunkified
// I'm sure I don't know what you mean, You forget yourself
// There's a million bytes I haven't sent
// but just you wait, just you wait
//
// Where are you taking me?
// I'm about to change your life
//
// The conversation lasted two seconds, maybe three seconds
// Everything we said in total agreement, it's
// A dream and it's a bit of a dance
// A bit of a posture, it's a bit of a stance
// Each chunk gone by as the bytes advance,
// Jetstream came through, with message enhanced!
//
// I will always be chunkified ...

use std::marker::Unpin;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_nats::jetstream;
use async_nats::jetstream::object_store::{self, ObjectStore};
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::{debug, error, instrument};

/// Amount of time to add to rpc timeout if chunkifying
pub const CHUNK_RPC_EXTRA_TIME: Duration = Duration::from_secs(13);

/// Maximum size of a message payload before it will be chunked
/// Nats currently uses 128kb chunk size so this should be at least 128KB
#[cfg(not(test))]
pub const CHUNK_THRESHOLD_BYTES: usize = 1024 * 900; // 900KB
#[cfg(test)]
pub const CHUNK_THRESHOLD_BYTES: usize = 1024; // 1KB

#[derive(Clone, Debug)]
pub struct ChunkEndpoint {
    lattice: String,
    js: jetstream::Context,
}

impl ChunkEndpoint {
    #[must_use]
    pub fn new(lattice: &str, js: jetstream::Context) -> Self {
        ChunkEndpoint {
            lattice: lattice.to_string(),
            js,
        }
    }

    pub fn with_client(
        lattice: &str,
        nc: async_nats::Client,
        domain: Option<impl AsRef<str>>,
    ) -> Self {
        let js = if let Some(domain) = domain {
            jetstream::with_domain(nc, domain)
        } else {
            jetstream::new(nc)
        };
        ChunkEndpoint::new(lattice, js)
    }

    /// load the message after de-chunking
    #[instrument(level = "trace", skip(self))]
    pub async fn get_unchunkified(&self, inv_id: &str) -> anyhow::Result<Vec<u8>> {
        let mut result = Vec::new();
        let store = self
            .create_or_reuse_store()
            .await
            .context("failed to get object store")?;
        debug!(invocation_id = %inv_id, "chunkify starting to receive");
        let mut obj = store
            .get(inv_id)
            .await
            .context("failed to receive chunked stream")?;
        obj.read_to_end(&mut result)
            .await
            .context("failed to read chunked stream")?;
        if let Err(err) = store.delete(inv_id).await {
            // not deleting will be a non-fatal error for the receiver,
            // if all the bytes have been received
            error!(invocation_id = %inv_id, %err, "failed to delete chunks");
        }
        Ok(result)
    }

    /// load response after de-chunking
    #[allow(clippy::missing_errors_doc)] // TODO: Document errors
    pub async fn get_unchunkified_response(&self, inv_id: &str) -> anyhow::Result<Vec<u8>> {
        // responses are stored in the object store with '-r' suffix on the object name
        self.get_unchunkified(&format!("{inv_id}-r")).await
    }

    /// chunkify a message
    #[instrument(level = "trace", skip(self, bytes))]
    pub async fn chunkify(
        &self,
        inv_id: &str,
        mut bytes: (impl AsyncRead + Unpin),
    ) -> anyhow::Result<()> {
        let store = self.create_or_reuse_store().await?;
        debug!(invocation_id = %inv_id, "chunkify starting to send");
        let info = store
            .put(inv_id, &mut bytes)
            .await
            .map_err(|e| anyhow!(e))
            .context("error when writing chunkified data for {inv_id}")?;
        debug!(?info, invocation_id = %inv_id, "chunkify completed writing");

        Ok(())
    }

    /// chunkify a portion of a response
    #[allow(clippy::missing_errors_doc)] // TODO: Document errors
    pub async fn chunkify_response(
        &self,
        inv_id: &str,
        bytes: (impl AsyncRead + Unpin),
    ) -> anyhow::Result<()> {
        self.chunkify(&format!("{inv_id}-r"), bytes).await
    }

    // TODO: cache the store locally
    async fn create_or_reuse_store(&self) -> anyhow::Result<ObjectStore> {
        let store = match self.js.get_object_store(&self.lattice).await {
            Ok(store) => store,
            Err(_) => self
                .js
                .create_object_store(object_store::Config {
                    bucket: self.lattice.clone(),
                    ..Default::default()
                })
                .await
                .map_err(|e| anyhow!(e))
                .context("Failed to create chunking store")?,
        };
        Ok(store)
    }
}
