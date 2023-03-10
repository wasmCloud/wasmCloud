/// A logging capability discarding all logging statements
pub mod discard;
#[cfg(feature = "log")]
/// [log](::log) crate adaptors for logging capability
pub mod log;

pub use self::discard::Logging as DiscardLogging;
#[cfg(feature = "log")]
pub use self::log::Logging as LogLogging;

use core::fmt::Debug;

/// Builtin logging capability available within `wasmcloud:builtin:logging` namespace
pub trait Logging {
    /// Error returned by logging operations
    type Error: ToString + Debug;

    /// Log at debug level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn debug(&self, text: String) -> Result<(), Self::Error>;

    /// Log at info level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn info(&self, text: String) -> Result<(), Self::Error>;

    /// Log at warn level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn warn(&self, text: String) -> Result<(), Self::Error>;

    /// Log at error level
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn error(&self, text: String) -> Result<(), Self::Error>;
}
