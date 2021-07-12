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
use atelier_core::model::{
    shapes::{AppliedTraits, HasTraits, Operation, ShapeKind},
    values::{Number, Value as NodeValue},
    HasIdentity, Identifier, Model, NamespaceID, ShapeID,
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
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE),
        Identifier::from_str(TRAIT_WASMBUS).unwrap(),
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

/*
/// if member is not present during deserialization we can fill in the natural default value
/// This doesn't actually work .. the member type is a user-defined type, which in turn is a map or a list.
/// To make it work, we need to follow the chain of types to get to the leaf/underlying-type.
pub fn has_default(member: &MemberShape) -> bool {
    let id = member.target();
    id == &ShapeID::new_unchecked("smithy.api", "List", None)
        || id == &ShapeID::new_unchecked("smithy.api", "Map", None)
}
 */

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
