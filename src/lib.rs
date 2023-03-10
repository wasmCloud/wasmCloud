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
    ModuleInstance as ActorModuleInstance, InstanceConfig as ActorInstanceConfig, Module as ActorModule,
    Response as ActorResponse,
};
pub use capability::{
    BuiltinHandler as BuiltinCapabilityHandler, Handler as CapabilityHandler,
    Logging as LoggingCapability, Numbergen as NumbergenCapability,
};
pub use runtime::*;
