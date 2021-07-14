// This file is generated automatically using wasmcloud-weld and smithy model definitions
//

#![allow(dead_code)]
use serde::{Deserialize, Serialize};

pub const SMITHY_VERSION: &str = "1.0";

/// Capability contract id, e.g. 'wasmcloud:httpserver'
pub type CapabilityContractId = String;

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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodegenRust {
    /// Instructs rust codegen to add `#[derive(Default)]` (default false)
    #[serde(rename = "deriveDefault")]
    #[serde(default)]
    pub derive_default: bool,
}

/// A non-empty string (minimum length 1)
pub type NonEmptyString = String;

/// Overrides for serializer & deserializer
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Synonym {}

/// The unsignedInt trait indicates that one of the number types is unsigned
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnsignedInt {}

/// a protocol defines the semantics
/// of how a client and server communicate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Wasmbus {
    /// indicates this service's operations are handled by an actor (default false)
    #[serde(rename = "actorReceive")]
    #[serde(default)]
    pub actor_receive: bool,
    /// capability id such as "wasmcloud:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    #[serde(rename = "contractId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<CapabilityContractId>,
    /// indicates this service's operations are handled by an provider (default false)
    #[serde(rename = "providerReceive")]
    #[serde(default)]
    pub provider_receive: bool,
}

/// data sent via wasmbus
/// This trait is required for all messages sent via wasmbus
#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WasmbusData {}
