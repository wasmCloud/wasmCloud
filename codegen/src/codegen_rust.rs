use crate::{
    error::{Error, Result},
    model::{
        actor_receiver_trait, capability_trait, expect_member, provider_receiver_trait,
        wasmcloud_namespace, CommentKind, ModelIndex, Shape,
    },
    writer::ToBytes,
    CodeGen,
};
use atelier_core::{
    model::{
        shapes::{
            AppliedTraits, HasTraits, ListOrSet, Map as MapShape, MemberShape, Operation, Service,
            Simple, StructureOrUnion,
        },
        values::Value,
        HasIdentity, ShapeID,
    },
    prelude::{
        prelude_namespace_id, prelude_shape_named, SHAPE_BLOB, SHAPE_BOOLEAN, SHAPE_BYTE,
        SHAPE_DOUBLE, SHAPE_FLOAT, SHAPE_INTEGER, SHAPE_LONG, SHAPE_PRIMITIVEBOOLEAN,
        SHAPE_PRIMITIVEBYTE, SHAPE_PRIMITIVEDOUBLE, SHAPE_PRIMITIVEFLOAT, SHAPE_PRIMITIVEINTEGER,
        SHAPE_PRIMITIVELONG, SHAPE_PRIMITIVESHORT, SHAPE_SHORT, SHAPE_STRING, TRAIT_DEPRECATED,
        TRAIT_DOCUMENTATION, TRAIT_TRAIT, TRAIT_UNSTABLE,
    },
};
use bytes::BytesMut;
use std::string::ToString;

const DEFAULT_MAP_TYPE: &str = "std::collections::HashMap";
const DEFAULT_LIST_TYPE: &str = "Vec";
const DEFAULT_SET_TYPE: &str = "std::collections::BTreeSet";

/// declarations for sorting. First sort key is the type (simple, then map, then struct).
/// In rust, sorting by BytesMut as the second key will result in sort by item name.
#[derive(Eq, Ord, PartialOrd, PartialEq)]
struct Declaration(u8, BytesMut);

#[derive(Default)]
pub struct RustCodeGen {
    writer: BytesMut,
}

impl CodeGen for RustCodeGen {
    /// load files, parse, and generate output
    fn init(&mut self, _ix: &ModelIndex) -> Result<()> {
        Ok(())
    }

    fn write_source_file_header(&mut self, ix: &ModelIndex) -> Result<()> {
        self.write(
            br#"// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
use wasmcloud_weld_rpc::{
    client, context, deserialize, serialize, MessageDispatch, RpcError, Transport, Message,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
"#,
        );

        self.write(&format!(
            "\npub const SMITHY_VERSION : &str = \"{}\";\n\n",
            ix.version.unwrap().to_string()
        ));
        Ok(())
    }

    // Declare simple types, then maps, then structures
    fn declare_types(&mut self, ix: &ModelIndex) -> Result<()> {
        for Shape(id, traits, simple) in ix.simples.values() {
            self.declare_simple_shape(id, traits, simple)?;
        }
        for Shape(id, traits, map) in ix.maps.values() {
            self.declare_map_shape(id, traits, map)?;
        }
        for Shape(id, traits, list) in ix.lists.values() {
            self.declare_list_or_set_shape(id, traits, list, DEFAULT_LIST_TYPE)?;
        }
        for Shape(id, traits, set) in ix.sets.values() {
            self.declare_list_or_set_shape(id, traits, set, DEFAULT_SET_TYPE)?;
        }
        for Shape(id, traits, strukt) in ix.structs.values().filter(|shape| {
            shape
                .1
                .get(&prelude_shape_named(TRAIT_TRAIT).unwrap())
                .is_none()
        }) {
            self.declare_struct_shape(id, traits, strukt)?;
        }
        Ok(())
    }

    fn write_services(&mut self, ix: &ModelIndex) -> Result<()> {
        for Shape(id, traits, service) in ix.services.values() {
            self.write_service_interface(ix, id, traits, service)?;
            self.write_service_receiver(ix, id, traits, service)?;
            self.write_service_sender(ix, id, traits, service)?;
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, kind: CommentKind, line: &str) {
        self.write(match kind {
            CommentKind::Documentation => "/// ",
            CommentKind::Inner => "// ",
        });
        self.write(line);
        self.write(b"\n");
    }

    #[inline]
    fn write<B: ToBytes>(&mut self, bytes: B) {
        self.writer.extend_from_slice(bytes.to_bytes());
    }

    /// Returns the current buffer, zeroing out self
    fn take(&mut self) -> BytesMut {
        self.writer.split_to(self.writer.len())
    }

    /// returns rust source file extension "rs"
    fn get_file_extension(&self) -> &'static str {
        "rs"
    }
}

impl RustCodeGen {
    /// Apply documentation traits: documentation, deprecated, unstable
    fn apply_documentation_traits(&mut self, id: &ShapeID, traits: &AppliedTraits) {
        if let Some(Some(Value::String(text))) =
            traits.get(&prelude_shape_named(TRAIT_DOCUMENTATION).unwrap())
        {
            self.write_documentation(id, text);
        }

        // directionality
        if traits.get(actor_receiver_trait()).is_some() {
            self.write_comment(CommentKind::Documentation, "@direction(actorReceiver)");
        }
        if traits.get(provider_receiver_trait()).is_some() {
            self.write_comment(CommentKind::Documentation, "@direction(providerReceiver)");
        }
        // capability contract id
        if let Some(Some(Value::Object(map))) = traits.get(capability_trait()) {
            if let Some(Value::String(contract_id)) = map.get("capabilityId") {
                self.write_comment(
                    CommentKind::Documentation,
                    &format!("capabilityContractId({})", contract_id),
                );
            }
        }
        // deprecated
        if let Some(Some(Value::Object(map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            self.write(b"#[deprecated(");
            if let Some(Value::String(since)) = map.get("since") {
                self.write(&format!("since=\"{}\"\n", since));
            }
            if let Some(Value::String(message)) = map.get("message") {
                self.write(&format!("note=\"{}\"\n", message));
            }
            self.write(b")\n");
        }
        // unstable
        if traits
            .get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap())
            .is_some()
        {
            self.write_comment(CommentKind::Documentation, "@unstable");
        }
        /*
        // private (internal only, hidden from doc
        // Commented out for now because I don't think 'hidden' is the right interpretation of private.
        // If we were generating an internal sdk, we would want to document the item.
        if traits
            .get(&prelude_shape_named(TRAIT_PRIVATE).unwrap())
            .is_some()
        {
            self.write(b"#[doc(hidden)]\n");
        }
         */
    }

    /// Write a type name, either a primitive or defined type.
    fn write_type(&mut self, id: &ShapeID) -> Result<()> {
        let name = id.shape_name().to_string();
        if id.namespace() == prelude_namespace_id() {
            let ty = match name.as_ref() {
                SHAPE_BLOB => "Vec<u8>",
                SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool",
                SHAPE_STRING => "String",
                SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "i8",
                SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "i16",
                SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "i32",
                SHAPE_LONG | SHAPE_PRIMITIVELONG => "i64",
                SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "float32",
                SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "float64",
                // SHAPE_BIGINTEGER
                // SHAPE_BIGDECIMAL
                // SHAPE_TIMESTAMP
                // SHAPE_DOCUMENT
                _ => return Err(Error::UnsupportedType(name)),
            };
            self.write(ty);
        } else if id.namespace() == wasmcloud_namespace() {
            let ty = match name.as_ref() {
                "U64" => "u64",
                "U32" => "u32",
                "U8" => "u8",
                "I64" => "i64",
                "I32" => "i32",
                "I8" => "i8",
                "U16" => "u16",
                "I16" => "i16",
                other => other, // return Err(Error::UnsupportedType(name)),
            };
            self.write(ty);
        } else {
            // TODO: need to be able to lookup from namespace to canonical module path
            self.write(&self.to_type_name(&id.shape_name().to_string()));
        }
        Ok(())
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    fn write_type_with_suffix(&mut self, id: &ShapeID, suffix: &str) -> Result<()> {
        self.write_type(id)?;
        self.write(suffix); // assume it's already PascalCalse
        Ok(())
    }

    // declaration for simple type
    fn declare_simple_shape(
        &mut self,
        id: &ShapeID,
        traits: &AppliedTraits,
        simple: &Simple,
    ) -> Result<()> {
        self.apply_documentation_traits(id, traits);
        self.write(b"pub type ");
        self.write_type(id)?;
        self.write(b" = ");
        let ty = match simple {
            Simple::Blob => "Vec<u8>",
            Simple::Boolean => "bool",
            Simple::String => "String",
            Simple::Byte => "i8",
            Simple::Short => "i16",
            Simple::Integer => "i32",
            Simple::Long => "i64",
            Simple::Float => "f32",
            Simple::Double => "f64",
            Simple::Document => return Err(Error::UnsupportedDocument),
            Simple::BigInteger => return Err(Error::UnsupportedBigInteger),
            Simple::BigDecimal => return Err(Error::UnsupportedBigDecimal),
            Simple::Timestamp => return Err(Error::UnsupportedTimestamp),
        };
        self.write(ty);
        self.write(b";\n\n");
        Ok(())
    }

    fn declare_map_shape(
        &mut self,
        id: &ShapeID,
        traits: &AppliedTraits,
        shape: &MapShape,
    ) -> Result<()> {
        self.apply_documentation_traits(id, traits);
        self.write(b"pub type ");
        self.write_type(id)?;
        self.write(b" = ");
        self.write(DEFAULT_MAP_TYPE);
        self.write(b"<");
        self.write_type(shape.key().target())?;
        self.write(b",");
        self.write_type(shape.value().target())?;
        self.write(b">;\n\n");
        Ok(())
    }

    fn declare_list_or_set_shape(
        &mut self,
        id: &ShapeID,
        traits: &AppliedTraits,
        shape: &ListOrSet,
        typ: &str,
    ) -> Result<()> {
        self.apply_documentation_traits(id, traits);
        self.write(b"pub type ");
        self.write_type(id)?;
        self.write(b" = ");
        self.write(typ);
        self.write(b"<");
        self.write_type(shape.member().target())?;
        self.write(b">;\n\n");
        Ok(())
    }

    fn declare_struct_shape(
        &mut self,
        id: &ShapeID,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        self.apply_documentation_traits(id, traits);
        self.write(b"#[derive(Debug, Clone, Serialize, Deserialize)]\n");
        self.write(b"pub struct ");
        self.write_type(id)?;
        self.write(b" {\n");
        for member in strukt.members() {
            self.apply_documentation_traits(member.id(), member.traits());
            let declared_name = expect_member(member.id())?;
            let rust_field_name = self.to_field_name(member.id())?;
            if declared_name != rust_field_name {
                self.write(&format!("#[serde(rename=\"{}\")] ", declared_name));
            }
            if !member.is_required() {
                self.write(r#"#[serde(default, skip_serializing_if = "Option::is_none")]"#);
            }
            self.write(&rust_field_name);
            self.write(b": ");
            self.write_field_type(member)?;
            self.write(b",\n");
        }
        self.write(b"}\n\n");
        Ok(())
    }

    /// write field type, optionally surrounded by Option<>
    fn write_field_type(&mut self, field: &MemberShape) -> Result<()> {
        let is_optional = !field.is_required();
        if is_optional {
            self.write(b"Option<");
        }
        self.write_type(field.id())?;
        if is_optional {
            self.write(b">");
        }
        Ok(())
    }

    /// Declares the service as a rust Trait whose methods are the smithy service operations
    fn write_service_interface(
        &mut self,
        ix: &ModelIndex,
        service_id: &ShapeID,
        traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        self.apply_documentation_traits(service_id, traits);
        self.write(b"#[async_trait]\npub trait ");
        self.write_type(service_id)?;
        self.write(b"{\n");
        for method_id in service.operations() {
            let Shape(_, traits, op) = ix.get_operation(service_id, method_id)?;
            self.write_method_signature(method_id, traits, op)?;
            self.write(b";\n");
        }
        self.write(b"}\n\n");
        Ok(())
    }

    /// write trait function declaration "async fn method(args) -> Result< return_type, RpcError >"
    /// does not write trailing semicolon so this can be used for declaration and implementation
    fn write_method_signature(
        &mut self,
        id: &ShapeID,
        traits: &AppliedTraits,
        op: &Operation,
    ) -> Result<()> {
        let method_name = self.to_method_name(id);
        self.apply_documentation_traits(id, traits);
        self.write(b"async fn ");
        self.write(&method_name);
        self.write(b"(&self, context: &context::Context<'_>");
        if let Some(input_type) = op.input() {
            self.write(b", arg: ");
            self.write_type(input_type)?;
        }
        self.write(b") -> Result<");
        if let Some(output_type) = op.output() {
            self.write_type(output_type)?;
        } else {
            self.write(b"()");
        }
        self.write(b">");
        Ok(())
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        ix: &ModelIndex,
        id: &ShapeID,
        traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Receiver receives messages defined in the {} service trait",
            id.shape_name(),
            id.shape_name()
        );
        self.write_comment(CommentKind::Documentation, &doc);
        self.apply_documentation_traits(id, traits);
        self.write(b"#[async_trait]\npub trait ");
        self.write_type_with_suffix(id, "Receiver")?;
        self.write(b" : MessageDispatch + ");
        self.write_type(id)?;
        self.write(
            br#"{
            async fn dispatch(
                &self,
                ctx: &context::Context<'_>,
                message: &Message<'_> ) -> Result< Message<'static>, RpcError> {
                match message.method {
        "#,
        );

        for method_id in service.operations() {
            let Shape(_, _, op) = ix.get_operation(id, method_id)?;
            self.write(b"\"");
            self.write(&self.op_dispatch_name(method_id));
            self.write(b"\" => {\n");
            if op.has_input() {
                // let value : InputType = deserialize(...)?;
                self.write(b"let value: ");
                self.write_type(op.input().as_ref().unwrap())?;
                self.write(b" = deserialize(message.arg.as_ref())?;\n");
            }
            // let resp = Trait::method(self, ctx, &value).await?;
            self.write(b"let resp = ");
            self.write_type(id)?; // Service::method
            self.write(b"::");
            self.write(&self.to_method_name(method_id));
            self.write(b"(self, ctx");
            if op.has_input() {
                self.write(b", &value");
            }
            self.write(b").await?;\n");

            // deserialize result
            self.write(b"let buf = Cow::Owned(serialize(&resp)?);\n");
            self.write(b"Ok(Message { method: ");
            self.write(&self.full_dispatch_name(id, method_id));
            self.write(b", arg: buf })},\n");
        }
        self.write(b"_ => Err(RpcError::MethodNotHandled(format!(\"");
        self.write_type(id)?;
        self.write(b"::{}\", message.method))),\n");
        self.write(b"}\n}\n}\n\n"); // end match, end fn dispatch, end trait

        Ok(())
    }

    /// writes the service sender struct and constructor
    // pub struct FooSender{ ... }
    fn write_service_sender(
        &mut self,
        ix: &ModelIndex,
        id: &ShapeID,
        traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Sender sends messages to a {} service",
            id.shape_name(),
            id.shape_name()
        );
        self.write_comment(CommentKind::Documentation, &doc);
        self.apply_documentation_traits(id, traits);
        self.write(b"#[derive(Debug)]\npub struct ");
        self.write_type_with_suffix(id, "Sender")?;
        self.write(b"<T> { transport: T, config: client::SendConfig }\n\n");

        // implement constructor for TraitClient
        self.write(b"impl<T:Transport>  ");
        self.write_type_with_suffix(id, "Sender")?;
        self.write(b"<T> { \n");
        self.write(b" pub fn new(config: SendConfig, transport: T) -> Self { ");
        self.write_type_with_suffix(id, "Sender")?;
        self.write(b"{ transport, config }\n}\n}\n\n");

        // implement Trait for TraitSender
        self.write(b"#[async_trait]\nimpl<T:Transport + std::marker::Sync + std::marker::Send> ");
        self.write_type(id)?;
        self.write(b" for ");
        self.write_type_with_suffix(id, "Sender")?;
        self.write(b"<T> {\n");

        for method_id in service.operations() {
            let Shape(_, traits, op) = ix.get_operation(id, method_id)?;
            self.write(b"#[allow(unused)]\n");
            self.write_method_signature(method_id, traits, op)?;
            self.write(b" {\n");

            if op.has_input() {
                self.write(b"let arg = serialize(value)?;\n");
            } else {
                self.write(b"let arg = *b\"\";\n");
            }
            self.write(b"let resp = self.transport.send(ctx, &self.config, Message{ method: ");
            // TODO: switch to quoted full method (increment api version # if not legacy)
            //self.write(self.full_dispatch_name(trait_base.id(), method_id));
            // note: legacy is just the latter part
            self.write(b"\"");
            self.write(&self.op_dispatch_name(method_id));
            self.write(b"\", arg: Cow::Borrowed(&arg)}).await?;\n");
            if op.has_output() {
                self.write(b"let value = deserialize(resp.arg.as_ref())?; Ok(value)");
            } else {
                self.write(b"Ok(())");
            }
            self.write(b" }\n");
        }
        self.write(b"}\n\n");
        Ok(())
    }
} // impl CodeGenRust
