use crate::error::{RpcError, RpcResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, fmt};

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
pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
    rmp_serde::from_read_ref(buf).map_err(|e| RpcError::Deser(e.to_string()))
}

pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
    rmp_serde::to_vec_named(data).map_err(|e| RpcError::Ser(e.to_string()))
    // for benchmarking: the following line uses msgpack without field names
    //rmp_serde::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
}

#[async_trait]
pub trait MessageDispatch {
    async fn dispatch<'disp, 'ctx, 'msg>(
        &'disp self,
        ctx: &'ctx Context,
        message: Message<'msg>,
    ) -> Result<Message<'msg>, RpcError>;
}

/// Message encoding format
#[derive(Clone, PartialEq)]
pub enum MessageFormat {
    Msgpack,
    Cbor,
    Empty,
    Unknown,
}

impl fmt::Display for MessageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MessageFormat::Msgpack => "msgpack",
            MessageFormat::Cbor => "cbor",
            MessageFormat::Empty => "empty",
            MessageFormat::Unknown => "unknown",
        })
    }
}

impl fmt::Debug for MessageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string())
    }
}

impl MessageFormat {
    pub fn write_header<W: std::io::Write>(&self, mut buf: W) -> std::io::Result<usize> {
        match self {
            MessageFormat::Cbor => buf.write(&[127u8]),    // 0x7f
            MessageFormat::Msgpack => buf.write(&[193u8]), // 0xc1
            MessageFormat::Empty => Ok(0),
            MessageFormat::Unknown => Ok(0),
        }
    }
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
                0x7f => (MessageFormat::Cbor, 1),           // prefix + cbor
                0xc1 => (MessageFormat::Msgpack, 1),        // prefix + msgpack
                0x00..=0x7e => (MessageFormat::Unknown, 0), // RESERVED
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

pub trait DecodeOwned: for<'de> crate::minicbor::Decode<'de> {}
impl<T> DecodeOwned for T where T: for<'de> crate::minicbor::Decode<'de> {}

/// Wasmbus rpc sender that can send any message and cbor-serializable payload
/// requires Protocol="2"
pub struct AnySender<T: Transport> {
    transport: T,
}

impl<T: Transport> AnySender<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport + Sync + Send> AnySender<T> {
    /// Send enoded payload
    #[inline]
    async fn send_raw<'s, 'ctx, 'msg>(
        &'s self,
        ctx: &'ctx Context,
        msg: Message<'msg>,
    ) -> RpcResult<Vec<u8>> {
        self.transport.send(ctx, msg, None).await
    }

    /// Send rpc with serializable payload
    pub async fn send<In: Serialize, Out: serde::de::DeserializeOwned>(
        &self,
        ctx: &Context,
        method: &str,
        arg: &In,
    ) -> RpcResult<Out> {
        let mut buf = Vec::new();
        MessageFormat::Cbor.write_header(&mut buf).unwrap();
        minicbor_ser::to_writer(arg, &mut buf).map_err(|e| RpcError::Ser(e.to_string()))?;
        let resp = self
            .send_raw(
                ctx,
                Message {
                    method,
                    arg: Cow::Borrowed(&buf),
                },
            )
            .await?;
        let result: Out =
            minicbor_ser::from_slice(&resp).map_err(|e| RpcError::Deser(e.to_string()))?;
        Ok(result)
    }

    /// Send rpc with serializable payload using cbor encode/decode
    pub async fn send_cbor<'de, In: crate::minicbor::Encode, Out: DecodeOwned>(
        &self,
        ctx: &Context,
        method: &str,
        arg: &In,
    ) -> RpcResult<Out> {
        let mut buf = Vec::new();
        MessageFormat::Cbor.write_header(&mut buf).unwrap();
        crate::minicbor::encode(arg, &mut buf).map_err(|e| RpcError::Ser(e.to_string()))?;
        let resp = self
            .send_raw(
                ctx,
                Message {
                    method,
                    arg: Cow::Borrowed(&buf),
                },
            )
            .await?;
        let result: Out =
            crate::minicbor::decode(&resp).map_err(|e| RpcError::Deser(e.to_string()))?;
        Ok(result)
    }
}
