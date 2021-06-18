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
    #[derive(Default, Debug)]
    pub struct Context<'msg> {
        /// Messages received by Context Provider will have actor set to the actor's public key
        pub actor: Option<&'msg str>,

        /// Span name/context for tracing. This is a placeholder for now
        pub span: Option<String>,
    }
}

/// client is the caller side of any interface
pub mod client {
    /// Client config defines the intended recipient of a message
    #[derive(Debug)]
    pub struct ClientConfig {
        /// Host/link name, usually "default" for the current host
        pub host: String,
        /// Recipient of message, such as actor's public key or provider id
        pub target: String,
    }

    impl ClientConfig {
        /// Constructs a new client with host and target
        /// when sending to a capability provider,
        pub fn new<H: Into<String>, T: Into<String>>(host: H, target: T) -> ClientConfig {
            ClientConfig {
                host: host.into(),
                target: target.into(),
            }
        }

        /// Create a ClientConfig for sending to an actor
        pub fn actor<T: Into<String>>(target: T) -> ClientConfig {
            ClientConfig {
                host: "default".into(),
                target: target.into(),
            }
        }

        /// Create a ClientConfig using the default host and specified target
        pub fn target<T: Into<String>>(target: T) -> ClientConfig {
            ClientConfig {
                host: "default".into(),
                target: target.into(),
            }
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
        config: &client::ClientConfig,
        req: Message<'_>,
    ) -> std::result::Result<Message<'static>, RpcError>;
}

#[derive(Clone, Debug, Default)]
pub struct WasmHost {}

//#[cfg(target_arch = "wasm32")]
#[async_trait]
impl Transport for WasmHost {
    async fn send(
        &self,
        _ctx: &context::Context<'_>,
        config: &client::ClientConfig,
        req: Message<'_>,
    ) -> std::result::Result<Message<'static>, RpcError> {
        // TODO: currently makes no distinction between sending to actor and provider
        // this is an actor call
        let res = crate::host_call(
            &config.host,   // "default", or capability provider ID
            &config.target, // actor_ref, or capability name (e.g. wasmcloud::messaging)
            req.method,
            req.arg.as_ref(),
        )?;
        Ok(Message {
            method: "_reply",
            arg: Cow::Owned(res),
        })
    }
}

pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
    //serde_json::from_slice(buf).map_err(|e| RpcError::Deser(e.to_string()))
    rmp_serde::from_slice(buf).map_err(|e| RpcError::Deser(e.to_string()))
}

pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
    //serde_json::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
    rmp_serde::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
}

/// An error that can occur in the processing of an RPC. This is not request-specific errors but
/// rather cross-cutting errors that can always occur.
#[derive(thiserror::Error, Debug, Serialize, Deserialize)]
pub enum RpcError {
    /// The request exceeded its deadline.
    #[error("the request exceeded its deadline")]
    DeadlineExceeded,

    /// A capability provider was called before its configure_dispatch was called.
    #[error("the capability provider has not been initialized")]
    NotInitialized,

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

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// Error occurred in actor's rpc handler
    #[error("actor: {0}")]
    ActorHandler(String),

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
    ) -> Result<Message<'static>, RpcError>;
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
