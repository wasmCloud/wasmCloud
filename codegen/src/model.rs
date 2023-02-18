//! Smithy model helpers
//! - some constants used for model validation
//! - ModelIndex is a "cache" of a smithy model grouped by shape kind and sorted by identifier name
//! - various macros used in the codegen crate
//!
use std::{fmt, ops::Deref, str::FromStr};

use atelier_core::{
    model::{
        shapes::{AppliedTraits, HasTraits, MemberShape, Operation, ShapeKind, StructureOrUnion},
        values::{Number, Value as NodeValue},
        HasIdentity, Identifier, Model, NamespaceID, ShapeID,
    },
    prelude::prelude_namespace_id,
};
use lazy_static::lazy_static;
use serde::{de::DeserializeOwned, Deserialize};

use crate::{
    error::{Error, Result},
    JsonValue,
};

const WASMCLOUD_MODEL_NAMESPACE: &str = "org.wasmcloud.model";
const WASMCLOUD_CORE_NAMESPACE: &str = "org.wasmcloud.core";
const WASMCLOUD_ACTOR_NAMESPACE: &str = "org.wasmcloud.actor";

const TRAIT_CODEGEN_RUST: &str = "codegenRust";
// If any of these are needed, they would have to be defined in core namespace
//const TRAIT_CODEGEN_ASM: &str = "codegenAsm";
//const TRAIT_CODEGEN_GO: &str = "codegenGo";
//const TRAIT_CODEGEN_TINYGO: &str = "codegenTinyGo";

const TRAIT_SERIALIZATION: &str = "serialization";
const TRAIT_WASMBUS: &str = "wasmbus";
const TRAIT_WASMBUS_DATA: &str = "wasmbusData";
const TRAIT_FIELD_NUM: &str = "n";
const TRAIT_RENAME: &str = "rename";

lazy_static! {
    static ref WASMCLOUD_MODEL_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE);
    static ref WASMCLOUD_CORE_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE);
    static ref WASMCLOUD_ACTOR_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_ACTOR_NAMESPACE);
    static ref SERIALIZATION_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_SERIALIZATION).unwrap(),
        None
    );
    static ref CODEGEN_RUST_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_CODEGEN_RUST).unwrap(),
        None
    );
    static ref WASMBUS_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_WASMBUS).unwrap(),
        None
    );
    static ref WASMBUS_DATA_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_WASMBUS_DATA).unwrap(),
        None
    );
    static ref FIELD_NUM_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_FIELD_NUM).unwrap(),
        None
    );
    static ref RENAME_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_RENAME).unwrap(),
        None
    );
    static ref UNIT_ID: ShapeID = ShapeID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE, "Unit", None);
}

/// namespace for org.wasmcloud.model
pub fn wasmcloud_model_namespace() -> &'static NamespaceID {
    &WASMCLOUD_MODEL_NAMESPACE_ID
}
pub fn wasmcloud_core_namespace() -> &'static NamespaceID {
    &WASMCLOUD_CORE_NAMESPACE_ID
}
pub fn wasmcloud_actor_namespace() -> &'static NamespaceID {
    &WASMCLOUD_ACTOR_NAMESPACE_ID
}

#[cfg(feature = "wasmbus")]
/// shape id of trait @wasmbus
pub fn wasmbus_trait() -> &'static ShapeID {
    &WASMBUS_TRAIT_ID
}

#[allow(dead_code)]
#[cfg(feature = "wasmbus")]
/// shape id of trait @wasmbusData
pub fn wasmbus_data_trait() -> &'static ShapeID {
    &WASMBUS_DATA_TRAIT_ID
}

/// shape id of trait @serialization
pub fn serialization_trait() -> &'static ShapeID {
    &SERIALIZATION_TRAIT_ID
}

/// shape id of trait @codegenRust
pub fn codegen_rust_trait() -> &'static ShapeID {
    &CODEGEN_RUST_TRAIT_ID
}

/// shape id of trait @n
pub fn field_num_trait() -> &'static ShapeID {
    &FIELD_NUM_TRAIT_ID
}

/// shape id of trait @rename
pub fn rename_trait() -> &'static ShapeID {
    &RENAME_TRAIT_ID
}

pub fn unit_shape() -> &'static ShapeID {
    &UNIT_ID
}

#[allow(dead_code)]
pub enum CommentKind {
    Inner,
    Documentation,
    /// inside a multi-line quote, as in python
    InQuote,
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct WasmbusProtoVersion {
    base: u8, // base number
}

impl TryFrom<&str> for WasmbusProtoVersion {
    type Error = crate::error::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "0" => Ok(WasmbusProtoVersion { base: 0 }),
            "2" => Ok(WasmbusProtoVersion { base: 2 }),
            _ => Err(Error::Model(format!(
                "Invalid wasmbus.protocol: '{value}'. The default value is \"0\"."
            ))),
        }
    }
}

impl fmt::Debug for WasmbusProtoVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.to_string())
    }
}

impl ToString for WasmbusProtoVersion {
    fn to_string(&self) -> String {
        format!("{}", self.base)
    }
}

impl WasmbusProtoVersion {
    pub fn has_cbor(&self) -> bool {
        self.base >= 2
    }
}

// Modifiers on data type
// This enum may be extended in the future if other variations are required.
// It's recursively composable, so you could represent &Option<&Value>
// with `Ty::Ref(Ty::Opt(Ty::Ref(id)))`
pub(crate) enum Ty<'typ> {
    /// write a plain shape declaration
    Shape(&'typ ShapeID),
    /// write a type wrapped in Option<>
    Opt(&'typ ShapeID),
    /// write a reference type: preceded by &
    Ref(&'typ ShapeID),

    /// write a ptr type: preceded by *
    Ptr(&'typ ShapeID),
}

// verify that the model doesn't contain unsupported types
#[macro_export]
macro_rules! expect_empty {
    ($list:expr, $msg:expr) => {
        if !$list.is_empty() {
            return Err(Error::InvalidModel(format!(
                "{}: {}",
                $msg,
                $list.keys().map(|k| k.to_string()).collect::<Vec<String>>().join(",")
            )));
        }
    };
}

#[macro_export]
macro_rules! unsupported_shape {
    ($fn_name:ident, $shape_type:ty, $doc:expr) => {
        #[allow(unused_variables)]
        fn $fn_name(
            &mut self,
            id: &ShapeID,
            traits: &AppliedTraits,
            shape: &$shape_type,
        ) -> Result<()> {
            return Err(weld_codegen::error::Error::UnsupportedShape(
                id.to_string(),
                $doc.to_string(),
            ));
        }
    };
}

/// true if namespace matches, or if there is no namespace constraint
pub fn is_opt_namespace(id: &ShapeID, ns: &Option<NamespaceID>) -> bool {
    match ns {
        Some(ns) => id.namespace() == ns,
        None => true,
    }
}

/// Finds the operation in the model or returns error
pub fn get_operation<'model>(
    model: &'model Model,
    operation_id: &'_ ShapeID,
    service_id: &'_ Identifier,
) -> Result<(&'model Operation, &'model AppliedTraits)> {
    let op = model
        .shapes()
        .filter(|t| t.id() == operation_id)
        .find_map(|t| {
            if let ShapeKind::Operation(op) = t.body() {
                Some((op, t.traits()))
            } else {
                None
            }
        })
        .ok_or_else(|| {
            Error::Model(format!(
                "missing operation {} for service {}",
                &operation_id.to_string(),
                &service_id.to_string()
            ))
        })?;
    Ok(op)
}

/// Returns trait as deserialized object, or None if the trait is not defined.
/// Returns error if the deserialization failed.
pub fn get_trait<T: DeserializeOwned>(traits: &AppliedTraits, id: &ShapeID) -> Result<Option<T>> {
    match traits.get(id) {
        Some(Some(val)) => match trait_value(val) {
            Ok(obj) => Ok(Some(obj)),
            Err(e) => Err(e),
        },
        Some(None) => Ok(None),
        None => Ok(None),
    }
}

/// Returns Ok(Some( version )) if this is a service with the wasmbus protocol trait
/// Returns Ok(None) if this is not a wasmbus service
/// Returns Err() if there was an error parsing the declarataion
pub fn wasmbus_proto(traits: &AppliedTraits) -> Result<Option<WasmbusProtoVersion>> {
    match get_trait(traits, wasmbus_trait()) {
        Ok(Some(Wasmbus { protocol: Some(version), .. })) => {
            Ok(Some(WasmbusProtoVersion::try_from(version.as_str())?))
        }
        Ok(_) => Ok(Some(WasmbusProtoVersion::default())),
        _ => Ok(None),
    }
}

/// Convert trait object to its native type
pub fn trait_value<T: DeserializeOwned>(value: &NodeValue) -> Result<T> {
    let json = value_to_json(value);
    let obj = serde_json::from_value(json)?;
    Ok(obj)
}

/// Convert smithy model 'Value' to a json object
pub fn value_to_json(value: &NodeValue) -> JsonValue {
    match value {
        NodeValue::None => JsonValue::Null,
        NodeValue::Array(v) => JsonValue::Array(v.iter().map(value_to_json).collect()),
        NodeValue::Object(v) => {
            let mut object = crate::JsonMap::default();
            for (k, v) in v {
                let _ = object.insert(k.clone(), value_to_json(v));
            }
            JsonValue::Object(object)
        }
        NodeValue::Number(v) => match v {
            Number::Integer(v) => JsonValue::Number((*v).into()),
            Number::Float(v) => JsonValue::Number(serde_json::Number::from_f64(*v).unwrap()),
        },
        NodeValue::Boolean(v) => JsonValue::Bool(*v),
        NodeValue::String(v) => JsonValue::String(v.clone()),
    }
}

/// resolve shape to its underlying shape
/// e.g., if you have a declaration "string Foo",
/// it will resolve Foo into smithy.api#String
pub fn resolve<'model>(model: &'model Model, shape: &'model ShapeID) -> &'model ShapeID {
    if let Some(resolved) = model.shape(shape) {
        resolved.id()
    } else {
        shape
    }
}

/// Returns true if the type has a natural default (zero, empty set/list/map, etc.).
/// Doesn't work for user-defined structs, only (most) simple types,
/// and set, list, and map.
///
/// This can be used for deserialization,
/// to allow missing fields to be filled in with the default.
///
/// The smithy developers considered and rejected the idea of being able to declare
/// a default value that is not zero (such as http status with a default 200),
/// which would be in the realm of business logic and outside the scope of smithy.
/// This default only applies to simple types that have a zero value,
/// and empty sets, list, and maps.
pub fn has_default(model: &'_ Model, member: &MemberShape) -> bool {
    let id = resolve(model, member.target());
    #[allow(unused_mut)]
    let mut has = false;
    let name = id.shape_name().to_string();

    if id.namespace().eq(prelude_namespace_id()) {
        cfg_if::cfg_if! {
            if #[cfg(feature = "BigInteger")] {
                has = has || &name == "bigInteger";
            }
        }
        cfg_if::cfg_if! {
            if #[cfg(feature = "BigDecimal")] {
                has = has || &name == "bigDecimal";
            }
        }
        has || matches!(
            name.as_str(),
            // some aggregate types
            "List" | "Set" | "Map"
            // most simple types
            | "Blob" | "Boolean" | "String" | "Byte" | "Short"
            | "Integer" | "Long" | "Float" | "Double"
            | "Timestamp"
        )
        // excluded: Resource, Operation, Service, Document, Union
    } else if id.namespace() == wasmcloud_model_namespace() {
        matches!(
            name.as_str(),
            "U64" | "U32" | "U16" | "U8" | "I64" | "I32" | "I16" | "I8" | "F64" | "F32"
        )
    } else {
        false
        // for any other type, return false.
        // if there was a need to override this,
        // a trait could be added
    }
}

pub(crate) struct NumberedMember {
    field_num: Option<u16>,
    shape: MemberShape,
}

impl NumberedMember {
    pub(crate) fn new(member: &MemberShape) -> Result<Self> {
        Ok(NumberedMember {
            shape: member.to_owned(),
            field_num: get_trait::<u16>(member.traits(), field_num_trait()).map_err(|e| {
                Error::Model(format!(
                    "invalid field number @n() for field '{}': {}",
                    member.id(),
                    e
                ))
            })?,
        })
    }

    pub(crate) fn field_num(&self) -> &Option<u16> {
        &self.field_num
    }
}

impl Deref for NumberedMember {
    type Target = MemberShape;

    fn deref(&self) -> &Self::Target {
        &self.shape
    }
}

use std::iter::Iterator;

use crate::wasmbus_model::Wasmbus;

/// Returns sorted list of fields for the structure, and whether it is numbered.
/// If there are any errors in numbering, returns Error::Model
pub(crate) fn get_sorted_fields(
    id: &Identifier,
    strukt: &StructureOrUnion,
) -> Result<(Vec<NumberedMember>, bool)> {
    let mut fields = strukt
        .members()
        .map(NumberedMember::new)
        .collect::<Result<Vec<NumberedMember>>>()?;
    let has_numbers = crate::model::has_field_numbers(&fields, &id.to_string())?;
    // Sort fields for deterministic output
    // by number, if declared with numbers, otherwise by name
    if has_numbers {
        fields.sort_by_key(|f| f.field_num().unwrap());
    } else {
        fields.sort_by_key(|f| f.id().to_owned());
    }
    Ok((fields, has_numbers))
}

/// Checks whether a struct has complete and valid field numbers.
/// Returns true if all fields have unique numbers.
/// Returns false if no fields are numbered.
/// Returns Err if fields are incompletely numbered, or there are duplicate numbers.
pub(crate) fn has_field_numbers(fields: &[NumberedMember], name: &str) -> Result<bool> {
    let mut numbered = std::collections::BTreeSet::default();
    for f in fields.iter() {
        if let Some(n) = f.field_num() {
            numbered.insert(*n);
        }
    }
    if numbered.is_empty() {
        Ok(false)
    } else if numbered.len() == fields.len() {
        // all fields are numbered uniquely
        Ok(true)
    } else {
        Err(crate::Error::Model(format!(
            "structure {name} has incomplete or invalid field numbers: either some fields are missing \
             the '@n()' trait, or some fields have duplicate numbers."
        )))
    }
}

/*
pub fn get_metadata(model: &Model) -> JsonMap {
    let mut metadata_map = JsonMap::default();
    for (key, value) in model.metadata() {
        let _ = metadata_map.insert(key.to_string(), value_to_json(value));
    }
    metadata_map
}
 */

/// Map namespace to package
///   rust: crate_name
///   other-languages: TBD
#[derive(Clone, Deserialize)]
pub struct PackageName {
    pub namespace: String,
    #[serde(rename = "crate")]
    pub crate_name: Option<String>,
    #[serde(rename = "py_module")]
    pub py_module: Option<String>,
    pub go_package: Option<String>,
    pub doc: Option<String>,
}
