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
    pub struct Context<'msg> {
        /// Messages received by Context Provider will have actor set to the actor's public key
        pub actor: Option<&'msg str>,

        /// Span name/context for tracing. This is a placeholder for now
        pub span: Option<String>,
    }
}

/// client is the caller side of any interface
pub mod client {

    /// Client config defines the intended recipient of a message and parameters that transport may use to adapt sending it
    #[derive(Debug)]
    pub struct SendConfig {
        /// Host/link name, usually "default" for the current host
        pub host: String,

        /// Recipient of message, such as actor's public key or provider id
        pub target: String,

        /// Optional flag for idempotent messages - transport may perform retries within configured timeouts
        pub idempotent: bool,

        /// Optional flag for read-only messages - those that do not change the responder's state. read-only messages may be retried within configured timeouts.
        pub read_only: bool,
    }

    impl Default for SendConfig {
        fn default() -> SendConfig {
            SendConfig {
                host: "default".to_string(),
                target: String::default(),
                idempotent: false,
                read_only: false,
            }
        }
    }

    impl SendConfig {
        /// Constructs a new client with host (link binding) and target.
        /// When sending to a capability provider, the host parameter
        /// is the link name (usually "default" for the default host),
        /// and target is the capability contract id, e.g., "wasmcloud:keyvalue"
        pub fn new<H: Into<String>, T: Into<String>>(host: H, target: T) -> SendConfig {
            SendConfig {
                host: host.into(),
                target: target.into(),
                ..Default::default()
            }
        }

        /// Create a SendConfig for sending to an actor
        pub fn actor<T: Into<String>>(target: T) -> SendConfig {
            SendConfig {
                target: target.into(),
                ..Default::default()
            }
        }

        /// Create a SendConfig using the capability contract id
        /// (e.g., "wasmcloud:keyvalue"). Uses the default link binding.
        pub fn contract<T: Into<String>>(contract: T) -> SendConfig {
            SendConfig {
                target: contract.into(),
                ..Default::default()
            }
        }

        /// Create a SendConfig using the default host and specified target
        pub fn target<T: Into<String>>(target: T) -> SendConfig {
            SendConfig {
                target: target.into(),
                ..Default::default()
            }
        }

        pub fn idempotent(mut self, val: bool) -> SendConfig {
            self.idempotent = val;
            self
        }

        pub fn read_only(mut self, val: bool) -> SendConfig {
            self.read_only = val;
            self
        }
    }
}

/// Transport determines how messages are sent
/// Alternate implementations could be mock-server, or test-fuzz-server / test-fuzz-client
#[async_trait]
pub trait Transport: Send {
    async fn send(
        &self,
        ctx: &context::Context<'_>,
        config: &client::SendConfig,
        req: Message<'_>,
    ) -> std::result::Result<Message<'_>, RpcError>;
}

// select serialization/deserialization mode
cfg_if::cfg_if! {
    if #[cfg(feature = "ser_msgpack")] {
        pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
            rmp_serde::from_read_ref(buf).map_err(|e| RpcError::Deser(e.to_string()))
        }

        pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
            rmp_serde::to_vec_named(data).map_err(|e| RpcError::Ser(e.to_string()))
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
pub enum RpcError {
    /// The request exceeded its deadline.
    #[error("the request exceeded its deadline: {0}")]
    DeadlineExceeded(String),

    /// A capability provider was called before its configure_dispatch was called.
    #[error("the capability provider has not been initialized: {0}")]
    NotInitialized(String),

    /// The message was invalid
    #[error("the message was invalid")]
    Invalid(String),

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

    /// Anything else
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait MessageDispatch {
    async fn dispatch(
        &self,
        ctx: &context::Context<'_>,
        message: Message<'_>,
    ) -> Result<Message<'_>, RpcError>;
}

//macro_rules! implement_service {
//    ( ( $trait:ident, $impl:ident ),*) => {
// need to do a few things
//  1. build list of interfaces tha I respond to, so I can return my own api
//  2. a: return list of serve() functions, so dispatcher can call them
//  2. b: or, build my own dispatcher function that tries each one
//
//    };
//}

#[cfg(test)]
mod test {

    use super::*;
    use client::SendConfig;

    #[test]
    fn send_config_constructor() {
        let c = SendConfig::default();
        assert_eq!(&c.target, "");
        assert_eq!(&c.host, "default");
        assert_eq!(c.idempotent, false);
        assert_eq!(c.read_only, false);

        let c = SendConfig::actor("a");
        assert_eq!(&c.target, "a");
        assert_eq!(&c.host, "default");
        assert_eq!(c.idempotent, false);
        assert_eq!(c.read_only, false);

        let c = SendConfig::target("t");
        assert_eq!(&c.target, "t");
        assert_eq!(&c.host, "default");
        assert_eq!(c.idempotent, false);
        assert_eq!(c.read_only, false);
    }

    #[test]
    fn send_config_builder() {
        let c = SendConfig::actor("x").idempotent(true).read_only(true);
        assert_eq!(&c.target, "x");
        assert_eq!(&c.host, "default");
        assert_eq!(c.idempotent, true);
        assert_eq!(c.read_only, true);

        let c = SendConfig::actor("x").idempotent(false).read_only(false);
        assert_eq!(c.idempotent, false);
        assert_eq!(c.read_only, false);
    }
}
