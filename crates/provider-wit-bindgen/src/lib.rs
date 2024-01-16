pub mod deps {
    pub use async_trait;
    pub use serde;
    pub use serde_bytes;
    pub use serde_json;
    pub use wasmcloud_provider_sdk;
}

// Backwards compatibility
pub use wasmcloud_provider_wit_bindgen_macro::generate;
