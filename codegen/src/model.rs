//! Smithy model helpers
//! - some constants used for model validation
//! - ModelIndex is a "cache" of a smithy model grouped by shape kind and sorted by identifier name
//! - various macros used in the codegen crate
//!
//use crate::strings::{to_pascal_case, to_snake_case};
use crate::{
    error::{Error, Result},
    JsonValue,
};
use atelier_core::{
    model::{
        shapes::{AppliedTraits, HasTraits, MemberShape, Operation, ShapeKind},
        values::{Number, Value as NodeValue},
        HasIdentity, Identifier, Model, NamespaceID, ShapeID,
    },
    prelude::prelude_namespace_id,
};
use lazy_static::lazy_static;
use serde::{de::DeserializeOwned, Deserialize};
use std::str::FromStr;

const WASMCLOUD_MODEL_NAMESPACE: &str = "org.wasmcloud.model";
const WASMCLOUD_CORE_NAMESPACE: &str = "org.wasmcloud.core";

//const TRAIT_ACTOR_RECEIVER: &str = "actorReceiver";
//const TRAIT_CAPABILITY: &str = "capability";
//const TRAIT_PROVIDER_RECEIVER: &str = "providerReceiver";
const TRAIT_CODEGEN_RUST: &str = "codegenRust";
const TRAIT_SERIALIZATION: &str = "serialization";
const TRAIT_WASMBUS: &str = "wasmbus";
const TRAIT_WASMBUS_DATA: &str = "wasmbusData";
//const TRAIT_SERIALIZE_RENAME: &str = "rename";

lazy_static! {
    static ref WASMCLOUD_MODEL_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE);
    static ref WASMCLOUD_CORE_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE);
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
}

/// namespace for org.wasmcloud.model
pub fn wasmcloud_model_namespace() -> &'static NamespaceID {
    &WASMCLOUD_MODEL_NAMESPACE_ID
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

#[allow(dead_code)]
pub enum CommentKind {
    Inner,
    Documentation,
}

// verify that the model doesn't contain unsupported types
#[macro_export]
macro_rules! expect_empty {
    ($list:expr, $msg:expr) => {
        if !$list.is_empty() {
            return Err(Error::InvalidModel(format!(
                "{}: {}",
                $msg,
                $list
                    .keys()
                    .map(|k| k.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
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
            return Err(crate::error::Error::UnsupportedShape(
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
        NodeValue::Array(v) => JsonValue::Array(v.iter().map(|v| value_to_json(v)).collect()),
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

    if id.namespace().eq(prelude_namespace_id()) {
        let name = id.shape_name().to_string();
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
    } else {
        false
        // for any other type, return false.
        // if there was a need to override this,
        // a trait could be added
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
    pub crate_name: String,
}
