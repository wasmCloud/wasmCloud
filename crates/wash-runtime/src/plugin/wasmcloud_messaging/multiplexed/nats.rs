//! NATS backend for the multiplexed `wasmcloud:messaging` plugin.
//!
//! A [`MsgBackend`]/[`BackendProvider`] pair backed by an `async_nats` client,
//! serving the outbound (consumer) `publish`/`request` path — the same surface
//! the standalone `NatsMessaging` plugin uses.

use std::collections::HashMap;
use std::sync::Arc;

use crate::plugin::multiplex::BackendProvider;

use super::{BrokerMessage, MsgBackend, MsgError, MsgId};

use crate::plugin::wasmcloud_messaging::nats::{classify_publish, classify_request};

/// A NATS-backed [`MsgBackend`]. The provider pools clients by `url`
/// ([`NatsMsgProvider::pool_key`]), so named imports pointing at the same
/// cluster share one connection while imports with distinct urls get
/// independent clients.
pub struct NatsMsgBackend {
    client: Arc<async_nats::Client>,
}

#[async_trait::async_trait]
impl MsgBackend for NatsMsgBackend {
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: Option<u32>,
    ) -> Result<BrokerMessage, MsgError> {
        // `None` falls through to the NATS client's own request timeout
        // (10s unless configured otherwise), reported as `TimedOut` and
        // classified below.
        let request = self.client.request(subject, body.into());
        let resp = match timeout_ms {
            Some(ms) => {
                let timeout = std::time::Duration::from_millis(ms as u64);
                match tokio::time::timeout(timeout, request).await {
                    Ok(Ok(msg)) => msg,
                    Ok(Err(e)) => return Err(classify_request(&e)),
                    Err(_) => {
                        return Err(MsgError::Timeout(format!("request timed out after {ms}ms")));
                    }
                }
            }
            None => match request.await {
                Ok(msg) => msg,
                Err(e) => return Err(classify_request(&e)),
            },
        };
        Ok(BrokerMessage {
            subject: resp.subject.to_string(),
            reply_to: resp.reply.as_ref().map(|r| r.to_string()),
            body: resp.payload.into(),
        })
    }

    async fn publish(&self, msg: BrokerMessage) -> Result<(), MsgError> {
        let result = if let Some(reply_to) = msg.reply_to {
            self.client
                .publish_with_reply(msg.subject, reply_to, msg.body.into())
                .await
        } else {
            self.client.publish(msg.subject, msg.body.into()).await
        };
        result.map_err(|e| classify_publish(&e))
    }
}

/// NATS provider, selected by `config.backend = "nats"`. Requires `config.url`
/// (e.g. `nats://127.0.0.1:4222`).
#[derive(Default)]
pub struct NatsMsgProvider;

#[async_trait::async_trait]
impl BackendProvider<MsgId> for NatsMsgProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        config.get("url").cloned()
    }
    fn backend_type(&self) -> &'static str {
        "nats"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<MsgId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("nats messaging backend requires a 'url' config"))?;
        let client = async_nats::connect(url).await?;
        Ok(Arc::new(NatsMsgBackend {
            client: Arc::new(client),
        }))
    }
}
