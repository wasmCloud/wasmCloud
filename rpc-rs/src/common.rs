use crate::{RpcError, RpcResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A wasmcloud message
#[derive(Debug)]
pub struct Message<'m> {
    /// Message name, usually in the form 'Trait.method'
    pub method: &'m str,
    /// parameter serialized as a byte array. If the method takes no args, the array will be
    /// zero length.
    pub arg: Cow<'m, [u8]>,
}

/// Context - message passing metadata used by wasmhost Actors and Capability Providers
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Messages received by Context Provider will have actor set to the actor's public key
    pub actor: Option<String>,

    /// Span name/context for tracing. This is a placeholder for now
    pub span: Option<String>,
}

/// Client config defines the intended recipient of a message and parameters that transport may use to adapt sending it
#[derive(Default, Debug)]
pub struct SendOpts {
    /// Optional flag for idempotent messages - transport may perform retries within configured timeouts
    pub idempotent: bool,

    /// Optional flag for read-only messages - those that do not change the responder's state. read-only messages may be retried within configured timeouts.
    pub read_only: bool,
}

impl SendOpts {
    #[must_use]
    pub fn idempotent(mut self, val: bool) -> SendOpts {
        self.idempotent = val;
        self
    }

    #[must_use]
    pub fn read_only(mut self, val: bool) -> SendOpts {
        self.read_only = val;
        self
    }
}

/// Transport determines how messages are sent
/// Alternate implementations could be mock-server, or test-fuzz-server / test-fuzz-client
#[async_trait]
pub trait Transport: Send {
    async fn send(
        &self,
        ctx: &Context,
        req: Message<'_>,
        opts: Option<SendOpts>,
    ) -> std::result::Result<Vec<u8>, RpcError>;

    /// Sets rpc timeout
    fn set_timeout(&self, interval: std::time::Duration);
}

// select serialization/deserialization mode
cfg_if::cfg_if! {
    if #[cfg(feature = "ser_msgpack")] {
        pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
            rmp_serde::from_read_ref(buf).map_err(|e| RpcError::Deser(e.to_string()))
        }

        pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
            rmp_serde::to_vec_named(data).map_err(|e| RpcError::Ser(e.to_string()))
            // for benchmarking: the following line uses msgpack without field names
            //rmp_serde::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
        }
    } else if #[cfg(feature = "ser_json")] {
        pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
            serde_json::from_slice(buf).map_err(|e| RpcError::Deser(e.to_string()))
        }

        pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
            serde_json::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
        }
    }
}

#[async_trait]
pub trait MessageDispatch {
    async fn dispatch(&self, ctx: &Context, message: Message<'_>) -> Result<Message<'_>, RpcError>;
}

/// Message encodingn format
pub enum MessageFormat {
    Msgpack,
    Cbor,
    Empty,
    Unknown,
}

/// returns serialization format,
/// and offset for beginning of payload
pub fn message_format(data: &[u8]) -> (MessageFormat, usize) {
    // The initial byte of the message is used to distinguish between
    // a legacy msgpack payload, and a prefix plus payload.
    // If the payload has only a single byte, it must be msgpack.
    // A wasmbus msgpack payload containing 2 or more bytes will never begin
    // with any of the following:
    //   0x00-0x7f - used by msgpack to encode small ints 0-127
    //   0xc0      - msgpack Nil
    //   0xc1      - unused by msgpack
    // These values can all be used as the initial byte of a prefix.
    //
    // (The reason the first byte cannot be a msgpack small int is that
    //  wasmbus payloads only contain a single value: a primitive type,
    //  or a struct or a map. It cannot be a Nil because the single value
    //  of a wasmbus message is never an Option<> and there is no other
    //  way that a null would be generated.)
    match data.len() {
        0 => (MessageFormat::Empty, 0),
        1 => (MessageFormat::Msgpack, 0), // 1-byte msgpack legacy
        _ => {
            match data[0] {
                0x00 => (MessageFormat::Cbor, 1),           // prefix + cbor
                0xc1 => (MessageFormat::Msgpack, 1),        // prefix + msgpack
                0x01..=0x7f => (MessageFormat::Unknown, 0), // RESERVED
                0xc0 => (MessageFormat::Unknown, 0),        // RESERVED
                _ => (MessageFormat::Msgpack, 0),           // legacy
            }
        }
    }
}

pub type CborDecodeFn<T> = dyn Fn(&mut crate::cbor::Decoder<'_>) -> RpcResult<T>;

pub fn decode<T: serde::de::DeserializeOwned>(
    buf: &[u8],
    cbor_dec: &CborDecodeFn<T>,
) -> RpcResult<T> {
    let value = match message_format(buf) {
        (MessageFormat::Cbor, offset) => {
            let d = &mut crate::cbor::Decoder::new(&buf[offset..]);
            cbor_dec(d)?
        }
        (MessageFormat::Msgpack, offset) => deserialize(&buf[offset..])
            .map_err(|e| RpcError::Deser(format!("decoding '{}': {{}}", e)))?,
        _ => return Err(RpcError::Deser("invalid encoding for '{}'".to_string())),
    };
    Ok(value)
}
