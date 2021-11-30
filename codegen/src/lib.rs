#![forbid(unsafe_code)]

mod error;
pub use error::{Error, Result};
#[cfg(feature = "cbor")]
pub(crate) mod codegen_py;
pub(crate) mod codegen_rust;
pub mod config;
#[cfg(feature = "cbor")]
pub(crate) mod decode_py;
pub(crate) mod decode_rust;
pub mod docgen;
#[cfg(feature = "cbor")]
pub(crate) mod encode_py;
pub(crate) mod encode_rust;
pub(crate) mod gen;
mod loader;
pub(crate) mod model;
pub mod render;
pub mod writer;
pub use gen::{templates_from_dir, Generator};

pub(crate) use loader::sources_to_paths;
pub use loader::{sources_to_model, weld_cache_dir};
pub use rust_build::rust_build;

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
