//! utilities for model validation
//!
//!
use atelier_core::model::{
    shapes::{AppliedTraits, ListOrSet, Operation, Service, Simple, StructureOrUnion},
    visitor::{walk_model, ModelVisitor},
    Model, ShapeID,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

const MAX_DEPTH: usize = 8;

#[derive(Debug)]
enum NodeType {
    List,
    Map,
    Operation,
    Service,
    Structure,
    Simple,
    Union,
    Unknown,
}

/// Use Visitor pattern to build a tree of shapes and their dependencies.
/// Used for validation checks
struct ShapeTree {
    tree: RefCell<Tree>,
    services: RefCell<BTreeSet<ShapeID>>,
}

impl Default for ShapeTree {
    fn default() -> Self {
        ShapeTree {
            tree: RefCell::new(Tree::default()),
            services: RefCell::new(BTreeSet::default()),
        }
    }
}

/// Data structure containing dependency tree of all shapes in model
#[derive(Default)]
struct Tree {
    nodes: BTreeMap<ShapeID, Node>,
}

impl Tree {
    /// adds a parent-child relationship, ensuring nodes are created for both
    fn insert_parent_child(&mut self, parent_id: &ShapeID, child_id: &ShapeID) {
        let parent = self.get_or_insert(parent_id);
        parent.add_child(child_id.clone());

        let child = self.get_or_insert(child_id);
        child.add_parent(parent_id.clone());
    }

    /// Returns node if it is in the model
    fn get(&self, id: &ShapeID) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Returns the model node, creating a default instance if this is the first lookup
    fn get_or_insert(&mut self, id: &ShapeID) -> &mut Node {
        if !self.nodes.contains_key(id) {
            self.nodes.insert(id.clone(), Node::new(id.clone()));
        }
        self.nodes.get_mut(id).unwrap()
    }

    /// Returns true if the type is supported for cbor-only services
    fn is_cbor_only(&self, node: &Node) -> Option<String> {
        if let NodeType::Union = node.typ {
            Some(format!(
                "union '{}' in namespace: {}",
                node.id.shape_name(),
                node.id.namespace()
            ))
        } else {
            None
        }
    }

    /// Recursive node dump. Prints current value, and recurses up to max depts
    fn has_cbor_only(&self, node: &Node, depth: usize) -> Option<String> {
        if let Some(reason) = self.is_cbor_only(node) {
            Some(reason)
        } else if depth < MAX_DEPTH {
            node.children
                .iter()
                .filter_map(|child_id| {
                    self.nodes
                        .get(child_id)
                        .and_then(|c| self.has_cbor_only(c, depth + 1))
                })
                .next()
        } else {
            None
        }
    }
}

impl ModelVisitor for ShapeTree {
    type Error = String;

    fn simple_shape(
        &self,
        id: &ShapeID,
        _traits: &AppliedTraits,
        _shape: &Simple,
    ) -> Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Simple;
        Ok(())
    }

    fn list(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        list: &ListOrSet,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::List;
        tree.insert_parent_child(id, list.member().target());
        Ok(())
    }

    fn map(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        map: &atelier_core::model::shapes::Map,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Map;
        tree.insert_parent_child(id, map.key().target());
        tree.insert_parent_child(id, map.value().target());
        Ok(())
    }

    fn structure(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Structure;
        for field in strukt.members() {
            tree.insert_parent_child(id, field.target());
        }
        Ok(())
    }

    fn union(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Union;
        for field in strukt.members() {
            tree.insert_parent_child(id, field.target());
        }
        Ok(())
    }

    fn service(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        service: &Service,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Service;
        for operation in service.operations() {
            tree.insert_parent_child(id, operation);
        }
        let mut services = self.services.borrow_mut();
        services.insert(id.clone());
        Ok(())
    }

    fn operation(
        &self,
        id: &ShapeID,
        _: &AppliedTraits,
        operation: &Operation,
    ) -> std::result::Result<(), Self::Error> {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.get_or_insert(id);
        node.typ = NodeType::Operation;
        if let Some(input) = operation.input() {
            tree.insert_parent_child(id, input);
        }
        if let Some(output) = operation.output() {
            tree.insert_parent_child(id, output);
        }
        Ok(())
    }
}

struct Node {
    id: ShapeID,
    parents: BTreeSet<ShapeID>,
    children: BTreeSet<ShapeID>,
    typ: NodeType,
}

impl Node {
    fn new(id: ShapeID) -> Self {
        Self {
            id,
            parents: BTreeSet::new(),
            children: BTreeSet::new(),
            typ: NodeType::Unknown,
        }
    }

    fn add_parent(&mut self, parent: ShapeID) {
        self.parents.insert(parent);
    }

    fn add_child(&mut self, child: ShapeID) {
        self.children.insert(child);
    }
}

/// Check the model for structures that require cbor serialization that are
/// used by a service operation (directly or indirectly), if the service uses non-cbor serialization.
/// This validation rule returns the first incompatibility found, not a list of all incompatibilities
pub(crate) fn check_cbor_dependencies(model: &Model) -> Result<(), String> {
    use atelier_core::model::shapes::HasTraits as _;
    let visitor = ShapeTree::default();
    let _ = walk_model(model, &visitor).expect("walk model");
    for service_id in visitor.services.borrow().iter() {
        let service = model.shape(service_id).unwrap();
        let traits = service.traits();
        let proto = crate::model::wasmbus_proto(traits).map_err(|e| e.to_string())?;
        let service_has_cbor = proto.map(|pv| pv.has_cbor()).unwrap_or(false);
        // we only need to check for cbor conflicts on services that haven't declared cbor proto
        if !service_has_cbor {
            let tree = visitor.tree.borrow();
            let node = tree.get(service_id).unwrap();
            if let Some(reason) = tree.has_cbor_only(node, 0) {
                return Err(format!(
                    "Service {}.{} must be declared @wasmbus{{protocol: \"2\"}} due to a \
                     dependency on {}",
                    service_id.namespace(),
                    service_id.shape_name(),
                    reason
                ));
            }
        }
    }
    Ok(())
}

/// Perform model validation - language-independent validation
/// Returns Ok(), or a list of one or more errors
/// TODO: these should also be called from `wash validate`
pub(crate) fn validate(model: &Model) -> Result<(), Vec<String>> {
    if let Err(msg) = check_cbor_dependencies(model) {
        Err(vec![msg])
    } else {
        Ok(())
    }
}
