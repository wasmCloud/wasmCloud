use serde::{Deserialize, Serialize};

/// An error that can occur in the processing of an RPC. This is not request-specific errors but
/// rather cross-cutting errors that can always occur.
#[derive(thiserror::Error, Debug, PartialEq, Serialize, Deserialize)]
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

pub type RpcResult<T> = std::result::Result<T, RpcError>;

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

impl<E> core::convert::From<minicbor::encode::Error<E>> for RpcError {
    fn from(e: minicbor::encode::Error<E>) -> Self {
        let msg = match e {
            minicbor::encode::Error::Write(_) => "writing to buffer",
            minicbor::encode::Error::Message(s) => s,
            _ => "unspecified encoding error",
        }
        .to_string();
        RpcError::Ser(format!("encode: {}", msg))
    }
}

impl core::convert::From<minicbor::decode::Error> for RpcError {
    fn from(e: minicbor::decode::Error) -> Self {
        RpcError::Ser(format!("decode: {}", e))
    }
}

/*
impl<W: minicbor::encode::Write, E: W::Error<W>>> From<minicbor::encode::Error<W::Error>> for RpcError {
    fn from(_ee: Error<<dyn minicbor::encode::Write>::Error>) -> RpcError {
        RpcError::Other("help".to_string())
    }
}
 */

/*
impl<W: std::io::Write> From<minicbor::encode::Error<W>> for RpcError {
    fn from(e: minicbor::encode::Error<W>) -> RpcError
    where
        W: minicbor::encode::Write,
    {
        RpcError::Ser(
            match e {
                minicbor::encode::Error::Write(_) => "writing to buffer",
                minicbor::encode::Error::Message(s) => s,
                _ => "unspecified encoding error",
            }
            .to_string(),
        )
    }
}
 */
