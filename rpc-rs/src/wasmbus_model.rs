// This file is generated automatically using wasmcloud/weld-codegen and smithy model definitions
//

use crate::RpcError;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

pub const SMITHY_VERSION: &str = "1.0";

/// Capability contract id, e.g. 'wasmcloud:httpserver'
pub type CapabilityContractId = String;

// Decode CapabilityContractId from cbor input stream
#[doc(hidden)]
pub fn decode_capability_contract_id(
    d: &mut crate::cbor::Decoder<'_>,
) -> Result<CapabilityContractId, RpcError> {
    let __result = { d.str()?.to_string() };
    Ok(__result)
}
/// signed 16-bit int
pub type I16 = i16;

// Decode I16 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i16(d: &mut crate::cbor::Decoder<'_>) -> Result<I16, RpcError> {
    let __result = { d.i16()? };
    Ok(__result)
}
/// signed 32-bit int
pub type I32 = i32;

// Decode I32 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i32(d: &mut crate::cbor::Decoder<'_>) -> Result<I32, RpcError> {
    let __result = { d.i32()? };
    Ok(__result)
}
/// signed 64-bit int
pub type I64 = i64;

// Decode I64 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i64(d: &mut crate::cbor::Decoder<'_>) -> Result<I64, RpcError> {
    let __result = { d.i64()? };
    Ok(__result)
}
/// signed byte
pub type I8 = i8;

// Decode I8 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i8(d: &mut crate::cbor::Decoder<'_>) -> Result<I8, RpcError> {
    let __result = { d.i8()? };
    Ok(__result)
}
/// list of identifiers
pub type IdentifierList = Vec<String>;

// Decode IdentifierList from cbor input stream
#[doc(hidden)]
pub fn decode_identifier_list(
    d: &mut crate::cbor::Decoder<'_>,
) -> Result<IdentifierList, RpcError> {
    let __result = {
        if let Some(n) = d.array()? {
            let mut arr: Vec<String> = Vec::with_capacity(n as usize);
            for _ in 0..(n as usize) {
                arr.push(d.str()?.to_string())
            }
            arr
        } else {
            // indefinite array
            let mut arr: Vec<String> = Vec::new();
            loop {
                match d.datatype() {
                    Err(_) => break,
                    Ok(crate::cbor::Type::Break) => break,
                    Ok(_) => arr.push(d.str()?.to_string()),
                }
            }
            arr
        }
    };
    Ok(__result)
}
/// unsigned 16-bit int
pub type U16 = i16;

// Decode U16 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u16(d: &mut crate::cbor::Decoder<'_>) -> Result<U16, RpcError> {
    let __result = { d.i16()? };
    Ok(__result)
}
/// unsigned 32-bit int
pub type U32 = i32;

// Decode U32 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u32(d: &mut crate::cbor::Decoder<'_>) -> Result<U32, RpcError> {
    let __result = { d.i32()? };
    Ok(__result)
}
/// unsigned 64-bit int
pub type U64 = i64;

// Decode U64 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u64(d: &mut crate::cbor::Decoder<'_>) -> Result<U64, RpcError> {
    let __result = { d.i64()? };
    Ok(__result)
}
/// unsigned byte
pub type U8 = i8;

// Decode U8 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u8(d: &mut crate::cbor::Decoder<'_>) -> Result<U8, RpcError> {
    let __result = { d.i8()? };
    Ok(__result)
}
/// Rust codegen traits
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodegenRust {
    /// if true, disables deriving 'Default' trait
    #[serde(rename = "noDeriveDefault")]
    #[serde(default)]
    pub no_derive_default: bool,
    /// if true, disables deriving 'Eq' trait
    #[serde(rename = "noDeriveEq")]
    #[serde(default)]
    pub no_derive_eq: bool,
}

/// indicates that a trait or class extends one or more bases
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Extends {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<IdentifierList>,
}

/// Field sequence number. A zero-based field number for each member of a structure,
/// to enable deterministic cbor serialization and improve forward and backward compatibility.
/// Although the values are not required to be sequential, gaps are filled with nulls
/// during encoding and so will slightly increase the encoding size.
pub type N = i16;

/// A non-empty string (minimum length 1)
pub type NonEmptyString = String;

/// Rename item(s) in target language.
/// Useful if the item name (operation, or field) conflicts with a keyword in the target language.
/// example: @rename({lang:"python",name:"delete"})
pub type Rename = Vec<RenameItem>;

/// list element of trait @rename. the item name in the target language
/// see '@rename'
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenameItem {
    /// language
    #[serde(default)]
    pub lang: String,
    /// the name of the structure/operation/field
    #[serde(default)]
    pub name: String,
}

/// Overrides for serializer & deserializer
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Synonym {}

/// The unsignedInt trait indicates that one of the number types is unsigned
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnsignedInt {}

/// a protocol defines the semantics
/// of how a client and server communicate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WasmbusData {}
