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

#[cfg(feature = "component-model")]
pub use actor::{Component as ActorComponent, ComponentInstance as ActorComponentInstance};
pub use actor::{
    Module as ActorModule, ModuleInstance as ActorModuleInstance,
    ModuleResponse as ActorModuleResponse,
};
pub use capability::{
    Handler as CapabilityHandler, HostHandler as HostCapabilityHandler,
    HostHandlerBuilder as HostCapabilityHandlerBuilder, Logging as LoggingCapability,
    Numbergen as NumbergenCapability,
};
pub use runtime::*;
