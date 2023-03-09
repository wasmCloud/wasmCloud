/// [rand](::rand) crate adaptors for random number generation capability
#[cfg(feature = "rand")]
pub mod rand;

#[cfg(feature = "rand")]
pub use self::rand::Numbergen as RandNumbergen;

pub use uuid;
pub use uuid::Uuid;

use core::fmt::Debug;

/// Builtin random number generation capability available within `wasmcloud:builtin:numbergen` namespace
pub trait Numbergen {
    /// Error returned by random number generation operations
    type Error: ToString + Debug;

    /// Generates a v4 [Uuid]
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn generate_guid(&self) -> Result<Uuid, Self::Error>;

    /// Returns a random [u32] within inclusive range from `min` to `max`
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn random_in_range(&self, min: u32, max: u32) -> Result<u32, Self::Error>;

    /// Returns a random [u32]
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn random_32(&self) -> Result<u32, Self::Error>;
}
