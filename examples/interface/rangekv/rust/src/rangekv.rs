// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
#[allow(unused_imports)]
use async_trait::async_trait;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::borrow::Cow;
#[allow(unused_imports)]
use wasmbus_rpc::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};

pub const SMITHY_VERSION: &str = "1.0";

/// BlobValue is a non-empty byte array
pub type BlobValue = Vec<u8>;

/// Key is any non-empty UTF-8 string
pub type Key = String;

/// A list of keys
pub type KeyList = Vec<Key>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyRangeResponse {
    /// first key in range returned
    #[serde(rename = "startKey")]
    pub start_key: String,
    /// number of items returned
    pub count: u32,
    /// values returned
    pub items: KeyList,
    /// startKey that should be used on the next request
    /// If this value is empty, there are no more keys
    #[serde(rename = "nextKey")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_key: Option<String>,
}

/// A structure containing a key and value
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: Key,
    pub value: BlobValue,
}

/// A list of key-value pairs
pub type KeyValueList = Vec<KeyValue>;

/// result of Values range query
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueRangeResponse {
    /// startKey that should be used on the next request
    /// If this value is empty, there are no more keys
    #[serde(rename = "nextKey")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_key: Option<String>,
    /// number of items returned
    pub count: u32,
    /// returned list of key-value pairs
    pub items: KeyValueList,
    /// first key in range returned
    #[serde(rename = "startKey")]
    pub start_key: String,
}

/// Structure that contains an optional value
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaybeValue {
    /// a value or none
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<BlobValue>,
}

/// Input a range request (Keys or Values)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RangeRequest {
    /// optional last key of the requested range (inclusive)
    #[serde(rename = "lastKey")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_key: Option<String>,
    /// maximum number of values to return
    /// the server may return fewer than this value.
    pub limit: u32,
    /// the initial key at start of range
    #[serde(rename = "startKey")]
    pub start_key: String,
}

/// wasmbus.contractId: wasmcloud::example:rangekv
/// wasmbus.providerReceive
#[async_trait]
pub trait RangeKeyValue {
    /// Gets a value for a specified key.
    async fn get(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<MaybeValue, RpcError>;
    /// Stores a value. Replaces an existing value of the same key.
    async fn put(&self, ctx: &context::Context<'_>, arg: &KeyValue) -> Result<(), RpcError>;
    /// Deletes a value if it exists.
    async fn delete(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<(), RpcError>;
    /// Clears all keys
    async fn clear(&self, ctx: &context::Context<'_>) -> Result<(), RpcError>;
    /// Returns true if the value is contained in the store
    async fn contains(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<bool, RpcError>;
    /// Returns a range of keys
    async fn keys(
        &self,
        ctx: &context::Context<'_>,
        arg: &RangeRequest,
    ) -> Result<KeyRangeResponse, RpcError>;
    /// Returns a range of key-value pairs
    async fn values(
        &self,
        ctx: &context::Context<'_>,
        arg: &RangeRequest,
    ) -> Result<KeyValueRangeResponse, RpcError>;
    /// Returns the number of items in the store
    async fn size(&self, ctx: &context::Context<'_>) -> Result<u64, RpcError>;
}

/// RangeKeyValueReceiver receives messages defined in the RangeKeyValue service trait
#[async_trait]
pub trait RangeKeyValueReceiver: MessageDispatch + RangeKeyValue {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: &Message<'_>,
    ) -> Result<Message<'static>, RpcError> {
        match message.method {
            "Get" => {
                let value: Key = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::get(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Get",
                    arg: buf,
                })
            }
            "Put" => {
                let value: KeyValue = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::put(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Put",
                    arg: buf,
                })
            }
            "Delete" => {
                let value: Key = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::delete(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Delete",
                    arg: buf,
                })
            }
            "Clear" => {
                let resp = RangeKeyValue::clear(self, ctx).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Clear",
                    arg: buf,
                })
            }
            "Contains" => {
                let value: Key = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::contains(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Contains",
                    arg: buf,
                })
            }
            "Keys" => {
                let value: RangeRequest = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::keys(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Keys",
                    arg: buf,
                })
            }
            "Values" => {
                let value: RangeRequest = deserialize(message.arg.as_ref())?;
                let resp = RangeKeyValue::values(self, ctx, &value).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Values",
                    arg: buf,
                })
            }
            "Size" => {
                let resp = RangeKeyValue::size(self, ctx).await?;
                let buf = Cow::Owned(serialize(&resp)?);
                Ok(Message {
                    method: "RangeKeyValue.Size",
                    arg: buf,
                })
            }
            _ => Err(RpcError::MethodNotHandled(format!(
                "RangeKeyValue::{}",
                message.method
            ))),
        }
    }
}

/// RangeKeyValueSender sends messages to a RangeKeyValue service
#[derive(Debug)]
pub struct RangeKeyValueSender<T> {
    transport: T,
    config: client::SendConfig,
}

impl<T: Transport> RangeKeyValueSender<T> {
    pub fn new(config: client::SendConfig, transport: T) -> Self {
        RangeKeyValueSender { transport, config }
    }
}

#[async_trait]
impl<T: Transport + std::marker::Sync + std::marker::Send> RangeKeyValue
    for RangeKeyValueSender<T>
{
    #[allow(unused)]
    /// Gets a value for a specified key.
    async fn get(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<MaybeValue, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Get",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
    #[allow(unused)]
    /// Stores a value. Replaces an existing value of the same key.
    async fn put(&self, ctx: &context::Context<'_>, arg: &KeyValue) -> Result<(), RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Put",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        Ok(())
    }
    #[allow(unused)]
    /// Deletes a value if it exists.
    async fn delete(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<(), RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Delete",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        Ok(())
    }
    #[allow(unused)]
    /// Clears all keys
    async fn clear(&self, ctx: &context::Context<'_>) -> Result<(), RpcError> {
        let arg = *b"";
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Clear",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        Ok(())
    }
    #[allow(unused)]
    /// Returns true if the value is contained in the store
    async fn contains(&self, ctx: &context::Context<'_>, arg: &Key) -> Result<bool, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Contains",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
    #[allow(unused)]
    /// Returns a range of keys
    async fn keys(
        &self,
        ctx: &context::Context<'_>,
        arg: &RangeRequest,
    ) -> Result<KeyRangeResponse, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Keys",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
    #[allow(unused)]
    /// Returns a range of key-value pairs
    async fn values(
        &self,
        ctx: &context::Context<'_>,
        arg: &RangeRequest,
    ) -> Result<KeyValueRangeResponse, RpcError> {
        let arg = serialize(arg)?;
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Values",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
    #[allow(unused)]
    /// Returns the number of items in the store
    async fn size(&self, ctx: &context::Context<'_>) -> Result<u64, RpcError> {
        let arg = *b"";
        let resp = self
            .transport
            .send(
                ctx,
                &self.config,
                Message {
                    method: "Size",
                    arg: Cow::Borrowed(&arg),
                },
            )
            .await?;
        let value = deserialize(resp.arg.as_ref())?;
        Ok(value)
    }
}
