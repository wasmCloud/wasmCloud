//! wasmCloud runtime library

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::type_complexity)]
#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// Component module parsing, loading and execution
#[allow(clippy::module_inception)]
pub mod component;

/// Capability bindings
pub mod capability;

/// Feature flags to enable experimental functionality in the runtime
pub mod experimental;

/// Shared wasmCloud runtime engine
pub mod runtime;

/// wasmCloud I/O functionality
pub mod io;

pub use component::{Component, ComponentConfig};
pub use runtime::*;

pub use async_trait::async_trait;
pub use tokio;
