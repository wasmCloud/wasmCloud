//! Python language code-generator
//!

use std::{collections::HashMap, path::Path, str::FromStr, string::ToString};

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
        prelude_namespace_id, prelude_shape_named, SHAPE_BIGDECIMAL, SHAPE_BIGINTEGER, SHAPE_BLOB,
        SHAPE_BOOLEAN, SHAPE_BYTE, SHAPE_DOCUMENT, SHAPE_DOUBLE, SHAPE_FLOAT, SHAPE_INTEGER,
        SHAPE_LONG, SHAPE_PRIMITIVEBOOLEAN, SHAPE_PRIMITIVEBYTE, SHAPE_PRIMITIVEDOUBLE,
        SHAPE_PRIMITIVEFLOAT, SHAPE_PRIMITIVEINTEGER, SHAPE_PRIMITIVELONG, SHAPE_PRIMITIVESHORT,
        SHAPE_SHORT, SHAPE_STRING, SHAPE_TIMESTAMP, TRAIT_DEPRECATED, TRAIT_DOCUMENTATION,
        TRAIT_TRAIT, TRAIT_UNSTABLE,
    },
};

#[cfg(feature = "wasmbus")]
use crate::wasmbus_model::Wasmbus;
use crate::{
    codegen_rust::is_optional_type,
    config::{LanguageConfig, OutputLanguage},
    error::{print_warning, Error, Result},
    format::SourceFormatter,
    gen::{spaces, CodeGen},
    model::{
        get_operation, get_sorted_fields, get_trait, is_opt_namespace, value_to_json,
        wasmcloud_model_namespace, CommentKind, PackageName, Ty,
    },
    render::Renderer,
    writer::Writer,
    BytesMut, ParamMap,
};

const WASMBUS_RPC_CRATE: &str = "wasmbus_rpc";

const DEFAULT_MAP_TYPE: &str = "dict"; // python 3.9+
const DEFAULT_LIST_TYPE: &str = "list"; // python 3.9+
const DEFAULT_SET_TYPE: &str = "set"; // python 3.9+
const DEFAULT_DOCUMENT_TYPE: &str = "Document";

/// declarations for sorting. First sort key is the type (simple, then map, then struct).
/// In rust, sorting by BytesMut as the second key will result in sort by item name.
#[derive(Eq, Ord, PartialOrd, PartialEq)]
struct Declaration(u8, BytesMut);

type ShapeList<'model> = Vec<(&'model ShapeID, &'model AppliedTraits, &'model ShapeKind)>;

#[derive(Default)]
pub struct PythonCodeGen<'model> {
    /// if set, limits declaration output to this namespace only
    pub(crate) namespace: Option<NamespaceID>,
    pub(crate) packages: HashMap<String, PackageName>,
    pub(crate) import_core: String,
    #[allow(dead_code)]
    pub(crate) model: Option<&'model Model>, // unused in python codegen
    imported_packages: std::collections::BTreeSet<String>,
    pub(crate) indent_level: u8,
}

impl<'model> PythonCodeGen<'model> {
    pub fn new(model: Option<&'model Model>) -> Self {
        Self {
            model,
            namespace: None,
            packages: HashMap::default(),
            import_core: String::default(),
            indent_level: 0,
            imported_packages: std::collections::BTreeSet::default(),
        }
    }
}

impl<'model> CodeGen for PythonCodeGen<'model> {
    fn output_language(&self) -> OutputLanguage {
        OutputLanguage::Python
    }

    /// Initialize code generator and renderer for language output.j
    /// This hook is called before any code is generated and can be used to initialize code generator
    /// and/or perform additional processing before output files are created.
    fn init(
        &mut self,
        model: Option<&Model>,
        _lc: &LanguageConfig,
        _output_dir: Option<&Path>,
        _renderer: &mut Renderer,
    ) -> std::result::Result<(), Error> {
        self.namespace = None;
        self.import_core = WASMBUS_RPC_CRATE.to_string();

        if let Some(model) = model {
            if let Some(packages) = model.metadata_value("package") {
                let packages: Vec<PackageName> = serde_json::from_value(value_to_json(packages))
                    .map_err(|e| {
                        Error::Model(format!(
                            "invalid metadata format for package, expecting format \
                             '[{{namespace:\"org.example\",crate:\"path::module\"}}]':  {e}"
                        ))
                    })?;
                for p in packages.iter() {
                    self.packages.insert(p.namespace.to_string(), p.clone());
                }
            }
        }
        Ok(())
    }

    fn source_formatter(&self, _: Vec<String>) -> Result<Box<dyn SourceFormatter>> {
        Ok(Box::<PythonSourceFormatter>::default())
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
        //self.import_core = match params.get("crate") {
        //    Some(JsonValue::String(c)) if c == WASMBUS_RPC_CRATE => "crate".to_string(),
        //    _ => WASMBUS_RPC_CRATE.to_string(),
        //};
        Ok(())
    }

    /// Complete generation and return the output bytes
    fn finalize(&mut self, w: &mut Writer) -> Result<bytes::Bytes> {
        let mut hdr = self.write_deferred_source_file_header()?;
        let rest = w.take();
        hdr.extend(&rest);
        Ok(hdr.freeze())
    }

    fn write_source_file_header(
        &mut self,
        w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        // we will write the top of the header later so we can add necessary imports
        w.write(&format!(
            "SMITHY_VERSION = \"{}\"\n\n",
            model.smithy_version()
        ));
        Ok(())
    }

    fn declare_types(&mut self, w: &mut Writer, model: &Model, _params: &ParamMap) -> Result<()> {
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
                ShapeKind::Operation(_)
                | ShapeKind::Resource(_)
                | ShapeKind::Service(_)
                | ShapeKind::Union(_)
                | ShapeKind::Unresolved => {}
            }
            if !traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap()) {
                self.declare_shape_encoder(w, id, shape)?;
                self.declare_shape_decoder(w, id, shape)?;
            }
        }
        Ok(())
    }

    fn write_services(&mut self, w: &mut Writer, model: &Model, _params: &ParamMap) -> Result<()> {
        let ns = self.namespace.clone();
        for (id, traits, shape) in model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
        {
            if let ShapeKind::Service(service) = shape {
                self.write_service_interface(w, model, id.shape_name(), traits, service)?;
                self.write_service_receiver(w, model, id.shape_name(), traits, service)?;
                self.write_service_sender(w, model, id.shape_name(), traits, service)?;
            }
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, w: &mut Writer, kind: CommentKind, line: &str) {
        w.write(spaces(self.indent_level));
        w.write(match kind {
            CommentKind::Documentation => "# ",
            CommentKind::Inner => "# ",
            CommentKind::InQuote => "",
        });
        w.write(line);
        w.write(b"\n");
    }

    /// returns python source file extension "py"
    fn get_file_extension(&self) -> &'static str {
        "py"
    }

    fn write_documentation(&mut self, w: &mut Writer, _id: &Identifier, text: &str) {
        for line in text.split('\n') {
            // remove whitespace from end of line
            let line = line.trim_end_matches(|c| c == '\r' || c == ' ' || c == '\t');
            self.write_comment(w, CommentKind::InQuote, line);
        }
    }

    /// Convert field name to its target-language-idiomatic case style
    fn to_field_name_case(&self, name: &str) -> String {
        crate::strings::to_camel_case(name)
    }

    /// Convert method name to its target-language-idiomatic case style
    fn to_method_name_case(&self, name: &str) -> String {
        crate::strings::to_camel_case(name)
    }

    /// Convert type name to its target-language-idiomatic case style
    fn to_type_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }
}

impl<'model> PythonCodeGen<'model> {
    fn write_deferred_source_file_header(&mut self) -> Result<bytes::BytesMut> {
        let mut w = Writer::default();
        w.write(b"# This file is generated automatically using wasmcloud/weld-codegen and smithy model definitions\n");
        w.write(b"#\n");
        w.write(b"import cbor2\n");
        w.write(b"from io import BytesIO\n");
        w.write(b"from typing import Optional\n");
        w.write(b"from wasmcloud import Message,Context,decode_map_or_array\n");
        for import in self.imported_packages.iter() {
            w.write(&format!("import {import}\n"));
        }
        Ok(w.take())
    }

    fn add_package_import(&mut self, p: &str) {
        let pkg_name = if let Some((before, _)) = p.split_once('.') { before } else { p };
        self.imported_packages.insert(pkg_name.to_string());
    }

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
            w.write(spaces(self.indent_level));
            w.write(b"\"\"\"\n");
            self.write_documentation(w, id, text);
            w.write(spaces(self.indent_level));
            w.write(b"\"\"\"\n");
        }

        // deprecated
        if let Some(Some(Value::Object(map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            w.write(b"#[deprecated(");
            if let Some(Value::String(since)) = map.get("since") {
                w.write(&format!("since=\"{since}\"\n"));
            }
            if let Some(Value::String(message)) = map.get("message") {
                w.write(&format!("note=\"{message}\"\n"));
            }
            w.write(b")\n");
        }

        // unstable
        if traits.get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap()).is_some() {
            self.write_comment(w, CommentKind::Documentation, "@unstable");
        }
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    pub(crate) fn type_string(&mut self, ty: Ty<'_>) -> Result<String> {
        let mut s = String::new();
        match ty {
            Ty::Opt(id) => {
                s.push_str("Optional[");
                s.push_str(&self.type_string(Ty::Shape(id))?);
                s.push(']');
            }
            Ty::Ref(id) | Ty::Shape(id) => {
                let name = id.shape_name().to_string();
                if id.namespace() == prelude_namespace_id() {
                    let ty = match name.as_ref() {
                        // Document are  Blob
                        SHAPE_BLOB => "bytes",
                        SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool",
                        SHAPE_STRING => "str",
                        SHAPE_BYTE
                        | SHAPE_PRIMITIVEBYTE
                        | SHAPE_SHORT
                        | SHAPE_PRIMITIVESHORT
                        | SHAPE_INTEGER
                        | SHAPE_PRIMITIVEINTEGER
                        | SHAPE_LONG
                        | SHAPE_PRIMITIVELONG => "int",
                        SHAPE_FLOAT
                        | SHAPE_PRIMITIVEFLOAT
                        | SHAPE_DOUBLE
                        | SHAPE_PRIMITIVEDOUBLE => "float",
                        // if declared as members (of a struct, list, or map), we don't have trait data here to write
                        // as anything other than a blob. Instead, a type should be created for the Document that can have traits,
                        // and that type used for the member. This should probably be a lint rule.
                        SHAPE_DOCUMENT => DEFAULT_DOCUMENT_TYPE,
                        SHAPE_TIMESTAMP => todo!(),
                        SHAPE_BIGINTEGER => "int",
                        SHAPE_BIGDECIMAL => todo!(),
                        _ => return Err(Error::UnsupportedType(name)),
                    };
                    s.push_str(ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    match name.as_str() {
                        "U64" | "U32" | "U16" | "U8" | "I64" | "I32" | "I16" | "I8" => {
                            s.push_str("int");
                        }
                        _ => {
                            if self.namespace.is_none()
                                || self.namespace.as_ref().unwrap() != id.namespace()
                            {
                                s.push_str("wasmbus_model.");
                                self.add_package_import("wasmbus_model");
                            }
                            s.push_str(self.to_type_name_case(&name).trim_matches('\''));
                        }
                    }
                } else if self.namespace.is_some()
                    && id.namespace() == self.namespace.as_ref().unwrap()
                {
                    // we are in the same namespace so we don't need to specify namespace
                    // but enclose in '' in case we need a forward reference
                    s.push('\'');
                    s.push_str(&self.to_type_name_case(&id.shape_name().to_string()));
                    s.push('\'');
                } else {
                    let import_pkg = match self.packages.get(&id.namespace().to_string()) {
                        Some(PackageName { py_module: Some(py_module), .. }) => {
                            // the crate name should be valid rust syntax. If not, they'll get an error with rustc
                            s.push_str(py_module);
                            s.push('.');
                            s.push_str(&self.to_type_name_case(&id.shape_name().to_string()));
                            py_module.to_string()
                        }
                        _ => {
                            return Err(Error::Model(format!(
                                "undefined py_module for namespace {} for symbol {}. Make sure \
                                 codegen.toml includes all dependent namespaces, and that the \
                                 dependent .smithy file contains package metadata with py_module: \
                                 value",
                                &id.namespace(),
                                &id
                            )));
                        }
                    };
                    self.add_package_import(&import_pkg);
                }
            }
            Ty::Ptr(_) => {
                unreachable!()
            }
        }
        Ok(s)
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    fn write_type(&mut self, w: &mut Writer, ty: Ty<'_>) -> Result<()> {
        w.write(&self.type_string(ty)?);
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
        let ty = match simple {
            Simple::Blob => "bytes",
            Simple::Boolean => "bool",
            Simple::String => "str",
            Simple::Byte => "int",
            Simple::Short => "int",
            Simple::Integer => "int",
            Simple::Long => "int",
            Simple::Float => "float",
            Simple::Double => "float",

            // note: in the future, codegen traits may modify this
            Simple::Document => DEFAULT_DOCUMENT_TYPE,
            Simple::Timestamp => todo!(), // "Timestamp",
            Simple::BigInteger => "int",
            Simple::BigDecimal => todo!(), // "Decimal",
        };
        self.declare_subtype(w, id, traits, ty)?;
        Ok(())
    }

    fn declare_map_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &MapShape,
    ) -> Result<()> {
        let map_type = format!(
            "{}['{}', '{}']",
            DEFAULT_MAP_TYPE,
            &self.type_string(Ty::Shape(shape.key().target()))?.trim_matches('\''),
            &self.type_string(Ty::Shape(shape.value().target()))?.trim_matches('\''),
        );
        self.declare_subtype(w, id, traits, &map_type)?;
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
        let list_type = format!(
            "{}['{}']",
            typ,
            self.type_string(Ty::Shape(shape.member().target()))?.trim_matches('\'')
        );
        self.declare_subtype(w, id, traits, &list_type)?;
        Ok(())
    }

    fn declare_subtype(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        base_type: &str,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        w.write(spaces(self.indent_level));
        w.write(b"class ");
        self.write_ident(w, id);
        w.write(b"(");
        w.write(base_type);
        w.write(b"):\n");
        w.write(spaces(self.indent_level + 1));
        w.write(b"pass\n\n");
        Ok(())
    }

    fn declare_structure_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);

        w.write(spaces(self.indent_level));
        w.write(b"class ");
        self.write_ident(w, id);
        w.write(b":\n");
        let (fields, _is_numbered) = get_sorted_fields(id, strukt)?;
        let init_fields = fields
            .iter()
            .map(|f| self.to_field_name(f.id(), f.traits()))
            .collect::<Result<Vec<String>>>()?
            .join(",");
        self.indent_level += 1;
        //w.write(spaces(self.indent_level));
        //w.write(&format!("__slots__ = ({})\n\n", &init_fields));
        for field in fields.iter() {
            w.write(spaces(self.indent_level));
            w.write(&self.to_field_name(field.id(), field.traits())?);
            w.write(b": ");
            w.write(&self.field_type_string(field)?);
            w.write(b"\n");
        }

        // constructor
        w.write(spaces(self.indent_level));
        if fields.is_empty() {
            w.write("def __init__(self) -> None:\n");
            w.write(spaces(self.indent_level + 1));
            w.write("pass\n\n");
        } else {
            w.write(&format!("def __init__(self,{}) -> None:\n", &init_fields));

            self.indent_level += 1;
            for field in fields.iter() {
                let field_name = self.to_field_name(field.id(), field.traits())?;
                w.write(spaces(self.indent_level));
                w.write(&format!(
                    "self.{} : {} = {}\n",
                    &field_name,
                    &self.field_type_string(field)?,
                    &field_name
                ));
            }
            w.write(b"\n");
            self.indent_level -= 1;
        }

        for member in fields.iter() {
            let field_name = self.to_field_name(member.id(), member.traits())?;

            // getter
            self.apply_documentation_traits(w, member.id(), member.traits());
            w.write(spaces(self.indent_level));
            w.write(b"@property\n");
            w.write(spaces(self.indent_level));
            w.write(b"def ");
            w.write(&field_name); // also using field_name as getter method name
            w.write(b"(self) -> ");
            w.write(&self.field_type_string(member)?);
            w.write(b":\n");
            w.write(spaces(self.indent_level + 1));
            w.write(b"return self.");
            w.write(&field_name);
            w.write(b"\n\n");

            // setter
            self.apply_documentation_traits(w, member.id(), member.traits());
            w.write(spaces(self.indent_level));
            w.write(b"@");
            w.write(&field_name); // also using field_name as getter method name
            w.write(b".setter\n");
            w.write(spaces(self.indent_level));
            w.write(b"def ");
            w.write(&field_name); // also using field_name as getter method name
            w.write(b"(self, value: ");
            w.write(&self.field_type_string(member)?);
            w.write(b") -> None:\n");
            w.write(spaces(self.indent_level + 1));
            w.write(b"self.");
            w.write(&field_name);
            w.write(b" = value\n\n");
        }
        self.indent_level -= 1;
        Ok(())
    }

    /// Declares the service with empty stubs for operations
    fn write_service_interface(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        self.apply_documentation_traits(w, service_id, service_traits);

        #[cfg(feature = "wasmbus")]
        self.add_wasmbus_comments(w, service_id, service_traits)?;

        w.write(spaces(self.indent_level));
        w.write(b"class ");
        self.write_ident(w, service_id);
        w.write(b":\n");
        self.indent_level += 1;

        self.write_service_contract_getter(w, service_id, service_traits)?;

        for operation in service.operations() {
            // if operation is not declared in this namespace, don't define it here
            if let Some(ref ns) = self.namespace {
                if operation.namespace() != ns {
                    continue;
                }
            }
            let (op, op_traits) = get_operation(model, operation, service_id)?;
            let method_id = operation.shape_name();
            self.write_method_signature(w, method_id, op_traits, op)?;
            w.write(spaces(self.indent_level + 1));
            w.write(b"raise Exception(\"Not implemented!\")\n\n");
        }
        self.indent_level -= 1;
        Ok(())
    }

    /// add static getter for capability contract id
    fn write_service_contract_getter(
        &mut self,
        w: &mut Writer,
        _service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<()> {
        if let Some(Wasmbus { contract_id: Some(contract_id), .. }) =
            get_trait(service_traits, crate::model::wasmbus_trait())?
        {
            w.write(spaces(self.indent_level));
            w.write(b"# returns the capability contract id for this interface\n");
            w.write(spaces(self.indent_level));
            w.write(b"@staticmethod\n");
            w.write(spaces(self.indent_level));
            w.write(b"def contract_id():\n");
            w.write(spaces(self.indent_level + 1));
            w.write(b"return \"");
            w.write(&contract_id);
            w.write(b"\"\n\n");
        }
        Ok(())
    }

    #[cfg(feature = "wasmbus")]
    fn add_wasmbus_comments(
        &mut self,
        w: &mut Writer,
        _service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<()> {
        // currently the only thing we do with Wasmbus in codegen is add comments
        let wasmbus: Option<Wasmbus> = get_trait(service_traits, crate::model::wasmbus_trait())?;
        if let Some(wasmbus) = wasmbus {
            if let Some(contract) = wasmbus.contract_id {
                let text = format!("wasmbus.contractId: {}", &contract);
                self.write_comment(w, CommentKind::Inner, &text);
            }
            if wasmbus.provider_receive {
                let text = "wasmbus.providerReceive";
                self.write_comment(w, CommentKind::Inner, text);
            }
            if wasmbus.actor_receive {
                let text = "wasmbus.actorReceive";
                self.write_comment(w, CommentKind::Inner, text);
            }
        }
        Ok(())
    }

    /// write service operation method signature, including ':\n' at end of line
    fn write_method_signature(
        &mut self,
        w: &mut Writer,
        method_id: &Identifier,
        method_traits: &AppliedTraits,
        op: &Operation,
    ) -> Result<()> {
        let method_name = self.to_method_name(method_id, method_traits);
        self.apply_documentation_traits(w, method_id, method_traits);

        w.write(spaces(self.indent_level));
        w.write(b"def ");
        w.write(&method_name);
        w.write(b"(self, ctx: Context");
        if let Some(input_type) = op.input() {
            w.write(b", arg: ");
            self.write_type(w, Ty::Ref(input_type))?;
        }
        w.write(b")");
        if let Some(output_type) = op.output() {
            w.write(b" -> ");
            self.write_type(w, Ty::Shape(output_type))?;
        }
        w.write(b":\n");
        Ok(())
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{service_id}Receiver receives messages defined in the {service_id} service trait"
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service_id, service_traits);

        w.write(spaces(self.indent_level));
        w.write(b"class ");
        self.write_ident(w, service_id);
        w.write(b"Receiver:\n");
        self.indent_level += 1;

        // constructor
        w.write(spaces(self.indent_level));
        w.write(b"def __init__(self, impl):\n");
        w.write(spaces(self.indent_level + 1));
        w.write(b"self._impl = impl\n\n");

        w.write(spaces(self.indent_level));
        w.write(b"def dispatch(self, ctx: Context, message: Message):\n");
        self.indent_level += 1;

        let mut ix = 0;
        for method_id in service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, method_traits) = get_operation(model, method_id, service_id)?;
            w.write(spaces(self.indent_level));
            if ix == 0 {
                w.write(b"if ");
            } else {
                w.write(b"elif ");
            }
            w.write(b"message.op == \"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\":\n");
            self.indent_level += 1;

            if let Some(op_input) = op.input() {
                w.write(spaces(self.indent_level));
                w.write(b"d = cbor2.decoder.CBORDecoder(BytesIO(message.arg))\n");
                w.write(spaces(self.indent_level));
                w.write(b"value = ");
                w.write(&self.decode_shape_id(op_input)?);
                w.write("\n");
            }
            // resp = self.impl.method(ctx, value)
            w.write(spaces(self.indent_level));
            w.write(b"resp = self.impl.");
            w.write(&self.to_method_name(method_ident, method_traits));
            w.write(b"(ctx");
            if op.has_input() {
                w.write(b", value");
            }
            w.write(b")\n");

            w.write(spaces(self.indent_level));
            w.write(b"buf = BytesIO()\n");
            if let Some(_op_output) = op.output() {
                // serialize result
                w.write(spaces(self.indent_level));
                w.write(b"e = cbor2.encoder.CBOREncoder(buf)\n");
                let s =
                    self.encode_shape_id(_op_output, crate::encode_py::ValExpr::Plain("resp"))?;
                w.write(spaces(self.indent_level));
                w.write(&s);
            }
            w.write(spaces(self.indent_level));
            w.write(b"return Message(\"");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            w.write(b"\", buf.getvalue())\n");

            self.indent_level -= 1; // if operation
            ix += 1;
        }
        if ix > 0 {
            w.write(spaces(self.indent_level));
            w.write(b"else:\n");
            w.write(spaces(self.indent_level + 1));
            w.write(b"raise Exception(\"MethodNotHandled: {}\".format(message.op))\n");
        }
        self.indent_level -= 1; // end fn dispatch
        self.indent_level -= 1; // end class _Receiver
        w.write(b"\n");

        Ok(())
    }

    /// writes the service sender struct and constructor
    fn write_service_sender(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!("{service_id}Sender sends messages to a {service_id} service");
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service_id, service_traits);
        w.write(spaces(self.indent_level));
        w.write(b"class ");
        self.write_ident(w, service_id);
        w.write(b"Sender:\n");
        self.indent_level += 1;

        // constructor
        w.write(spaces(self.indent_level));
        w.write(b"def __init__(self):\n");
        w.write(spaces(self.indent_level + 1));
        w.write(b"pass\n\n");

        for method_id in service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, method_traits) = get_operation(model, method_id, service_id)?;
            self.write_method_signature(w, method_ident, method_traits, op)?;
            self.indent_level += 1;

            w.write(spaces(self.indent_level));
            w.write(b"buf = BytesIO()\n");
            if let Some(op_input) = op.input() {
                w.write(spaces(self.indent_level));
                w.write(b"e = cbor2.encoder.CBOREncoder(buf)\n");
                w.write(spaces(self.indent_level));
                w.write(b"value = ");
                w.write(&self.encode_shape_id(op_input, crate::encode_py::ValExpr::Plain("arg"))?);
                w.write(b"\n");
            }
            w.write(spaces(self.indent_level));
            w.write(b"resp = Transport.send(ctx, Message(\"");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            w.write(b"\", buf.getvalue()))\n");

            if let Some(op_output) = op.output() {
                w.write(spaces(self.indent_level));
                w.write(b"d = cbor2.decoder.CBORDecoder(BytesIO(resp))\n");
                w.write(spaces(self.indent_level));
                w.write(b"value = ");
                w.write(&self.decode_shape_id(op_output)?);
                w.write(b"\n");
                w.write(spaces(self.indent_level));
                w.write(b"return value\n");
            }
            self.indent_level -= 1; // end send method
        }

        self.indent_level -= 1; // end class _Sender
        w.write(b"\n");
        Ok(())
    }

    /// field type, wrapped with Option if field is not required
    pub(crate) fn field_type_string(&mut self, field: &MemberShape) -> Result<String> {
        self.type_string(if is_optional_type(field) {
            Ty::Opt(field.target())
        } else {
            Ty::Shape(field.target())
        })
    }
} // impl PythonCodeGen

/// Formatter of python code using `black`
pub struct PythonSourceFormatter {
    /// 'black' - must be in execution path
    program: String,
    /// any additional args
    extra: Vec<String>,
}

impl Default for PythonSourceFormatter {
    fn default() -> Self {
        PythonSourceFormatter {
            program: "black".to_string(),
            extra: Vec::new(),
        }
    }
}

impl SourceFormatter for PythonSourceFormatter {
    fn run(&self, source_files: &[&str]) -> Result<()> {
        let mut args = Vec::new();
        args.extend(self.extra.iter().map(|s| s.as_str()));
        args.extend(source_files.iter());
        crate::format::run_command(&self.program, &args)?;
        Ok(())
    }
}
