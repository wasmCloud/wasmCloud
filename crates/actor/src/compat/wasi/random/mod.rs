#[cfg(feature = "module")]
pub mod random;

#[cfg(all(not(feature = "module"), feature = "component"))]
pub use crate::bindings::wasi::random::random;
