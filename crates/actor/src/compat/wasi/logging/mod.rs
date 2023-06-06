#[cfg(feature = "module")]
pub mod logging;

#[cfg(all(not(feature = "module"), feature = "component"))]
pub use crate::bindings::wasi::logging::logging;
