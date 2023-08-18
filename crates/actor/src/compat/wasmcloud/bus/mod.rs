#[cfg(feature = "module")]
pub mod lattice;

#[cfg(feature = "module")]
pub mod host;

#[cfg(all(not(feature = "module"), feature = "component"))]
pub use crate::bindings::wasmcloud::bus::{host, lattice};
