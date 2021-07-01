//! Smithy model helpers
//! - some constants used for model validation
//! - ModelIndex is a "cache" of a smithy model grouped by shape kind and sorted by identifier name
//! - various macros used in the codegen crate
//!
//use crate::strings::{to_pascal_case, to_snake_case};
use crate::error::Error;
use atelier_core::{
    model::{
        shapes::{self, AppliedTraits, HasTraits, Operation, ShapeKind},
        values::Value,
        HasIdentity, Identifier, Model, NamespaceID, ShapeID,
    },
    Version,
};
use lazy_static::lazy_static;
use std::collections::BTreeMap;
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

//pub fn wasmcloud_core_namespace() -> &'static NamespaceID {
//    &WASMCLOUD_CORE_NAMESPACE_ID
//}

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

pub struct IxShape<'model, K>(
    pub &'model ShapeID,
    pub &'model AppliedTraits,
    pub &'model K,
);
type ShapeIndex<'model, K> = BTreeMap<&'model ShapeID, IxShape<'model, K>>;
type UnresolvedIndex<'model> = BTreeMap<&'model ShapeID, &'model AppliedTraits>;

/// Index to model shapes and metadata
#[derive(Default)]
pub struct ModelIndex<'model> {
    pub version: Option<&'model Version>,
    pub metadata: BTreeMap<&'model String, &'model Value>,
    pub simples: ShapeIndex<'model, shapes::Simple>,
    pub lists: ShapeIndex<'model, shapes::ListOrSet>,
    pub sets: ShapeIndex<'model, shapes::ListOrSet>,
    pub maps: ShapeIndex<'model, shapes::Map>,
    pub structs: ShapeIndex<'model, shapes::StructureOrUnion>,
    pub unions: ShapeIndex<'model, shapes::StructureOrUnion>,
    pub services: ShapeIndex<'model, shapes::Service>,
    pub operations: ShapeIndex<'model, shapes::Operation>,
    pub resources: ShapeIndex<'model, shapes::Resource>,
    pub unresolved: UnresolvedIndex<'model>,
}

impl<'model> ModelIndex<'model> {
    pub fn build(model: &'model Model) -> Self {
        let mut index = ModelIndex::<'_> {
            version: Some(model.smithy_version()),
            ..Default::default()
        };
        for (key, value) in model.metadata() {
            index.metadata.insert(key, value);
        }

        for shape in model.shapes() {
            let id = shape.id();
            match &shape.body() {
                ShapeKind::Simple(body) => {
                    index.simples.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::List(body) => {
                    index.lists.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Set(body) => {
                    index.sets.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Map(body) => {
                    index.maps.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Structure(body) => {
                    index.structs.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Union(body) => {
                    index.unions.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Service(body) => {
                    index.services.insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Operation(body) => {
                    index
                        .operations
                        .insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Resource(body) => {
                    index
                        .resources
                        .insert(id, IxShape(id, shape.traits(), body));
                }
                ShapeKind::Unresolved => {
                    index.unresolved.insert(id, shape.traits());
                }
            }
        }
        index
    }

    pub fn get_operation(
        &'model self,
        service_id: &Identifier,
        method_id: &ShapeID,
    ) -> std::result::Result<&IxShape<'model, Operation>, crate::error::Error> {
        self.operations
            .get(method_id)
            .ok_or_else(|| Error::OperationNotFound(service_id.to_string(), method_id.to_string()))
    }
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
        ) -> std::result::Result<(), crate::error::Error> {
            return Err(crate::error::Error::UnsupportedShape(
                id.to_string(),
                $doc.to_string(),
            ));
        }
    };
}

/*
/// get member component of shape, or return error
pub fn expect_member(id: &ShapeID) -> Result<String, crate::error::Error> {
    Ok(id
        .member_name()
        .as_ref()
        .ok_or_else(|| {
            crate::error::Error::InvalidModel(format!("expecting member in {}", &id.to_string()))
        })?
        .to_string())
}
 */

impl<'model, T> IxShape<'model, T> {
    /*
    pub fn is_in_namespace(&self, ns: &NamespaceID) -> bool {
        self.0.namespace() == ns
    }
     */

    /// true if namespace matches, or if there is no namespace constraint
    pub fn is_opt_namespace(&self, ns: &Option<NamespaceID>) -> bool {
        match ns {
            Some(ns) => self.0.namespace() == ns,
            None => true,
        }
    }
}
