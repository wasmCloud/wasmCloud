//! In-memory loopback backend for the multiplexed `wasmcloud:messaging` plugin.
//!
//! A reference [`MsgBackend`] needing no external infra: it records published
//! messages and answers `request` by echoing it back as its own reply. Used to
//! prove routing and for tests; each instance is isolated, so two named imports
//! backed by two `InMemoryMsgBackend`s do not share published messages.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::plugin::multiplex::BackendProvider;

use super::{BrokerMessage, MsgBackend, MsgId};

/// An in-memory [`MsgBackend`] that records published messages and answers
/// `request` by echoing it back as its own reply. Each instance is isolated.
#[derive(Default)]
pub struct InMemoryMsgBackend {
    published: RwLock<Vec<BrokerMessage>>,
}

impl InMemoryMsgBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Messages published through this backend, in order.
    pub async fn published(&self) -> Vec<BrokerMessage> {
        self.published.read().await.clone()
    }
}

#[async_trait::async_trait]
impl MsgBackend for InMemoryMsgBackend {
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        _timeout_ms: u32,
    ) -> Result<BrokerMessage, String> {
        // Loopback: echo the request back as its own reply.
        self.published.write().await.push(BrokerMessage {
            subject: subject.clone(),
            reply_to: None,
            body: body.clone(),
        });
        Ok(BrokerMessage {
            subject,
            reply_to: None,
            body,
        })
    }

    async fn publish(&self, msg: BrokerMessage) -> Result<(), String> {
        self.published.write().await.push(msg);
        Ok(())
    }
}

/// In-memory provider, the default backend type. Each named interface gets its
/// own isolated loopback (it never pools).
#[derive(Default)]
pub struct InMemoryMsgProvider;

#[async_trait::async_trait]
impl BackendProvider<MsgId> for InMemoryMsgProvider {
    fn backend_type(&self) -> &'static str {
        super::DEFAULT_BACKEND
    }

    async fn instantiate(&self, _config: &HashMap<String, String>) -> anyhow::Result<MsgId> {
        Ok(Arc::new(InMemoryMsgBackend::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brokered(subject: &str, body: &[u8]) -> BrokerMessage {
        BrokerMessage {
            subject: subject.to_string(),
            reply_to: None,
            body: body.to_vec(),
        }
    }

    #[tokio::test]
    async fn in_memory_backend_records_publishes() {
        let b = InMemoryMsgBackend::new();
        b.publish(brokered("a", b"1")).await.unwrap();
        b.publish(brokered("b", b"2")).await.unwrap();
        let published = b.published().await;
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].subject, "a");
        assert_eq!(published[1].subject, "b");
    }
}
