//! Error types for interacting with a provider

pub type InvocationResult<T> = Result<T, InvocationError>;
pub type ProviderInitResult<T> = Result<T, ProviderInitError>;

/// Result form for [`ProviderHandler`] trait methods
pub type ProviderOperationResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// All errors that that can be returned by a provider when it is being initialized,
/// primarily from internal provider-sdk code
#[derive(Debug, thiserror::Error)]
pub enum ProviderInitError {
    /// Errors when connecting to the lattice NATS cluster
    #[error(transparent)]
    Connect(#[from] async_nats::ConnectError),
    /// An error that occurs when subscribing to or interacting with RPC topics
    #[error(transparent)]
    Subscription(#[from] async_nats::SubscribeError),
    /// Initialization error when setting up a provider (such as invalid information or configuration)
    #[error("Initialization error: {0}")]
    Initialization(String),
}

/// Errors that can occur when sending or receiving an invocation, including the `dispatch` method
/// of the provider.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InvocationError {
    /// Indicates that validation for the invocation failed
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The invocation or dispatch timed out
    #[error("Invocation timed out")]
    Timeout,
    /// The invocation or dispatch failed when serializing data from the wire
    #[error("Error when serializing invocation: {0:?}")]
    // NOTE(thomastaylor312): we might have to just make this and `Deser` a string with some
    // convenience `From` implementations if we do have to indicate other failures other than
    // serialization to our wasmbus RPC messages
    Ser(#[from] rmp_serde::encode::Error),
    #[error("Error while serializing/deserializing JSON: {0:?}")]
    /// Serialization/Deserializing errors that occur for JSON
    SerdeJson(#[from] serde_json::Error),
    /// The invocation or dispatch failed when deserializing data from the wire
    #[error("Error when deserializing invocation: {0:?}")]
    Deser(#[from] rmp_serde::decode::Error),
    /// An error that occurred when trying to publish or request over NATS
    #[error("Networking error during invocation: {0:?}")]
    Network(#[from] NetworkError),
    /// Errors that occur when chunking data
    #[error("Error when chunking data: {0}")]
    Chunking(String),
    /// Returned when an invocation is malformed (e.g. has a method type that isn't supported)
    #[error("Malformed invocation: {0}")]
    Malformed(String),
    /// Returned when an invocation returns an error
    #[error("Unexpected error: {0}")]
    Unexpected(String),
}

/// All errors that can occur when validating an invocation
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// Issuer of the invocation is not authorized for this cluster
    #[error("Issuer of this invocation is not in list of cluster issuers")]
    InvalidIssuer,
    /// The target of the invocation is not the same as the provider
    #[error("Target of the invocation was {0}, which does not match the provider {1}")]
    InvalidTarget(String, String),
    /// The actor that sent the request is not linked to the provider
    #[error("Actor {0} is not linked to this provider")]
    InvalidActor(String),
    // Claims have expired
    #[error("Invocation claims token expired")]
    Expired,
    /// The signature on the claims is invalid
    #[error("Invocation claims signature invalid")]
    InvalidSignature,
    /// Claims are not valid yet. This occurs when the `nbf` field is in the future
    #[error("Invocation claims not valid yet")]
    NotValidYet,
    /// Wascap metadata is not present
    #[error("Invocation claims missing wascap metadata")]
    MissingWascapClaims,
    /// The hash on the invocation doesn't match the hash on the claims
    #[error("Invocation hash does not match claims hash")]
    HashMismatch,
    /// The claims are not valid JSON
    #[error("Invocation claims are not valid JSON")]
    InvalidJson(String),
    /// Host ID is not a valid nkey identity
    #[error("Invalid host ID: {0}")]
    InvalidHostId(String),
    /// The target of the invocation is not valid
    #[error("Invocation claims and invocation target URL do not match: {0} != {1}")]
    InvalidTargetUrl(String, String),
    /// The origin of the invocation is not valid
    #[error("Invocation claims and invocation origin URL do not match: {0} != {1}")]
    InvalidOriginUrl(String, String),
}

/// This is a wrapper around two different NATS errors that we use (publish and request). It
/// delegates to the underlying error types from NATS
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error(transparent)]
    Publish(#[from] async_nats::PublishError),
    #[error(transparent)]
    Request(#[from] async_nats::RequestError),
}
