mod error;

pub mod provider;
pub use anyhow;
pub use async_nats;
pub use provider::{
    get_connection, load_ext_data, run_provider, serve_provider_exports, serve_provider_extension,
    Context, ProviderConnection, WrpcClient,
};
pub use tracing_subscriber;
pub use wasmcloud_core as core;
/// Re-export of types from [`wasmcloud_core`]
pub use wasmcloud_core::{
    bindings::wrpc::extension::types, ExtensionData, InterfaceLinkDefinition, WitFunction,
    WitInterface, WitNamespace, WitPackage,
};
pub use wasmcloud_tracing;

#[cfg(feature = "otel")]
mod otel;
