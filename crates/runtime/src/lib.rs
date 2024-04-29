//! wasmCloud runtime library

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// Actor module parsing, loading and execution
pub mod actor;

/// Capability provider implementations and adaptors
pub mod capability;

/// Shared wasmCloud runtime engine
pub mod runtime;

/// wasmCloud I/O functionality
pub mod io;

#[deprecated(
    since = "0.3.1",
    note = "ActorConfig has been renamed to ComponentConfig"
)]
pub use actor::Config as ActorConfig;
pub use actor::{Component, ComponentInstance, Config as ComponentConfig};
pub use runtime::*;

pub use async_trait::async_trait;
pub use tokio;
