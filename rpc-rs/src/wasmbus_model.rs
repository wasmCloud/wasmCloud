// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
#[allow(unused_imports)]
use crate::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};
#[allow(unused_imports)]
use async_trait::async_trait;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::borrow::Cow;

pub const SMITHY_VERSION: &str = "1.0";

/// signed 16-bit int
pub type I16 = i16;

/// signed 32-bit int
pub type I32 = i32;

/// signed 64-bit int
pub type I64 = i64;

/// signed byte
pub type I8 = i8;

/// unsigned 16-bit int
pub type U16 = i16;

/// unsigned 32-bit int
pub type U32 = i32;

/// unsigned 64-bit int
pub type U64 = i64;

/// unsigned byte
pub type U8 = i8;

/// Rust codegen traits
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodegenRust {
    /// Instructs rust codegen to add `#[derive(Default)]` (default false)
    #[serde(rename = "deriveDefault")]
    #[serde(default)]
    pub derive_default: bool,
}

/// A non-empty string (minimum length 1)
pub type NonEmptyString = String;

/// Overrides for serializer & deserializer
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Serialization {
    /// (optional setting) Override field name when serializing and deserializing
    /// By default, (when `name` not specified) is the exact declared name without
    /// casing transformations. This setting does not affect the field name
    /// produced in code generation, which is always lanaguage-idiomatic
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// This trait doesn't have any functional impact on codegen. It is simply
/// to document that the defined type is a synonym, and to silence
/// the default validator that prints a notice for synonyms with no traits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Synonym {}

/// definitions for api modeling
/// These are modifications to the basic data model
/// The unsignedInt trait indicates that one of the number types is unsigned
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnsignedInt {}
