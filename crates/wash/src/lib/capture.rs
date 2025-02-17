use std::convert::TryFrom;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::Result;
use async_compression::tokio::{bufread::GzipDecoder, write::GzipEncoder};
use async_nats::HeaderMap;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};
use tokio_tar::Archive;
use wasmcloud_control_interface::HostInventory;

pub const INVENTORY_FILE: &str = "inventory.json";
pub const MESSAGES_DIR: &str = "messages";

/// A subset of NATS message info that we need to serialize for now. Basically it is all the types that easily
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableMessage {
    pub subject: String,
    pub reply: Option<String>,
    pub payload: bytes::Bytes,
    pub description: Option<String>,
    pub length: usize,
    pub published: time::OffsetDateTime,
    pub headers: Option<HeaderMap>,
}

impl TryFrom<async_nats::jetstream::Message> for SerializableMessage {
    type Error = anyhow::Error;

    fn try_from(msg: async_nats::jetstream::Message) -> Result<Self, Self::Error> {
        let published = msg.info().map_err(|e| anyhow::anyhow!("{e:?}"))?.published;
        Ok(Self {
            subject: msg.message.subject.to_string(),
            reply: msg.message.reply.map(|s| s.to_string()),
            payload: msg.message.payload,
            description: msg.message.description,
            length: msg.message.length,
            headers: msg.message.headers,
            published,
        })
    }
}

/// A read capture is a parsed tarball that contains all of the messages and inventory for a given
/// capture.
///
/// Currently this only loads the data into memory, but we might add additional helper methods in
/// the future.
///
/// NOTE: The interior structure of the tarball is not a guaranteed API and may change in the
/// future. All interactions should be done through this type
pub struct ReadCapture {
    pub inventory: Vec<HostInventory>,
    // NOTE: A further optimization would be to only load based off of a filter here rather than
    // possibly having thousands of messages
    pub messages: Vec<SerializableMessage>,
}

impl ReadCapture {
    /// Loads the given capture file from the path and returns all of the data
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(&path).await.map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "failed to load capture from file [{}]: {e}",
                    path.as_ref().display()
                ),
            )
        })?;
        let mut archive = Archive::new(GzipDecoder::new(tokio::io::BufReader::new(file)));

        let mut capture = Self {
            inventory: Vec::new(),
            messages: Vec::new(),
        };
        let mut entries = archive.entries()?;
        while let Some(mut entry) = entries.try_next().await? {
            let path = entry.path()?;
            if path.file_name().unwrap_or_default() == INVENTORY_FILE {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).await?;
                // We can't use a reader because it is async
                capture.inventory = serde_json::from_slice(&buf)?;
            } else if path
                .parent()
                .and_then(|p| p.file_name())
                .unwrap_or_default()
                == MESSAGES_DIR
                && path.extension().unwrap_or_default() == "json"
            {
                // For any path that matches messages/*.json, we want to read the file and
                // deserialize it. Ordering should be the same as we wrote it (in order), so reading
                // out should be ok
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf).await?;
                let msg: SerializableMessage = serde_json::from_slice(&buf)?;
                capture.messages.push(msg);
            }
        }
        Ok(capture)
    }
}

pub struct WriteCapture {
    builder: tokio_tar::Builder<GzipEncoder<File>>,
    current_index: usize,
}

impl WriteCapture {
    /// Create a new `WriteCapture` that will write the capture tarball to the given path with the
    /// expected inventory
    pub async fn start(inventory: Vec<HostInventory>, path: impl AsRef<Path>) -> Result<Self> {
        let file = File::create(path).await?;
        let encoder = GzipEncoder::new(file);
        let mut builder = tokio_tar::Builder::new(encoder);
        // We always start by encoding the inventory first
        let inventory_data = serde_json::to_vec(&inventory)?;
        let mut header = tokio_tar::Header::new_gnu();
        header.set_size(inventory_data.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, INVENTORY_FILE, Cursor::new(inventory_data))
            .await?;
        Ok(Self {
            builder,
            current_index: 0,
        })
    }

    /// Adds an observed message to the capture
    pub async fn add_message(&mut self, msg: SerializableMessage) -> Result<()> {
        // NOTE(thomastaylor312): If encoding in json becomes a bottleneck, we can switch to a more
        // efficient format, but I figured this could be easier for people to read if someone
        // unpacks the message themselves
        let data = serde_json::to_vec(&msg)?;
        let mut header = tokio_tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_cksum();
        // Name of the file is an incrementing index and the timestamp of the message
        let path = PathBuf::from(MESSAGES_DIR).join(format!(
            "{}-{}.json",
            self.current_index,
            msg.published
                .format(&time::format_description::well_known::Rfc3339)?
        ));
        self.builder
            .append_data(&mut header, path, Cursor::new(data))
            .await?;
        self.current_index += 1;
        Ok(())
    }

    /// Marks the tarball write as complete and flushes the underlying writer to disk
    pub async fn finish(self) -> Result<()> {
        let mut encoder = self.builder.into_inner().await?;
        encoder.flush().await?;
        encoder.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_roundtrip() {
        let tempdir = tempfile::tempdir().unwrap();
        let tarball = tempdir.path().join("capture.tar.gz");
        let mut capture = WriteCapture::start(
            vec![HostInventory::builder()
                .host_id("test".into())
                .friendly_name("test".into())
                .version("1.0.0".into())
                .uptime_human("t".into())
                .uptime_seconds(100)
                .build()
                .expect("failed to build host inventory")],
            &tarball,
        )
        .await
        .expect("Should be able to start a capture");
        capture
            .add_message(SerializableMessage {
                subject: "first".to_string(),
                reply: None,
                payload: bytes::Bytes::from("test"),
                description: None,
                length: 5,
                published: time::OffsetDateTime::now_utc(),
                headers: None,
            })
            .await
            .expect("Should be able to add a message");
        capture
            .add_message(SerializableMessage {
                subject: "second".to_string(),
                reply: None,
                payload: bytes::Bytes::from("test"),
                description: None,
                length: 6,
                published: time::OffsetDateTime::now_utc(),
                headers: None,
            })
            .await
            .expect("Should be able to add a message");

        capture
            .finish()
            .await
            .expect("Should be able to finish a capture");

        let capture = ReadCapture::load(&tarball)
            .await
            .expect("Should be able to load a capture");

        assert_eq!(
            capture.inventory.len(),
            1,
            "Should have the correct inventory"
        );
        assert_eq!(
            capture.inventory[0].host_id(),
            "test",
            "Should have the correct inventory"
        );
        assert_eq!(
            capture.messages.len(),
            2,
            "Should have the right amount of messages"
        );
        assert_eq!(
            capture.messages[0].subject, "first",
            "Should have the right ordering"
        );
        assert_eq!(
            capture.messages[1].subject, "second",
            "Should have the right ordering"
        );
    }
}
