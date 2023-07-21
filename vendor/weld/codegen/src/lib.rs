#![forbid(unsafe_code)]

pub mod error;
use error::Error;
pub mod config;
pub mod docgen;

pub(crate) mod codegen_go;
pub(crate) mod codegen_py;
pub(crate) mod codegen_rust;

pub(crate) mod decode_py;
pub(crate) mod decode_rust;

pub(crate) mod encode_py;
pub(crate) mod encode_rust;

pub(crate) mod gen;
mod loader;
pub(crate) mod model;
pub mod render;
pub(crate) mod validate;
pub mod writer;
pub use gen::{templates_from_dir, Generator};
pub(crate) use loader::sources_to_paths;
pub use loader::{sources_to_model, weld_cache_dir};
pub use rust_build::{rust_build, rust_build_into};

pub mod format;
mod rust_build;

#[allow(dead_code)]
pub(crate) mod wasmbus_model {
    include!("./wasmbus_model.rs");
}

// enable other tools to invoke codegen directly. Add other languages as needed
pub mod generators {
    pub use crate::codegen_go::GoCodeGen;
    pub use crate::codegen_rust::RustCodeGen;
    pub use crate::gen::CodeGen;
}

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
