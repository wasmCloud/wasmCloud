use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A wasmcloud message
#[derive(Debug)]
pub struct Message<'m> {
    /// Message name, usually in the form 'Trait.method'
    pub method: &'m str,
    /// parameter serialized as a byte array. If the method takes no args, the arraya will be
    /// zero length.
    pub arg: Cow<'m, [u8]>,
}

/// context data
pub mod context {

    /// Context - message passing metadata used by wasmhost Actors and Capability Providers
    #[derive(Default, Debug, Clone)]
    pub struct Context {
        /// Messages received by Context Provider will have actor set to the actor's public key
        pub actor: Option<String>,

        /// Span name/context for tracing. This is a placeholder for now
        pub span: Option<String>,
    }
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
        ctx: &context::Context,
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

/// An error that can occur in the processing of an RPC. This is not request-specific errors but
/// rather cross-cutting errors that can always occur.
#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RpcError {
    /// The request exceeded its deadline.
    #[error("the request exceeded its deadline: {0}")]
    DeadlineExceeded(String),

    /// A capability provider was called before its configure_dispatch was called.
    #[error("the capability provider has not been initialized: {0}")]
    NotInitialized(String),

    #[error("method not handled {0}")]
    MethodNotHandled(String),

    /// Error that can be returned if server has not implemented
    /// an optional interface method
    #[error("method not implemented")]
    NotImplemented,

    #[error("Host send error {0}")]
    HostError(String),

    #[error("deserialization: {0}")]
    Deser(String),

    #[error("serialization: {0}")]
    Ser(String),

    #[error("rpc: {0}")]
    Rpc(String),

    #[error("nats: {0}")]
    Nats(String),

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// Error occurred in actor's rpc handler
    #[error("actor: {0}")]
    ActorHandler(String),

    /// Error occurred during provider initialization or put-link
    #[error("provider initialization or put-link: {0}")]
    ProviderInit(String),

    /// Timeout occurred
    #[error("timeout: {0}")]
    Timeout(String),

    //#[error("IO error")]
    //IO([from] std::io::Error)
    /// Anything else
    #[error("{0}")]
    Other(String),
}

impl From<String> for RpcError {
    fn from(s: String) -> RpcError {
        RpcError::Other(s)
    }
}

impl From<&str> for RpcError {
    fn from(s: &str) -> RpcError {
        RpcError::Other(s.to_string())
    }
}

impl From<std::io::Error> for RpcError {
    fn from(e: std::io::Error) -> RpcError {
        RpcError::Other(format!("io: {}", e))
    }
}

impl From<minicbor::encode::Error<std::io::Error>> for RpcError {
    fn from(e: minicbor::encode::Error<std::io::Error>) -> RpcError {
        RpcError::Ser(format!("cbor-encode: {}", e))
    }
}

impl From<minicbor::decode::Error> for RpcError {
    fn from(e: minicbor::decode::Error) -> RpcError {
        RpcError::Deser(format!("cbor-decode: {}", e))
    }
}

#[async_trait]
pub trait MessageDispatch {
    async fn dispatch(
        &self,
        ctx: &context::Context,
        message: Message<'_>,
    ) -> Result<Message<'_>, RpcError>;
}
