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

pub use actor::{Component, ComponentInstance, Config as ActorConfig};
pub use runtime::*;

pub use async_trait::async_trait;
pub use tokio;
