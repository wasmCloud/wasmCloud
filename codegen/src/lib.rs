#![forbid(unsafe_code)]

pub use error::{Error, Result};
mod loader;

//pub mod codegen_as;
//pub mod codegen_go;
pub(crate) mod codegen_rust;
pub mod config;
//mod docgen;
pub mod docgen;
mod error;
pub(crate) mod gen;
pub(crate) mod model;
pub(crate) mod render;
/// utility for running 'rustfmt'
#[cfg(not(target_arch = "wasm32"))]
pub mod rustfmt;
pub mod writer;
pub use codegen_rust::rust_build;
pub use config::ModelSource;
pub use gen::{templates_from_dir, Generator};
pub(crate) use loader::sources_to_paths;
pub use loader::{sources_to_model, weld_cache_dir};

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
