//! Rust language code-generator
//!
#[cfg(feature = "wasmbus")]
use crate::wasmbus_model::Wasmbus;
use crate::{
    config::LanguageConfig,
    error::{print_warning, Error, Result},
    gen::{CodeGen, SourceFormatter},
    model::{
        codegen_rust_trait, get_operation, get_trait, has_default, is_opt_namespace,
        serialization_trait, value_to_json, wasmcloud_model_namespace, CommentKind, PackageName,
    },
    render::Renderer,
    wasmbus_model::{CodegenRust, Serialization},
    writer::Writer,
    BytesMut, JsonValue, ParamMap,
};
use atelier_core::model::shapes::ShapeKind;
use atelier_core::{
    model::{
        shapes::{
            AppliedTraits, HasTraits, ListOrSet, Map as MapShape, MemberShape, Operation, Service,
            Simple, StructureOrUnion,
        },
        values::Value,
        HasIdentity, Identifier, Model, NamespaceID, ShapeID,
    },
    prelude::{
        prelude_namespace_id, prelude_shape_named, PRELUDE_NAMESPACE, SHAPE_BIGDECIMAL,
        SHAPE_BIGINTEGER, SHAPE_BLOB, SHAPE_BOOLEAN, SHAPE_BYTE, SHAPE_DOCUMENT, SHAPE_DOUBLE,
        SHAPE_FLOAT, SHAPE_INTEGER, SHAPE_LONG, SHAPE_PRIMITIVEBOOLEAN, SHAPE_PRIMITIVEBYTE,
        SHAPE_PRIMITIVEDOUBLE, SHAPE_PRIMITIVEFLOAT, SHAPE_PRIMITIVEINTEGER, SHAPE_PRIMITIVELONG,
        SHAPE_PRIMITIVESHORT, SHAPE_SHORT, SHAPE_STRING, SHAPE_TIMESTAMP, TRAIT_DEPRECATED,
        TRAIT_DOCUMENTATION, TRAIT_TRAIT, TRAIT_UNSTABLE,
    },
};
use std::{collections::HashMap, path::Path, str::FromStr, string::ToString};

const WASMBUS_RPC_CRATE: &str = "wasmbus_rpc";

const DEFAULT_MAP_TYPE: &str = "std::collections::HashMap";
const DEFAULT_LIST_TYPE: &str = "Vec";
const DEFAULT_SET_TYPE: &str = "std::collections::BTreeSet";
const DEFAULT_DOCUMENT_TYPE: &str = "Vec<u8>";

/// declarations for sorting. First sort key is the type (simple, then map, then struct).
/// In rust, sorting by BytesMut as the second key will result in sort by item name.
#[derive(Eq, Ord, PartialOrd, PartialEq)]
struct Declaration(u8, BytesMut);

type ShapeList<'model> = Vec<(&'model ShapeID, &'model AppliedTraits, &'model ShapeKind)>;

// Modifiers on data type
// This enum may be extended in the future if other variations are required.
// It's recursively composable, so you could represent &Option<&Value>
// with `Ty::Ref(Ty::Opt(Ty::Ref(id)))`
enum Ty<'typ> {
    /// write a plain shape declaration
    Shape(&'typ ShapeID),
    /// write a type wrapped in Option<>
    Opt(&'typ ShapeID),
    /// write a reference type: preceeded by &
    Ref(&'typ ShapeID),
}

#[derive(Default)]
pub struct RustCodeGen<'model> {
    /// if set, limits declaration output to this namespace only
    namespace: Option<NamespaceID>,
    packages: HashMap<String, PackageName>,
    import_core: String,
    model: Option<&'model Model>,
}

impl<'model> RustCodeGen<'model> {
    pub fn new(model: Option<&'model Model>) -> Self {
        Self {
            model,
            namespace: None,
            packages: HashMap::default(),
            import_core: String::default(),
        }
    }
}

enum MethodArgFlags {
    Normal,
    // arg is type ToString
    ToString,
}

impl<'model> CodeGen for RustCodeGen<'model> {
    /// Initialize code generator and renderer for language output.j
    /// This hook is called before any code is generated and can be used to initialize code generator
    /// and/or perform additional processing before output files are created.
    fn init(
        &mut self,
        model: Option<&Model>,
        _lc: &LanguageConfig,
        _output_dir: &Path,
        _renderer: &mut Renderer,
    ) -> std::result::Result<(), Error> {
        self.namespace = None;
        self.import_core = WASMBUS_RPC_CRATE.to_string();

        if let Some(model) = model {
            if let Some(packages) = model.metadata_value("package") {
                let packages: Vec<PackageName> = serde_json::from_value(value_to_json(packages))
                    .map_err(|e| Error::Model(format!("invalid metadata format for package, expecting format '[{{namespace:\"org.example\",crate:\"path::module\"}}]':  {}", e.to_string())))?;
                for p in packages.iter() {
                    self.packages.insert(p.namespace.to_string(), p.clone());
                }
            }
        }
        Ok(())
    }

    fn source_formatter(&self) -> Result<Box<dyn SourceFormatter>> {
        Ok(Box::new(crate::format::RustSourceFormatter::default()))
    }

    /// Perform any initialization required prior to code generation for a file
    /// `model` may be used to check model metadata
    /// `id` is a tag from codegen.toml that indicates which source file is to be written
    /// `namespace` is the namespace in the model to generate
    #[allow(unused_variables)]
    fn init_file(
        &mut self,
        w: &mut Writer,
        model: &Model,
        file_config: &crate::config::OutputFile,
        params: &ParamMap,
    ) -> Result<()> {
        self.namespace = match &file_config.namespace {
            Some(ns) => Some(NamespaceID::from_str(ns)?),
            None => None,
        };
        if let Some(ref ns) = self.namespace {
            if self.packages.get(&ns.to_string()).is_none() {
                print_warning(&format!(
                    concat!(
                        "no package metadata defined for namespace {}.",
                        " Add a declaration like this at the top of fhe .smithy file: ",
                        " metadata package = [ {{ namespace: \"{}\", crate: \"crate_name\" }} ]"
                    ),
                    ns, ns
                ));
            }
        }
        self.import_core = match params.get("crate") {
            Some(JsonValue::String(c)) if c == WASMBUS_RPC_CRATE => "crate".to_string(),
            _ => WASMBUS_RPC_CRATE.to_string(),
        };
        Ok(())
    }

    fn write_source_file_header(
        &mut self,
        w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        w.write(
            r#"// This file is generated automatically using wasmcloud-weld and smithy model definitions
               //
            "#);
        match &self.namespace {
            Some(n) if n == wasmcloud_model_namespace() => {
                // the base model has minimal dependencies
                w.write(
                    r#"
                #![allow(dead_code)]
                use serde::{{Deserialize, Serialize}};
             "#,
                );
            }
            _ => {
                // all others use standard frontmatter

                // special case for imports:
                // if the crate we are generating is "wasmbus_rpc" then we have to import it with "crate::".
                w.write(&format!(
                    r#"
                #![allow(clippy::ptr_arg)]
                #[allow(unused_imports)]
                use {}::{{
                    Context, deserialize, serialize, MessageDispatch, RpcError, RpcResult,
                    Transport, Message, SendOpts,
                }};
                #[allow(unused_imports)] use serde::{{Deserialize, Serialize}};
                #[allow(unused_imports)] use async_trait::async_trait;
                #[allow(unused_imports)] use std::{{borrow::Cow, string::ToString}};
                "#,
                    &self.import_core
                ));
            }
        }
        w.write(&format!(
            "\npub const SMITHY_VERSION : &str = \"{}\";\n\n",
            model.smithy_version().to_string()
        ));
        Ok(())
    }

    fn declare_types(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        let ns = self.namespace.clone();

        let mut shapes = model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
            .collect::<ShapeList>();
        // sort shapes (they are all in the same namespace if ns.is_some(), which is usually true)
        shapes.sort_by_key(|v| v.0);

        for (id, traits, shape) in shapes.into_iter() {
            match shape {
                ShapeKind::Simple(simple) => {
                    self.declare_simple_shape(&mut w, id.shape_name(), traits, simple)?;
                }
                ShapeKind::Map(map) => {
                    self.declare_map_shape(&mut w, id.shape_name(), traits, map)?;
                }
                ShapeKind::List(list) => {
                    self.declare_list_or_set_shape(
                        &mut w,
                        id.shape_name(),
                        traits,
                        list,
                        DEFAULT_LIST_TYPE,
                    )?;
                }
                ShapeKind::Set(set) => {
                    self.declare_list_or_set_shape(
                        &mut w,
                        id.shape_name(),
                        traits,
                        set,
                        DEFAULT_SET_TYPE,
                    )?;
                }
                ShapeKind::Structure(strukt) => {
                    //if !traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap()) {
                    self.declare_structure_shape(&mut w, id.shape_name(), traits, strukt)?;
                    //}
                }
                ShapeKind::Operation(_)
                | ShapeKind::Resource(_)
                | ShapeKind::Service(_)
                | ShapeKind::Union(_)
                | ShapeKind::Unresolved => {}
            }
        }
        Ok(())
    }

    fn write_services(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        let ns = self.namespace.clone();
        for (id, traits, shape) in model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
        {
            if let ShapeKind::Service(service) = shape {
                self.write_service_interface(&mut w, model, id.shape_name(), traits, service)?;
                self.write_service_receiver(&mut w, model, id.shape_name(), traits, service)?;
                self.write_service_sender(&mut w, model, id.shape_name(), traits, service)?;
            }
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, w: &mut Writer, kind: CommentKind, line: &str) {
        w.write(match kind {
            CommentKind::Documentation => "/// ",
            CommentKind::Inner => "// ",
        });
        w.write(line);
        w.write(b"\n");
    }

    /// returns rust source file extension "rs"
    fn get_file_extension(&self) -> &'static str {
        "rs"
    }
}

/// returns true if the file path ends in ".rs"
#[allow(clippy::ptr_arg)]
pub(crate) fn is_rust_source(path: &Path) -> bool {
    match path.extension() {
        Some(s) => s.to_string_lossy().as_ref() == "rs",
        _ => false,
    }
}

impl<'model> RustCodeGen<'model> {
    /// Apply documentation traits: (documentation, deprecated, unstable)
    fn apply_documentation_traits(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
    ) {
        if let Some(Some(Value::String(text))) =
            traits.get(&prelude_shape_named(TRAIT_DOCUMENTATION).unwrap())
        {
            self.write_documentation(&mut w, id, text);
        }

        // deprecated
        if let Some(Some(Value::Object(map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            w.write(b"#[deprecated(");
            if let Some(Value::String(since)) = map.get("since") {
                w.write(&format!("since=\"{}\"\n", since));
            }
            if let Some(Value::String(message)) = map.get("message") {
                w.write(&format!("note=\"{}\"\n", message));
            }
            w.write(b")\n");
        }

        // unstable
        if traits
            .get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap())
            .is_some()
        {
            self.write_comment(&mut w, CommentKind::Documentation, "@unstable");
        }
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    fn write_type(&mut self, w: &mut Writer, ty: Ty<'_>) -> Result<()> {
        match ty {
            Ty::Opt(id) => {
                w.write(b"Option<");
                self.write_type(w, Ty::Shape(id))?;
                w.write(b">");
            }
            Ty::Ref(id) => {
                w.write(b"&");
                self.write_type(w, Ty::Shape(id))?;
            }
            Ty::Shape(id) => {
                let name = id.shape_name().to_string();
                if id.namespace() == prelude_namespace_id() {
                    let ty = match name.as_ref() {
                        // Document are  Blob
                        SHAPE_BLOB => "Vec<u8>",
                        SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool",
                        SHAPE_STRING => "String",
                        SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "i8",
                        SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "i16",
                        SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "i32",
                        SHAPE_LONG | SHAPE_PRIMITIVELONG => "i64",
                        SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "f32",
                        SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "f64",
                        // if declared as members (of a struct, list, or map), we don't have trait data here to write
                        // as anything other than a blob. Instead, a type should be created for the Document that can have traits,
                        // and that type used for the member. This should probably be a lint rule.
                        SHAPE_DOCUMENT => DEFAULT_DOCUMENT_TYPE,
                        SHAPE_TIMESTAMP => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "Timestamp")] { "Timestamp" } else { return Err(Error::UnsupportedTimestamp) }
                            }
                        }
                        SHAPE_BIGINTEGER => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "BigInteger")] { "BigInteger" } else { return Err(Error::UnsupportedBigInteger) }
                            }
                        }
                        SHAPE_BIGDECIMAL => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "BigDecimal")] { "BigDecimal" } else { return Err(Error::UnsupportedBigDecimal) }
                            }
                        }
                        _ => return Err(Error::UnsupportedType(name)),
                    };
                    w.write(ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    match name.as_bytes() {
                        b"U64" | b"U32" | b"U16" | b"U8" => {
                            w.write(b"u");
                            w.write(&name.as_bytes()[1..])
                        }
                        b"I64" | b"I32" | b"I16" | b"I8" => {
                            w.write(b"i");
                            w.write(&name.as_bytes()[1..])
                        }
                        _ => {
                            if self.namespace.is_none()
                                || self.namespace.as_ref().unwrap() != wasmcloud_model_namespace()
                            {
                                w.write(&self.import_core);
                                w.write(b"::model::");
                            }
                            w.write(&self.to_type_name(&name));
                        }
                    };
                } else if self.namespace.is_some()
                    && id.namespace() == self.namespace.as_ref().unwrap()
                {
                    // we are in the same namespace so we don't need to specify namespace
                    w.write(&self.to_type_name(&id.shape_name().to_string()));
                } else {
                    match self.packages.get(&id.namespace().to_string()) {
                        Some(package) => {
                            // the crate name should be valid rust syntax. If not, they'll get an error with rustc
                            w.write(&package.crate_name);
                            w.write(b"::");
                            w.write(&self.to_type_name(&id.shape_name().to_string()));
                        }
                        None => {
                            return Err(Error::Model(format!("undefined create for namespace {} for symbol {}. Make sure codegen.toml includes all dependent namespaces",
                                    &id.namespace(), &id)));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    fn write_ident_with_suffix(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        suffix: &str,
    ) -> Result<()> {
        self.write_ident(&mut w, id);
        w.write(suffix); // assume it's already PascalCalse
        Ok(())
    }

    // declaration for simple type
    fn declare_simple_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        simple: &Simple,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
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

            // note: in the future, codegen traits may modify this
            Simple::Document => DEFAULT_DOCUMENT_TYPE,

            Simple::Timestamp => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "Timestamp")] { "Timestamp" } else { return Err(Error::UnsupportedTimestamp) }
                }
            }
            Simple::BigInteger => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "BigInteger")] { "BigInteger" } else { return Err(Error::UnsupportedBigInteger) }
                }
            }
            Simple::BigDecimal => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "BigDecimal")] { "BigDecimal" } else { return Err(Error::UnsupportedBigDecimal) }
                }
            }
        };
        w.write(ty);
        w.write(b";\n\n");
        Ok(())
    }

    fn declare_map_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &MapShape,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
        w.write(DEFAULT_MAP_TYPE);
        w.write(b"<");
        self.write_type(&mut w, Ty::Shape(shape.key().target()))?;
        w.write(b",");
        self.write_type(&mut w, Ty::Shape(shape.value().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_list_or_set_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &ListOrSet,
        typ: &str,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
        w.write(typ);
        w.write(b"<");
        self.write_type(&mut w, Ty::Shape(shape.member().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_structure_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let is_trait_struct = traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap());
        self.apply_documentation_traits(&mut w, id, traits);
        let mut derive_list = vec!["Clone", "Debug", "PartialEq", "Serialize", "Deserialize"];
        // derive(Default) is disabled for traits and enabled for all other structs
        let mut derive_default = !is_trait_struct;
        // derive(Eq) is enabled, unless specifically disabled in codegenRust
        let mut derive_eq = true;
        if let Some(cg) = get_trait::<CodegenRust>(traits, codegen_rust_trait())? {
            derive_default = !cg.no_derive_default;
            derive_eq = !cg.no_derive_eq;
        }
        if derive_default {
            derive_list.push("Default");
        }
        if derive_eq {
            derive_list.push("Eq");
        }
        derive_list.sort_unstable();
        let derive_decl = format!("#[derive({})]\n", derive_list.join(","));
        w.write(&derive_decl);
        w.write(b"pub struct ");
        self.write_ident(&mut w, id);
        w.write(b" {\n");
        // sort fields for deterministic output
        let mut fields = strukt
            .members()
            .map(|m| m.to_owned())
            .collect::<Vec<MemberShape>>();
        fields.sort_by_key(|f| f.id().to_owned());
        for member in fields.iter() {
            self.apply_documentation_traits(&mut w, member.id(), member.traits());

            // use the declared name for serialization, unless an override is declared
            // with `@sesrialization(name: SNAME)`
            let ser_name = if let Some(Serialization {
                name: Some(ser_name),
            }) = get_trait(member.traits(), serialization_trait())?
            {
                ser_name
            } else {
                member.id().to_string()
            };
            //let declared_name = member.id().to_string();
            let rust_field_name = self.to_field_name(member.id())?;
            if ser_name != rust_field_name {
                w.write(&format!("  #[serde(rename=\"{}\")] ", ser_name));
            }

            // for rmp-msgpack - need to use serde_bytes serializer for Blob (and Option<Blob>)
            // otherwise Vec<u8> is written as an array of individual bytes, not a memory slice.
            //
            // We should only add this serde declaration if the struct is tagged with @wasmbusData.
            // Because of the possibility of smithy models being used for messages
            // that don't use wasmbus protocols, we don't want to "automatically"
            // assume wasmbusData trait, even if we are compiled with (feature="wasmbus").
            //
            // However, we don't really need to require users to declare
            // structs with wasmbusData - we can infer it if it's used in an operation
            // for a service tagged with wasmbus. This would require traversing the model
            // from all services tagged with wasmbus, looking at the inputs and outputs
            // of all operations for those services, and, transitively, any
            // data types referred from them, including struct fields, list members,
            // and map keys and values.
            // Until that traversal is implemented, assume that wasmbusData is enabled
            // for everything. This saves developers from needing to add a wasmbusData
            // declaration on every struct, which is error-prone.
            // I can't think of a use case when adding serde_bytes is the wrong thing to do,
            // even if msgpack is not used for serialization, so it seems like
            // an acceptable simplification.
            #[cfg(feature = "wasmbus")]
            if member.target() == &ShapeID::new_unchecked("smithy.api", "Blob", None)
            //&& traits.get(wasmbus_data_trait()).is_some()
            {
                w.write(r#"  #[serde(with="serde_bytes")] "#);
            }

            if is_optional_type(member) {
                w.write(r#"  #[serde(default, skip_serializing_if = "Option::is_none")] "#);
            } else if (is_trait_struct && !member.is_required())
                || has_default(self.model.unwrap(), member)
            {
                // trait structs are deserialized only and need default values
                // on deserialization, so always add [serde(default)] for trait structs.

                // Additionally, add [serde(default)] for types that have a natural
                // default value. Although not required if both ends of the protocol
                // are implemented correctly, it may improve message resiliency
                // if we can accept structs with missing fields, if the fields
                // can be filled in/constructed with appropriate default values.
                // This only applies if the default is a zero, empty list/map, etc,
                // and we don't make any attempt to determine if a user-declared
                // struct has a zero default.
                // See the comment for has_default for more info.
                w.write(r#"  #[serde(default)] "#);
            }
            w.write(b"  pub ");
            w.write(&rust_field_name);
            w.write(b": ");
            self.write_field_type(&mut w, member)?;
            w.write(b",\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// write field type, wrapping with Option if field is not required
    fn write_field_type(&mut self, mut w: &mut Writer, field: &MemberShape) -> Result<()> {
        self.write_type(
            &mut w,
            if is_optional_type(field) {
                Ty::Opt(field.target())
            } else {
                Ty::Shape(field.target())
            },
        )
    }

    /// Declares the service as a rust Trait whose methods are the smithy service operations
    fn write_service_interface(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, service_id, service_traits);

        #[cfg(feature = "wasmbus")]
        self.add_wasmbus_comments(&mut w, service_id, service_traits)?;

        w.write(b"#[async_trait]\npub trait ");
        self.write_ident(&mut w, service_id);
        w.write(b"{\n");
        self.write_service_contract_getter(&mut w, service_id, service_traits)?;

        for operation in service.operations() {
            // if operation is not declared in this namespace, don't define it here
            if let Some(ref ns) = self.namespace {
                if operation.namespace() != ns {
                    continue;
                }
            }
            let (op, op_traits) = get_operation(model, operation, service_id)?;

            // TODO: re-think what to do if operation is in another namespace and self.namespace is None
            let method_id = operation.shape_name();
            let _flags = self.write_method_signature(&mut w, method_id, op_traits, op)?;
            w.write(b";\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// add getter for capability contract id
    fn write_service_contract_getter(
        &mut self,
        w: &mut Writer,
        _service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<()> {
        if let Some(Wasmbus {
            contract_id: Some(contract_id),
            ..
        }) = get_trait(service_traits, crate::model::wasmbus_trait())?
        {
            w.write(&format!(
                r#"
                /// returns the capability contract id for this interface
                fn contract_id() -> &'static str {{ "{}" }}
                "#,
                contract_id
            ));
        }
        Ok(())
    }

    #[cfg(feature = "wasmbus")]
    fn add_wasmbus_comments(
        &mut self,
        mut w: &mut Writer,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<()> {
        // currently the only thing we do with Wasmbus in codegen is add comments
        let wasmbus: Option<Wasmbus> = get_trait(service_traits, crate::model::wasmbus_trait())?;
        if let Some(wasmbus) = wasmbus {
            if let Some(contract) = wasmbus.contract_id {
                let text = format!("wasmbus.contractId: {}", &contract);
                self.write_documentation(&mut w, service_id, &text);
            }
            if wasmbus.provider_receive {
                let text = "wasmbus.providerReceive";
                self.write_documentation(&mut w, service_id, text);
            }
            if wasmbus.actor_receive {
                let text = "wasmbus.actorReceive";
                self.write_documentation(&mut w, service_id, text);
            }
        }
        Ok(())
    }

    /// write trait function declaration "async fn method(args) -> Result< return_type, RpcError >"
    /// does not write trailing semicolon so this can be used for declaration and implementation
    fn write_method_signature(
        &mut self,
        mut w: &mut Writer,
        method_id: &Identifier,
        method_traits: &AppliedTraits,
        op: &Operation,
    ) -> Result<MethodArgFlags> {
        let method_name = self.to_method_name(method_id);
        let mut arg_flags = MethodArgFlags::Normal;
        self.apply_documentation_traits(&mut w, method_id, method_traits);
        w.write(b"async fn ");
        w.write(&method_name);
        if let Some(input_type) = op.input() {
            if input_type == &ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_STRING, None) {
                arg_flags = MethodArgFlags::ToString;
                w.write("<TS:ToString + ?Sized + std::marker::Sync>");
            }
        }
        w.write(b"(&self, ctx: &Context");
        if let Some(input_type) = op.input() {
            w.write(b", arg: "); // pass arg by reference
            if matches!(arg_flags, MethodArgFlags::ToString) {
                w.write(b"&TS");
            } else {
                self.write_type(&mut w, Ty::Ref(input_type))?;
            }
        }
        w.write(b") -> RpcResult<");
        if let Some(output_type) = op.output() {
            self.write_type(&mut w, Ty::Shape(output_type))?;
        } else {
            w.write(b"()");
        }
        w.write(b">");
        Ok(arg_flags)
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Receiver receives messages defined in the {} service trait",
            service_id, service_id
        );
        self.write_comment(&mut w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(&mut w, service_id, service_traits);
        w.write(b"#[doc(hidden)]\n#[async_trait]\npub trait ");
        self.write_ident_with_suffix(&mut w, service_id, "Receiver")?;
        w.write(b" : MessageDispatch + ");
        self.write_ident(&mut w, service_id);
        w.write(
            br#"{
            async fn dispatch(
                &self,
                ctx: &Context,
                message: &Message<'_> ) -> RpcResult< Message<'_>> {
                match message.method {
        "#,
        );

        for method_id in service.operations() {
            // TODO: if it's valid for a service to include operations from another namespace, then this isn't doing the right thing
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, _) = get_operation(model, method_id, service_id)?;
            w.write(b"\"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\" => {\n");
            if op.has_input() {
                // let value : InputType = deserialize(...)?;
                w.write(b"let value: ");
                // TODO: should this be input.target?
                self.write_type(&mut w, Ty::Shape(op.input().as_ref().unwrap()))?;
                w.write(b" = deserialize(message.arg.as_ref())\
                  .map_err(|e| RpcError::Deser(format!(\"message '{}': {}\", message.method, e)))?;\n");
            }
            // let resp = Trait::method(self, ctx, &value).await?;
            w.write(b"let resp = ");
            self.write_ident(&mut w, service_id); // Service::method
            w.write(b"::");
            w.write(&self.to_method_name(method_ident));
            w.write(b"(self, ctx");
            if op.has_input() {
                w.write(b", &value");
            }
            w.write(b").await?;\n");

            // serialize result
            w.write(b"let buf = Cow::Owned(serialize(&resp)?);\n");
            //w.write(br#"console_log(format!("actor result {}b",buf.len())); "#);
            w.write(b"Ok(Message { method: \"");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            w.write(b"\", arg: buf })},\n");
        }
        w.write(b"_ => Err(RpcError::MethodNotHandled(format!(\"");
        self.write_ident(&mut w, service_id);
        w.write(b"::{}\", message.method))),\n");
        w.write(b"}\n}\n}\n\n"); // end match, end fn dispatch, end trait

        Ok(())
    }

    /// writes the service sender struct and constructor
    // pub struct FooSender{ ... }
    fn write_service_sender(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Sender sends messages to a {} service",
            service_id, service_id
        );
        self.write_comment(&mut w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(&mut w, service_id, service_traits);
        w.write(&format!(
            r#"/// client for sending {} messages
              #[derive(Debug)]
              pub struct {}Sender<T:Transport>  {{ transport: T }}

              impl<T:Transport> {}Sender<T> {{
                  /// Constructs a {}Sender with the specified transport
                  pub fn via(transport: T) -> Self {{
                      Self{{ transport }}
                  }}
              }}
            "#,
            service_id,
            service_id,
            service_id,
            service_id,
        ));
        #[cfg(feature = "wasmbus")]
        w.write(&self.actor_receive_sender_constructors(service_id, service_traits)?);
        #[cfg(feature = "wasmbus")]
        w.write(&self.provider_receive_sender_constructors(service_id, service_traits)?);

        // implement Trait for TraitSender
        w.write(b"#[async_trait]\nimpl<T:Transport + std::marker::Sync + std::marker::Send> ");
        self.write_ident(&mut w, service_id);
        w.write(b" for ");
        self.write_ident_with_suffix(&mut w, service_id, "Sender")?;
        w.write(b"<T> {\n");

        for method_id in service.operations() {
            // TODO: if it's valid for a service to include operations from another namespace, then this isn't doing the right thing
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();

            let (op, method_traits) = get_operation(model, method_id, service_id)?;
            w.write(b"#[allow(unused)]\n");
            let arg_flags = self.write_method_signature(&mut w, method_ident, method_traits, op)?;
            w.write(b" {\n");
            if op.has_input() {
                if matches!(arg_flags, MethodArgFlags::ToString) {
                    w.write(b"let arg = serialize(&arg.to_string())?;\n");
                } else {
                    w.write(b"let arg = serialize(arg)?;\n");
                }
            } else {
                w.write(b"let arg = *b\"\";\n");
            }
            w.write(b"let resp = self.transport.send(ctx, Message{ method: ");
            // note: legacy is just the latter part
            w.write(b"\"");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            //w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\", arg: Cow::Borrowed(&arg)}, None).await?;\n");
            if op.has_output() {
                w.write(
                    b"let value = deserialize(&resp)\
                   .map_err(|e| RpcError::Deser(format!(\"response to {}: {}\", \"",
                );
                w.write(&method_ident.to_string());
                w.write(b"\", e)))?; Ok(value)");
            } else {
                w.write(b"Ok(())");
            }
            w.write(b" }\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// add sender constructors for calling actors, for services that declare actorReceive
    #[cfg(feature = "wasmbus")]
    fn actor_receive_sender_constructors(
        &mut self,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<String> {
        let ctors = if let Some(Wasmbus {
                                    actor_receive: true,
                                    ..
                                }) = get_trait(service_traits, crate::model::wasmbus_trait())?
        {
            format!(
                r#"
                #[cfg(not(target_arch="wasm32"))]
                impl<'send> {}Sender<{}::provider::ProviderTransport<'send>> {{
                    /// Constructs a Sender using an actor's LinkDefinition,
                    /// Uses the provider's HostBridge for rpc
                    pub fn for_actor(ld: &'send {}::core::LinkDefinition) -> Self {{
                        Self{{ transport: {}::provider::ProviderTransport::new(ld,None) }}
                    }}
                }}
                #[cfg(target_arch = "wasm32")]
                impl {}Sender<{}::actor::prelude::WasmHost> {{
                    /// Constructs a client for actor-to-actor messaging
                    /// using the recipient actor's public key
                    pub fn to_actor(actor_id: &str) -> Self {{
                        let transport = {}::actor::prelude::WasmHost::to_actor(actor_id.to_string()).unwrap();
                        Self{{ transport }}
                    }}

                }}
                "#,
                // for_actor() (from provider)
                service_id,
                &self.import_core,
                &self.import_core,
                &self.import_core,

                // impl declaration
                service_id,
                &self.import_core,

                // to_actor() (from actor)
                &self.import_core,

            )
        } else {
            String::new()
        };
        Ok(ctors)
    }

    /// add sender constructors for actors calling providers
    /// This is only used for wasm32 targets and for services that declare 'providerReceive'
    #[cfg(feature = "wasmbus")]
    fn provider_receive_sender_constructors(
        &mut self,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<String> {
        let ctors = if let Some(Wasmbus {
            provider_receive: true,
            contract_id: Some(contract),
            ..
        }) = get_trait(service_traits, crate::model::wasmbus_trait())?
        {
            format!(
                r#"
                #[cfg(target_arch = "wasm32")]
                impl {}Sender<{}::actor::prelude::WasmHost> {{

                    /// Constructs a client for sending to a {} provider
                    /// implementing the '{}' capability contract, with the "default" link
                    pub fn new() -> Self {{
                        let transport = {}::actor::prelude::WasmHost::to_provider("{}", "default").unwrap();
                        Self {{ transport }}
                    }}

                    /// Constructs a client for sending to a {} provider
                    /// implementing the '{}' capability contract, with the specified link name
                    pub fn new_with_link(link_name: &str) -> {}::RpcResult<Self> {{
                        let transport =  {}::actor::prelude::WasmHost::to_provider("{}", link_name)?;
                        Ok(Self {{ transport }})
                    }}

                }}
                "#,
                // impl declaration
                service_id,
                &self.import_core,

                // new() (provider)
                service_id,
                contract,
                &self.import_core,
                contract,

                // new_with_link()
                service_id,
                contract,
                &self.import_core,
                &self.import_core,
                contract,
            )
        } else {
            String::new()
        };
        Ok(ctors)
    }
} // impl CodeGenRust

/// is_optional_type determines whether the field should be wrapped in Option<>
/// the value is true if it has an explicit `box` trait, or if it's
/// un-annotated and not one of (boolean, byte, short, integer, long, float, double)
fn is_optional_type(field: &MemberShape) -> bool {
    field.is_boxed()
        || (!field.is_required()
            && ![
                "Boolean", "Byte", "Short", "Integer", "Long", "Float", "Double",
            ]
            .contains(&field.target().shape_name().to_string().as_str()))
}

/*
Opt   @required   @box    bool/int/...
1     0           0       0
0     0           0       1
1     0           1       0
1     0           1       1
0     1           0       0
0     1           0       1
x     1           1       0
x     1           1       1
*/
