//! Rust language code-generator
//!
#[cfg(feature = "wasmbus")]
use crate::wasmbus_model::Wasmbus;
use crate::{
    config::{LanguageConfig, OutputLanguage},
    error::{print_warning, Error, Result},
    gen::{CodeGen, SourceFormatter},
    model::{
        codegen_rust_trait, get_operation, get_sorted_fields, get_trait, has_default,
        is_opt_namespace, serialization_trait, value_to_json, wasmcloud_model_namespace,
        CommentKind, PackageName, Ty,
    },
    render::Renderer,
    wasmbus_model::{CodegenRust, Serialization},
    writer::Writer,
    BytesMut, JsonValue, ParamMap,
};
use atelier_core::{
    model::{
        shapes::{
            AppliedTraits, HasTraits, ListOrSet, Map as MapShape, MemberShape, Operation, Service,
            ShapeKind, Simple, StructureOrUnion,
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

pub struct RustCodeGen<'model> {
    /// if set, limits declaration output to this namespace only
    pub(crate) namespace: Option<NamespaceID>,
    pub(crate) packages: HashMap<String, PackageName>,
    pub(crate) import_core: String,
    pub(crate) model: Option<&'model Model>,
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

struct ServiceInfo<'model> {
    id: &'model Identifier,
    traits: &'model AppliedTraits,
    service: &'model Service,
}

impl<'model> ServiceInfo<'model> {
    fn wasmbus_contract_id(&self) -> Option<String> {
        match get_trait(self.traits, crate::model::wasmbus_trait()) {
            Ok(Some(Wasmbus {
                contract_id: Some(contract_id),
                ..
            })) => Some(contract_id),
            _ => None,
        }
    }
}

#[non_exhaustive]
enum MethodArgFlags {
    Normal,
    // arg is type ToString
    ToString,
}

/// Returns true if the type is a rust primitive
pub fn is_rust_primitive(id: &ShapeID) -> bool {
    (id.namespace() == prelude_namespace_id()
        && matches!(
            id.shape_name().to_string().as_str(),
            "Boolean" | "Byte" | "Short" | "Integer" | "Long" | "Float" | "Double"
        ))
        || (id.namespace() == wasmcloud_model_namespace()
            && matches!(
                id.shape_name().to_string().as_str(),
                "U64" | "U32" | "U16" | "U8" | "I64" | "I32" | "I16" | "I8" | "F64" | "F32"
            ))
}

impl<'model> CodeGen for RustCodeGen<'model> {
    fn output_language(&self) -> OutputLanguage {
        OutputLanguage::Rust
    }

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
            if let Some(Value::Array(codegen_min)) = model.metadata_value("codegen") {
                let current_ver =
                    semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| {
                        Error::InvalidModel(format!(
                            "parse error for weld-codegen package version: {}",
                            e
                        ))
                    })?;
                for val in codegen_min.iter() {
                    if let Value::Object(map) = val {
                        if let Some(Value::String(lang)) = map.get("language") {
                            if lang.as_str() == "rust" {
                                if let Some(Value::String(ver)) = map.get("min_version") {
                                    let min_ver = semver::Version::parse(ver).map_err(|e| {
                                        Error::InvalidModel(format!(
                                            "metadata parse error for codegen {{ language=rust, \
                                             min_version={} }}: {}",
                                            ver, e
                                        ))
                                    })?;
                                    if min_ver.gt(&current_ver) {
                                        return Err(Error::Model(format!(
                                            "model requires weld-codegen version >= {}",
                                            min_ver
                                        )));
                                    }
                                } else {
                                    return Err(Error::Model(
                                        "missing 'min_version' in metadata.codegen for lang=rust"
                                            .to_string(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(packages) = model.metadata_value("package") {
                let packages: Vec<PackageName> = serde_json::from_value(value_to_json(packages))
                    .map_err(|e| {
                        Error::Model(format!(
                            "invalid metadata format for package, expecting format \
                             '[{{namespace:\"org.example\",crate:\"path::module\"}}]':  {}",
                            e
                        ))
                    })?;
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
        params: &ParamMap,
    ) -> Result<()> {
        w.write(&format!(
            "// This file is generated automatically using wasmcloud/weld-codegen {}\n",
            env!("CARGO_PKG_VERSION")
        ));
        match &self.namespace {
            Some(n) if n == wasmcloud_model_namespace() => {
                // the base model has minimal dependencies
                w.write("#[allow(unused_imports)] use serde::{{Deserialize, Serialize}};\n");
                w.write(b"#[allow(unused_imports)] use minicbor::{Encode,encode::Write};\n");
                if !params.contains_key("no_serde") {
                    w.write(&format!(
                        "#[allow(unused_imports)] use {}::error::{{RpcError,RpcResult}};\n",
                        self.import_core
                    ));
                }
            }
            _ => {
                // all others use standard frontmatter

                // special case for imports:
                // if the crate we are generating is "wasmbus_rpc" then we have to import it with "crate::".
                w.write(&format!(
                    r#"
                #[allow(unused_imports)]
                use {}::{{
                    cbor::*,
                    common::{{
                        Context, deserialize, Message, MessageFormat, message_format, 
                        MessageDispatch, SendOpts, serialize, Transport,
                    }},
                    error::{{RpcError,RpcResult}},
                    Timestamp,
                }};
                #[allow(unused_imports)]
                use serde::{{Deserialize, Serialize}};
                #[allow(unused_imports)]
                use async_trait::async_trait;
                #[allow(unused_imports)]
                use std::{{borrow::Borrow, borrow::Cow, io::Write, string::ToString}};
                "#,
                    &self.import_core,
                ));
            }
        }
        w.write(&format!(
            "\npub const SMITHY_VERSION : &str = \"{}\";\n\n",
            model.smithy_version()
        ));
        Ok(())
    }

    fn declare_types(&mut self, w: &mut Writer, model: &Model, params: &ParamMap) -> Result<()> {
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
                    self.declare_simple_shape(w, id.shape_name(), traits, simple)?;
                }
                ShapeKind::Map(map) => {
                    self.declare_map_shape(w, id.shape_name(), traits, map)?;
                }
                ShapeKind::List(list) => {
                    self.declare_list_or_set_shape(
                        w,
                        id.shape_name(),
                        traits,
                        list,
                        DEFAULT_LIST_TYPE,
                    )?;
                }
                ShapeKind::Set(set) => {
                    self.declare_list_or_set_shape(
                        w,
                        id.shape_name(),
                        traits,
                        set,
                        DEFAULT_SET_TYPE,
                    )?;
                }
                ShapeKind::Structure(strukt) => {
                    self.declare_structure_shape(w, id.shape_name(), traits, strukt)?;
                }
                ShapeKind::Union(strukt) => {
                    self.declare_union_shape(w, id.shape_name(), traits, strukt)?;
                }
                ShapeKind::Operation(_)
                | ShapeKind::Resource(_)
                | ShapeKind::Service(_)
                | ShapeKind::Unresolved => {}
            }

            // If the shape is not a trait, and ser-deser isn't disabled, generate encoder and decoder
            // It's ok to declare cbor shape encoders & decoders even if not used by
            // this service's protocol version, because this shape might be used in other interfaces
            // that _do_ use cbor, and because the cbor en-/de- coders won't be added
            // to the compiler output of they aren't used
            if !traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap())
                && !params.contains_key("no_serde")
            //&& env!("CARGO_PKG_NAME") != "weld-codegen"
            // wasmbus-rpc can't be as dependency of weld-codegen or it creates circular dependencies
            {
                self.declare_shape_encoder(w, id, shape)?;
                self.declare_shape_decoder(w, id, shape)?;
            }
        }
        Ok(())
    }

    fn write_services(&mut self, w: &mut Writer, model: &Model, _params: &ParamMap) -> Result<()> {
        let ns = self.namespace.clone();
        let mut services: Vec<(&ShapeID, &AppliedTraits, &ShapeKind)> = model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
            .collect();
        // sort services in this namespace, so output order is deterministic
        services.sort_by_key(|me| me.0);
        for (id, traits, shape) in services.iter() {
            if let ShapeKind::Service(service) = shape {
                let service = ServiceInfo {
                    id: id.shape_name(),
                    service,
                    traits,
                };
                self.write_service_interface(w, model, &service)?;
                self.write_service_receiver(w, model, &service)?;
                self.write_service_sender(w, model, &service)?;
            }
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, w: &mut Writer, kind: CommentKind, line: &str) {
        w.write(match kind {
            CommentKind::Documentation => "/// ",
            CommentKind::Inner => "// ",
            CommentKind::InQuote => "// ", // not applicable for Rust
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
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
    ) {
        if let Some(Some(Value::String(text))) =
            traits.get(&prelude_shape_named(TRAIT_DOCUMENTATION).unwrap())
        {
            self.write_documentation(w, id, text);
        }

        // '@deprecated' trait
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

        // '@unstable' trait
        if traits
            .get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap())
            .is_some()
        {
            self.write_comment(w, CommentKind::Documentation, "@unstable");
        }
    }

    /// field type, wrapped with Option if field is not required
    pub(crate) fn field_type_string(&self, field: &MemberShape) -> Result<String> {
        self.type_string(if is_optional_type(field) {
            Ty::Opt(field.target())
        } else {
            Ty::Shape(field.target())
        })
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    pub(crate) fn type_string(&self, ty: Ty<'_>) -> Result<String> {
        let mut s = String::new();
        match ty {
            Ty::Opt(id) => {
                s.push_str("Option<");
                s.push_str(&self.type_string(Ty::Shape(id))?);
                s.push('>');
            }
            Ty::Ref(id) => {
                s.push('&');
                s.push_str(&self.type_string(Ty::Shape(id))?);
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
                        SHAPE_TIMESTAMP => "Timestamp",
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
                    s.push_str(ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    match name.as_str() {
                        "U64" | "U32" | "U16" | "U8" => {
                            s.push('u');
                            s.push_str(&name[1..])
                        }
                        "I64" | "I32" | "I16" | "I8" => {
                            s.push('i');
                            s.push_str(&name[1..]);
                        }
                        "F64" => s.push_str("f64"),
                        "F32" => s.push_str("f32"),
                        _ => {
                            if self.namespace.is_none()
                                || self.namespace.as_ref().unwrap() != id.namespace()
                            {
                                s.push_str(&self.import_core);
                                s.push_str("::model::");
                            }
                            s.push_str(&self.to_type_name(&name));
                        }
                    };
                } else if self.namespace.is_some()
                    && id.namespace() == self.namespace.as_ref().unwrap()
                {
                    // we are in the same namespace so we don't need to specify namespace
                    s.push_str(&self.to_type_name(&id.shape_name().to_string()));
                } else {
                    match self.packages.get(&id.namespace().to_string()) {
                        Some(PackageName {
                            crate_name: Some(crate_name),
                            ..
                        }) => {
                            // the crate name should be valid rust syntax. If not, they'll get an error with rustc
                            s.push_str(crate_name);
                            s.push_str("::");
                            s.push_str(&self.to_type_name(&id.shape_name().to_string()));
                        }
                        _ => {
                            return Err(Error::Model(format!(
                                "undefined crate for namespace {} for symbol {}. Make sure \
                                 codegen.toml includes all dependent namespaces, and that the \
                                 dependent .smithy file contains package metadata with crate: \
                                 value",
                                &id.namespace(),
                                &id
                            )));
                        }
                    }
                }
            }
        }
        Ok(s)
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    fn write_type(&mut self, w: &mut Writer, ty: Ty<'_>) -> Result<()> {
        w.write(&self.type_string(ty)?);
        Ok(())
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    fn write_ident_with_suffix(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        suffix: &str,
    ) -> Result<()> {
        self.write_ident(w, id);
        w.write(suffix); // assume it's already PascalCalse
        Ok(())
    }

    // declaration for simple type
    fn declare_simple_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        simple: &Simple,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        w.write(b"pub type ");
        self.write_ident(w, id);
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
            Simple::Timestamp => "Timestamp",
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
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &MapShape,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        w.write(b"pub type ");
        self.write_ident(w, id);
        w.write(b" = ");
        w.write(DEFAULT_MAP_TYPE);
        w.write(b"<");
        self.write_type(w, Ty::Shape(shape.key().target()))?;
        w.write(b",");
        self.write_type(w, Ty::Shape(shape.value().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_list_or_set_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &ListOrSet,
        typ: &str,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        w.write(b"pub type ");
        self.write_ident(w, id);
        w.write(b" = ");
        w.write(typ);
        w.write(b"<");
        self.write_type(w, Ty::Shape(shape.member().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_structure_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let is_trait_struct = traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap());
        self.apply_documentation_traits(w, id, traits);
        let mut derive_list = vec!["Clone", "Debug", "PartialEq", "Serialize", "Deserialize"];
        // derive(Default) is disabled for traits and enabled for all other structs
        let mut derive_default = !is_trait_struct;
        // derive(Eq) is enabled, unless specifically disabled in codegenRust
        let mut derive_eq = true;
        let mut non_exhaustive = false;

        if let Some(cg) = get_trait::<CodegenRust>(traits, codegen_rust_trait())? {
            derive_default = !cg.no_derive_default;
            derive_eq = !cg.no_derive_eq;
            non_exhaustive = cg.non_exhaustive;
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
        if non_exhaustive {
            w.write(b"#[non_exhaustive]\n");
        }
        w.write(b"pub struct ");
        self.write_ident(w, id);
        w.write(b" {\n");
        let (fields, _is_numbered) = get_sorted_fields(id, strukt)?;
        for member in fields.iter() {
            self.apply_documentation_traits(w, member.id(), member.traits());
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
            let rust_field_name = self.to_field_name(member.id(), member.traits())?;
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
            if member.target() == &ShapeID::new_unchecked("smithy.api", "Blob", None) {
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
            w.write(
                format!(
                    "  pub {}: {},\n",
                    &rust_field_name,
                    self.field_type_string(member)?
                )
                .as_bytes(),
            );
        }
        w.write(b"}\n\n");
        Ok(())
    }

    fn declare_union_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let (fields, is_numbered) = get_sorted_fields(id, strukt)?;
        if !is_numbered {
            return Err(Error::Model(format!(
                "union {} must have numbered fields",
                id
            )));
        }
        self.apply_documentation_traits(w, id, traits);
        w.write(b"#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]\n");
        println!("Union: {}:\n:{:#?}", id, strukt);

        w.write(b"pub enum ");
        self.write_ident(w, id);
        w.write(b" {\n");
        for member in fields.iter() {
            self.apply_documentation_traits(w, member.id(), member.traits());
            let variant_name = self.to_type_name(&member.id().to_string());
            w.write(&format!(
                "{}({}),\n",
                variant_name,
                self.type_string(Ty::Shape(member.target()))?
            )); // TODO: Ty::Ref ?
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// Declares the service as a rust Trait whose methods are the smithy service operations
    fn write_service_interface(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service: &ServiceInfo,
    ) -> Result<()> {
        self.apply_documentation_traits(w, service.id, service.traits);

        #[cfg(feature = "wasmbus")]
        self.add_wasmbus_comments(w, service)?;

        w.write(b"#[async_trait]\npub trait ");
        self.write_ident(w, service.id);
        w.write(b"{\n");
        self.write_service_contract_getter(w, service)?;

        for operation in service.service.operations() {
            // if operation is not declared in this namespace, don't define it here
            if let Some(ref ns) = self.namespace {
                if operation.namespace() != ns {
                    continue;
                }
            }
            let (op, op_traits) = get_operation(model, operation, service.id)?;
            let method_id = operation.shape_name();
            let _flags = self.write_method_signature(w, method_id, op_traits, op)?;
            w.write(b";\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// add getter for capability contract id
    fn write_service_contract_getter(
        &mut self,
        w: &mut Writer,
        service: &ServiceInfo,
    ) -> Result<()> {
        if let Some(contract_id) = service.wasmbus_contract_id() {
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
    fn add_wasmbus_comments(&mut self, w: &mut Writer, service: &ServiceInfo) -> Result<()> {
        // currently the only thing we do with Wasmbus in codegen is add comments
        let wasmbus: Option<Wasmbus> = get_trait(service.traits, crate::model::wasmbus_trait())?;
        if let Some(wasmbus) = wasmbus {
            if let Some(contract_id) = service.wasmbus_contract_id() {
                let text = format!("wasmbus.contractId: {}", contract_id);
                self.write_documentation(w, service.id, &text);
            }
            if wasmbus.provider_receive {
                let text = "wasmbus.providerReceive";
                self.write_documentation(w, service.id, text);
            }
            if wasmbus.actor_receive {
                let text = "wasmbus.actorReceive";
                self.write_documentation(w, service.id, text);
            }
        }
        Ok(())
    }

    /// write trait function declaration "async fn method(args) -> Result< return_type, RpcError >"
    /// does not write trailing semicolon so this can be used for declaration and implementation
    fn write_method_signature(
        &mut self,
        w: &mut Writer,
        method_id: &Identifier,
        method_traits: &AppliedTraits,
        op: &Operation,
    ) -> Result<MethodArgFlags> {
        let method_name = self.to_method_name(method_id, method_traits);
        let mut arg_flags = MethodArgFlags::Normal;
        self.apply_documentation_traits(w, method_id, method_traits);
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
                self.write_type(w, Ty::Ref(input_type))?;
            }
        }
        w.write(b") -> RpcResult<");
        if let Some(output_type) = op.output() {
            self.write_type(w, Ty::Shape(output_type))?;
        } else {
            w.write(b"()");
        }
        w.write(b">");
        Ok(arg_flags)
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service: &ServiceInfo,
    ) -> Result<()> {
        let doc = format!(
            "{}Receiver receives messages defined in the {} service trait",
            service.id, service.id
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service.id, service.traits);
        w.write(b"#[doc(hidden)]\n#[async_trait]\npub trait ");
        self.write_ident_with_suffix(w, service.id, "Receiver")?;
        w.write(b" : MessageDispatch + ");
        self.write_ident(w, service.id);
        let proto = crate::model::wasmbus_proto(service.traits)?;
        let has_cbor = proto.map(|pv| pv.has_cbor()).unwrap_or(false);
        w.write(
            br#"{
            async fn dispatch<'disp__,'ctx__,'msg__>(
                &'disp__ self,
                ctx: &'ctx__ Context,
                message: &Message<'msg__> ) -> Result<Message<'msg__>, RpcError> {
                match message.method {
        "#,
        );

        for method_id in service.service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, method_traits) = get_operation(model, method_id, service.id)?;
            w.write(b"\"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\" => {\n");
            if let Some(op_input) = op.input() {
                let symbol = op_input.shape_name().to_string();
                // let value : InputType = deserialize(...)?;
                if has_cbor {
                    w.write(&format!(
                        r#"
                    let value : {} = {}::common::decode(&message.arg, &decode_{})
                      .map_err(|e| RpcError::Deser(format!("'{}': {{}}", e)))?;
                    "#,
                        self.type_string(Ty::Shape(op_input))?,
                        self.import_core,
                        crate::strings::to_snake_case(&symbol),
                        &symbol,
                    ));
                } else {
                    w.write(&format!(
                        r#"
                        let value: {} = {}::common::deserialize(&message.arg)
                      .map_err(|e| RpcError::Deser(format!("'{}': {{}}", e)))?;
                        "#,
                        self.type_string(Ty::Shape(op_input))?,
                        self.import_core,
                        &symbol,
                    ))
                }
            }
            // let resp = Trait::method(self, ctx, &value).await?;
            if op.output().is_some() {
                w.write(b"let resp = ");
            } else {
                w.write(b"let _resp = ");
            }
            let method_name = self.to_method_name(method_ident, method_traits);
            self.write_ident(w, service.id); // Service::method
            w.write(b"::");
            w.write(&method_name);
            w.write(b"(self, ctx");
            if op.has_input() {
                w.write(b", &value");
            }
            w.write(b").await?;\n");

            if let Some(_op_output) = op.output() {
                // serialize result
                if has_cbor {
                    w.write(&format!(
                        "let mut e = {}::cbor::vec_encoder(true);\n",
                        &self.import_core
                    ));
                    let s = self.encode_shape_id(
                        _op_output,
                        crate::encode_rust::ValExpr::Plain("resp"),
                        true,
                    )?;
                    w.write(&s);
                    w.write(b"let buf = e.into_inner();\n");
                } else {
                    w.write(&format!(
                        "let buf = {}::common::serialize(&resp)?;\n",
                        &self.import_core
                    ));
                }
            } else {
                w.write(b"let buf = Vec::new();\n");
            }
            //w.write(br#"console_log(format!("actor result {}b",buf.len())); "#);
            w.write(b"Ok(Message { method: \"");
            w.write(&self.full_dispatch_name(service.id, method_ident));
            w.write(b"\", arg: Cow::Owned(buf) })},\n");
        }
        w.write(b"_ => Err(RpcError::MethodNotHandled(format!(\"");
        self.write_ident(w, service.id);
        w.write(b"::{}\", message.method))),\n");
        w.write(b"}\n}\n}\n\n"); // end match, end fn dispatch, end trait

        Ok(())
    }

    /// writes the service sender struct and constructor
    // pub struct FooSender{ ... }
    fn write_service_sender(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service: &ServiceInfo,
    ) -> Result<()> {
        let doc = format!(
            "{}Sender sends messages to a {} service",
            service.id, service.id
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service.id, service.traits);
        let proto = crate::model::wasmbus_proto(service.traits)?;
        let has_cbor = proto.map(|pv| pv.has_cbor()).unwrap_or(false);
        w.write(&format!(
            r#"/// client for sending {} messages
              #[derive(Debug)]
              pub struct {}Sender<T:Transport>  {{ transport: T }}

              impl<T:Transport> {}Sender<T> {{
                  /// Constructs a {}Sender with the specified transport
                  pub fn via(transport: T) -> Self {{
                      Self{{ transport }}
                  }}
                  
                  pub fn set_timeout(&self, interval: std::time::Duration) {{
                    self.transport.set_timeout(interval);
                  }}
              }}
            "#,
            service.id, service.id, service.id, service.id,
        ));
        #[cfg(feature = "wasmbus")]
        w.write(&self.actor_receive_sender_constructors(service.id, service.traits)?);
        #[cfg(feature = "wasmbus")]
        w.write(&self.provider_receive_sender_constructors(service.id, service.traits)?);

        // implement Trait for TraitSender
        w.write(b"#[async_trait]\nimpl<T:Transport + std::marker::Sync + std::marker::Send> ");
        self.write_ident(w, service.id);
        w.write(b" for ");
        self.write_ident_with_suffix(w, service.id, "Sender")?;
        w.write(b"<T> {\n");

        for method_id in service.service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();

            let (op, method_traits) = get_operation(model, method_id, service.id)?;
            w.write(b"#[allow(unused)]\n");
            let arg_flags = self.write_method_signature(w, method_ident, method_traits, op)?;
            let _arg_is_string = matches!(arg_flags, MethodArgFlags::ToString);
            w.write(b" {\n");
            if let Some(_op_input) = op.input() {
                if has_cbor {
                    if _arg_is_string {
                        w.write(b"let arg = arg.to_string();\n");
                    }
                    w.write(&format!(
                        "let mut e = {}::cbor::vec_encoder(true);\n",
                        &self.import_core
                    ));
                    let s = self.encode_shape_id(
                        _op_input,
                        if _arg_is_string {
                            crate::encode_rust::ValExpr::Ref("arg.as_ref()")
                        } else {
                            crate::encode_rust::ValExpr::Ref("arg")
                        },
                        true,
                    )?;
                    w.write(&s);
                    w.write(b"let buf = e.into_inner(); \n");
                    //let tn = crate::strings::to_snake_case(&self.type_string(Ty::Shape(op.input().as_ref().unwrap()))?);
                    //w.write(&format!("encode_{}(&mut e, arg)?;", tn));
                } else if matches!(arg_flags, MethodArgFlags::ToString) {
                    w.write(&format!(
                        "let buf = {}::common::serialize(&arg.to_string())?;\n",
                        &self.import_core
                    ));
                } else {
                    w.write(&format!(
                        "let buf = {}::common::serialize(arg)?;\n",
                        &self.import_core
                    ));
                }
            } else {
                w.write(b"let buf = *b\"\";\n");
            }
            w.write(b"let resp = self.transport.send(ctx, Message{ method: ");
            // note: legacy is just the latter part
            w.write(b"\"");
            w.write(&self.full_dispatch_name(service.id, method_ident));
            //w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\", arg: Cow::Borrowed(&buf)}, None).await?;\n");
            if let Some(op_output) = op.output() {
                let symbol = op_output.shape_name().to_string();
                if has_cbor {
                    w.write(&format!(
                        r#"
                    let value : {} = {}::common::decode(&resp, &decode_{})
                        .map_err(|e| RpcError::Deser(format!("'{{}}': {}", e)))?;
                    Ok(value)
                    "#,
                        self.type_string(Ty::Shape(op_output))?,
                        self.import_core,
                        crate::strings::to_snake_case(&symbol),
                        &symbol,
                    ));
                } else {
                    w.write(&format!(
                        r#"
                    let value : {} = {}::common::deserialize(&resp)
                        .map_err(|e| RpcError::Deser(format!("'{{}}': {}", e)))?;
                    Ok(value)
                    "#,
                        self.type_string(Ty::Shape(op_output))?,
                        self.import_core,
                        &symbol,
                    ));
                }
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
                    pub fn new_with_link(link_name: &str) -> {}::error::RpcResult<Self> {{
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
pub(crate) fn is_optional_type(field: &MemberShape) -> bool {
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

// check that the codegen package has a parseable version
#[test]
fn package_semver() {
    let package_version = env!("CARGO_PKG_VERSION");
    let version = semver::Version::parse(package_version);
    assert!(
        version.is_ok(),
        "package version {} has unexpected format",
        package_version
    );
}
