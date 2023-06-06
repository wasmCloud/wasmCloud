pub mod logging;
pub mod random;

#[cfg(all(not(feature = "module"), feature = "component"))]
pub use crate::bindings::wasi::io;
