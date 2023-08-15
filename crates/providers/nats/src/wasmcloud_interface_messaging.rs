use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wasmcloud_provider_sdk::{
    core::{LinkDefinition, WasmCloudEntity},
    error::ProviderInvocationError,
    Context,
};

/// A message to be published
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PubMessage {
    /// The subject, or topic, of the message
    #[serde(default)]
    pub subject: String,
    /// An optional topic on which the reply should be sent.
    #[serde(rename = "replyTo")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The message payload
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// Reply received from a Request operation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplyMessage {
    /// The subject, or topic, of the message
    #[serde(default)]
    pub subject: String,
    /// An optional topic on which the reply should be sent.
    #[serde(rename = "replyTo")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The message payload
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// Message sent as part of a request, with timeout
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestMessage {
    /// The subject, or topic, of the message
    #[serde(default)]
    pub subject: String,
    /// The message payload
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
    /// A timeout, in milliseconds
    #[serde(rename = "timeoutMs")]
    #[serde(default)]
    pub timeout_ms: u32,
}

/// Message received as part of a subscription
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubMessage {
    /// The subject, or topic, of the message
    #[serde(default)]
    pub subject: String,
    /// An optional topic on which the reply should be sent.
    #[serde(rename = "replyTo")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The message payload
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

pub struct Handler<'a> {
    ld: &'a LinkDefinition,
}

impl<'a> Handler<'a> {
    pub fn new(ld: &'a LinkDefinition) -> Self {
        Self { ld }
    }

    pub async fn handle_message(&self, msg: SubMessage) -> Result<(), ProviderInvocationError> {
        let connection = wasmcloud_provider_sdk::provider_main::get_connection();

        let client = connection.get_rpc_client();
        let origin = WasmCloudEntity {
            public_key: self.ld.provider_id.clone(),
            link_name: self.ld.link_name.clone(),
            contract_id: "wasmcloud:messaging".to_string(),
        };
        let target = WasmCloudEntity {
            public_key: self.ld.actor_id.clone(),
            ..Default::default()
        };

        let data = wasmcloud_provider_sdk::serialize(&msg)?;

        let response = client
            .send(origin, target, "MessageSubscriber.HandleMessage", data)
            .await?;

        if let Some(e) = response.error {
            Err(ProviderInvocationError::Provider(e))
        } else {
            Ok(())
        }
    }
}

/// The Messaging interface describes a service
/// that can deliver messages
/// wasmbus.contractId: wasmcloud:messaging
/// wasmbus.providerReceive
#[async_trait]
pub trait Messaging {
    /// returns the capability contract id for this interface
    fn contract_id() -> &'static str {
        "wasmcloud:messaging"
    }
    /// Publish - send a message
    /// The function returns after the message has been sent.
    /// If the sender expects to receive an asynchronous reply,
    /// the replyTo field should be filled with the
    /// subject for the response.
    async fn publish(&self, ctx: Context, arg: PubMessage) -> Result<(), String>;
    /// Request - send a message in a request/reply pattern,
    /// waiting for a response.
    async fn request(&self, ctx: Context, arg: RequestMessage) -> Result<ReplyMessage, String>;
}
