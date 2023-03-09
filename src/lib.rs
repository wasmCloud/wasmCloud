//! wasmCloud host runtime library

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

pub use actor::{
    Instance as ActorInstance, InstanceConfig as ActorInstanceConfig, Module as ActorModule,
    Response as ActorResponse,
};
pub use capability::{
    Logging as LoggingCapability, Numbergen as NumbergenCapability, Provider as CapabilityProvider,
};
pub use runtime::*;
