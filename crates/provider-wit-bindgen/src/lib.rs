pub mod deps {
    pub use anyhow;
    pub use async_trait;
    pub use bytes;
    pub use futures;
    pub use serde;
    pub use serde_bytes;
    pub use serde_json;
    pub use tracing;
    pub use wasmcloud_provider_sdk;
    pub use wrpc_transport;
    pub use wrpc_transport_derive;
    pub use wrpc_types;
}

// Backwards compatibility
pub use wasmcloud_provider_wit_bindgen_macro::generate;
