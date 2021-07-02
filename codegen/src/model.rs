//! Smithy model helpers
//! - some constants used for model validation
//! - ModelIndex is a "cache" of a smithy model grouped by shape kind and sorted by identifier name
//! - various macros used in the codegen crate
//!
//use crate::strings::{to_pascal_case, to_snake_case};
use crate::error::{Error, Result};
use atelier_core::model::{
    shapes::{AppliedTraits, HasTraits, Operation, ShapeKind},
    HasIdentity, Identifier, Model, NamespaceID, ShapeID,
};
use lazy_static::lazy_static;
use std::str::FromStr;

const WASMCLOUD_MODEL_NAMESPACE: &str = "org.wasmcloud.model";
const WASMCLOUD_CORE_NAMESPACE: &str = "org.wasmcloud.core";

const TRAIT_ACTOR_RECEIVER: &str = "actorReceiver";
const TRAIT_CAPABILITY: &str = "capability";
const TRAIT_PROVIDER_RECEIVER: &str = "providerReceiver";
const TRAIT_CODEGEN_RUST: &str = "codegenRust";

lazy_static! {
    static ref WASMCLOUD_MODEL_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE);
    static ref WASMCLOUD_CORE_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE);
    static ref ACTOR_RECEIVER_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE),
        Identifier::from_str(TRAIT_ACTOR_RECEIVER).unwrap(),
        None
    );
    static ref CODEGEN_RUST_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_MODEL_NAMESPACE),
        Identifier::from_str(TRAIT_CODEGEN_RUST).unwrap(),
        None
    );
    static ref PROVIDER_RECEIVER_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE),
        Identifier::from_str(TRAIT_PROVIDER_RECEIVER).unwrap(),
        None
    );
    static ref CAPABILITY_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_CORE_NAMESPACE),
        Identifier::from_str(TRAIT_CAPABILITY).unwrap(),
        None
    );
}

pub fn wasmcloud_model_namespace() -> &'static NamespaceID {
    &WASMCLOUD_MODEL_NAMESPACE_ID
}

pub fn actor_receiver_trait() -> &'static ShapeID {
    &ACTOR_RECEIVER_TRAIT_ID
}
pub fn provider_receiver_trait() -> &'static ShapeID {
    &PROVIDER_RECEIVER_TRAIT_ID
}
pub fn capability_trait() -> &'static ShapeID {
    &CAPABILITY_TRAIT_ID
}
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
