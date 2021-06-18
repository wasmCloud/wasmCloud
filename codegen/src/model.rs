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
    prelude::PRELUDE_NAMESPACE,
    Version,
};
use lazy_static::lazy_static;
use std::collections::BTreeMap;
use std::str::FromStr;

pub const WASMCLOUD_PRELUDE_NAMESPACE: &str = "wasmcloud.core";
pub const SMITHY_API_NAMESPACE: &str = "smithy.api";
pub const TRAIT_ACTOR_RECEIVER: &str = "actorReceiver";
pub const TRAIT_CAPABILITY: &str = "capability";
pub const TRAIT_PROVIDER_RECEIVER: &str = "providerReceiver";

lazy_static! {
    static ref WASMCLOUD_NAMESPACE_ID: NamespaceID =
        NamespaceID::new_unchecked(WASMCLOUD_PRELUDE_NAMESPACE);
    static ref DOCUMENTATION_TRAIT: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(PRELUDE_NAMESPACE),
        Identifier::from_str(atelier_core::prelude::TRAIT_DOCUMENTATION).unwrap(),
        None
    );
    static ref ACTOR_RECEIVER_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_PRELUDE_NAMESPACE),
        Identifier::from_str(TRAIT_ACTOR_RECEIVER).unwrap(),
        None
    );
    static ref PROVIDER_RECEIVER_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_PRELUDE_NAMESPACE),
        Identifier::from_str(TRAIT_PROVIDER_RECEIVER).unwrap(),
        None
    );
    static ref CAPABILITY_TRAIT_ID: ShapeID = ShapeID::new(
        NamespaceID::new_unchecked(WASMCLOUD_PRELUDE_NAMESPACE),
        Identifier::from_str(TRAIT_CAPABILITY).unwrap(),
        None
    );
}

pub fn wasmcloud_namespace() -> &'static NamespaceID {
    &WASMCLOUD_NAMESPACE_ID
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

pub enum CommentKind {
    Inner,
    Documentation,
}

/*
#[derive(Clone, Eq, PartialEq, PartialOrd, Ord, Serialize)]
pub struct ShapeIdent {
    pub namespace: NamespaceID,
    pub shape: String,
    pub member: Option<String>,
}
impl ShapeIdent {
    pub fn namespace(&self) -> &NamespaceID {
        &self.namespace
    }
    pub fn shape_name(&self) -> &str {
        &self.shape
    }
    pub fn member_name(&self) -> Option<&String> {
        self.member.as_ref()
    }
}

impl From<ShapeID> for ShapeIdent {
    fn from(s: ShapeID) -> ShapeIdent {
        ShapeIdent {
            namespace: s.namespace().to_owned(),
            shape: s.shape_name().to_string(),
            member: s.member_name().map(|i| i.to_string()),
        }
    }
}

impl From<&ShapeID> for ShapeIdent {
    fn from(s: &ShapeID) -> ShapeIdent {
        ShapeIdent {
            namespace: s.namespace().to_owned(),
            shape: s.shape_name().to_string(),
            member: s.member_name().map(|i| i.to_string()),
        }
    }
}
impl ToString for ShapeIdent {
    fn to_string(&self) -> String {
        match self.member {
            None => format!("{}#{}", &self.namespace, &self.shape),
            Some(m) => format!("{}#{}${}", &self.namespace, &self.shape, &m),
        }
    }
}
*/

pub struct Shape<'model, K>(
    pub &'model ShapeID,
    pub &'model AppliedTraits,
    pub &'model K,
);
type ShapeIndex<'model, K> = BTreeMap<&'model ShapeID, Shape<'model, K>>;
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
                    index.simples.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::List(body) => {
                    index.lists.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Set(body) => {
                    index.sets.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Map(body) => {
                    index.maps.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Structure(body) => {
                    index.structs.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Union(body) => {
                    index.unions.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Service(body) => {
                    index.services.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Operation(body) => {
                    index.operations.insert(id, Shape(id, shape.traits(), body));
                }
                ShapeKind::Resource(body) => {
                    index.resources.insert(id, Shape(id, shape.traits(), body));
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
        service_id: &ShapeID,
        method_id: &ShapeID,
    ) -> std::result::Result<&Shape<'model, Operation>, crate::error::Error> {
        self.operations
            .get(service_id)
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

/*
/// Select model shapes based on ShapeKind
/// Returns vector of (ShapeID, traits, shape) , sorted by ShapeID
#[macro_export]
macro_rules! select_shapes {
    ( $model:expr, $kind:ident, $cls:ident ) => {{
        let mut _shape_matches__ = $model
            .shapes()
            .filter_map(|s| match s.body() {
                ShapeKind::$kind(shape) => Some((s.id(), s.traits(), shape)),
                _ => None,
            })
            .collect::<Vec<(&ShapeID, &AppliedTraits, _)>>();
        _shape_matches__.sort_by(|a, b| a.0.shape_name().partial_cmp(b.0.shape_name()).unwrap());
        _shape_matches__
    }};
}
 */

/*
/// expect shape is a particular kind, otherwise raise error
#[macro_export]
macro_rules! expect_shape {
    ($top:expr, $kind:ident, $msg:expr) => {
        match $top.body() {
            ShapeKind::$kind(shape) => Ok(shape),
            _ => Err(Error::InvalidModel(format!(
                "{} is not a/an {}",
                $top.id(),
                $msg
            ))),
        }
    };
}
 */

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
   // returns first namespace as parent-namespace, name
   // For example,
   //     "namespace a.b.c" => Some("a.b", "c)
   //     "namespace frog" => Some("","frog")
   pub fn primary_namespace(&self) -> Option<(String, String)> {
       if self.namespaces().is_empty() {
           None
       } else {
           let ns: String = self.namespaces().get(0).unwrap().to_string();
           match ns.rsplit_once(atelier_core::syntax::SHAPE_ID_NAMESPACE_SEPARATOR) {
               None => Some((String::default(), ns.to_string())),
               Some((left, right)) => Some((left.to_string(), right.to_string())),
           }
       }
   }

*/

/*
   /// This is not a complete validation check, but it does some sanity checking before we begin writing files
   /// we can add rules to here over time
   pub fn validate_model(&self) -> Result<()> {
       let mut num_services = 0u32;

       for top in self.model.shapes() {
           match top.body() {
               // supported shapes
               ShapeKind::Simple(_)
               | ShapeKind::Map(_)
               | ShapeKind::Structure(_)
               | ShapeKind::Operation(_) => { /* ok */ }

               // service is supported; count them
               ShapeKind::Service(_) => {
                   num_services += 1;
               }

               // don't support these yet
               // make it a fatal error for now; we might want to fail with warning instead ..
               ShapeKind::List(_)
               | ShapeKind::Resource(_)
               | ShapeKind::Set(_)
               | ShapeKind::Union(_)
               | ShapeKind::Unresolved => {
                   return Err(Error::InvalidModel(format!(
                       "identifier {} has an unsupported model shape {:?}",
                       top.id(),
                       top.body(),
                   )));
               }
           }
       }
       if num_services == 0 {
           return Err(Error::InvalidModel(
               "there are no services defined in this model".to_string(),
           ));
       }
       Ok(())
   }
*/

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
