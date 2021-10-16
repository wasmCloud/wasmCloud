#![forbid(unsafe_code)]

mod error;
pub use error::{Error, Result};
pub(crate) mod codegen_go;
pub(crate) mod codegen_rust;
pub mod config;
pub mod docgen;
pub(crate) mod gen;
mod loader;
pub(crate) mod model;
pub mod render;
pub mod writer;
#[cfg(not(target_arch = "wasm32"))]
pub use gen::templates_from_dir;
pub use gen::Generator;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use loader::sources_to_paths;
#[cfg(not(target_arch = "wasm32"))]
pub use loader::{sources_to_model, weld_cache_dir};
pub use rust_build::rust_build;

mod decode_rust;
mod encode_rust;
pub mod format;
mod rust_build;

pub(crate) mod wasmbus_model;

// re-export
pub use bytes::Bytes;
pub(crate) use bytes::BytesMut;

// common types used in this crate
pub(crate) type TomlValue = toml::Value;
pub(crate) type JsonValue = serde_json::Value;
pub(crate) type JsonMap = serde_json::Map<String, JsonValue>;
pub(crate) type ParamMap = std::collections::BTreeMap<String, serde_json::Value>;

pub(crate) mod strings {
    /// re-export inflector string conversions
    pub use inflector::cases::{
        camelcase::to_camel_case, pascalcase::to_pascal_case, snakecase::to_snake_case,
    };
}
