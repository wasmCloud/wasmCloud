//! wasmCloud component adapters

/// WASI preview1 -> preview2 component adapter for components of "command" type
pub const WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER: &[u8] =
    include_bytes!(env!("WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER"));

/// WASI preview1 -> preview2 component adapter for components of "reactor" type
pub const WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER: &[u8] =
    include_bytes!(env!("WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER"));
