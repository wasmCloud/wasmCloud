/// A logging capability discarding all logging statements
pub mod discard;
#[cfg(feature = "log")]
/// [log](::log) crate adaptors for logging capability
pub mod log;

pub use self::discard::Logging as DiscardLogging;
#[cfg(feature = "log")]
pub use self::log::Logging as LogLogging;

use core::fmt::Debug;

use async_trait::async_trait;
use wascap::jwt;

/// Builtin logging capability available within `wasmcloud:builtin:logging` namespace
#[async_trait]
pub trait Logging: Sync + Send {
    /// Error returned by logging operations
    type Error: ToString + Debug;

    /// Log at debug level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    async fn debug(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error>;

    /// Log at info level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    async fn info(&self, claims: &jwt::Claims<jwt::Actor>, text: String)
        -> Result<(), Self::Error>;

    /// Log at warn level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    async fn warn(&self, claims: &jwt::Claims<jwt::Actor>, text: String)
        -> Result<(), Self::Error>;

    /// Log at error level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    async fn error(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error>;
}
