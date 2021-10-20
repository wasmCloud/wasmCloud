//! Go language code-generator
//!
#[cfg(feature = "wasmbus")]
use crate::wasmbus_model::Wasmbus;
use crate::{
    config::LanguageConfig,
    error::{print_warning, Error, Result},
    gen::{CodeGen, SourceFormatter},
    model::{
        get_operation, get_trait, is_opt_namespace, serialization_trait, value_to_json,
        wasmcloud_model_namespace, CommentKind, PackageName,
    },
    render::Renderer,
    wasmbus_model::Serialization,
    writer::Writer,
    BytesMut, ParamMap,
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
        prelude_namespace_id, prelude_shape_named, SHAPE_BIGDECIMAL, SHAPE_BIGINTEGER, SHAPE_BLOB,
        SHAPE_BOOLEAN, SHAPE_BYTE, SHAPE_DOCUMENT, SHAPE_DOUBLE, SHAPE_FLOAT, SHAPE_INTEGER,
        SHAPE_LONG, SHAPE_PRIMITIVEBOOLEAN, SHAPE_PRIMITIVEBYTE, SHAPE_PRIMITIVEDOUBLE,
        SHAPE_PRIMITIVEFLOAT, SHAPE_PRIMITIVEINTEGER, SHAPE_PRIMITIVELONG, SHAPE_PRIMITIVESHORT,
        SHAPE_SHORT, SHAPE_STRING, SHAPE_TIMESTAMP, TRAIT_DEPRECATED, TRAIT_DOCUMENTATION,
        TRAIT_TRAIT, TRAIT_UNSTABLE,
    },
};
use std::{collections::HashMap, path::Path, str::FromStr, string::ToString};

const WASMBUS_RPC_CRATE: &str = "wasmbus_rpc";

/// declarations for sorting. First sort key is the type (simple, then map, then struct).
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
pub struct GoCodeGen<'model> {
    /// if set, limits declaration output to this namespace only
    namespace: Option<NamespaceID>,
    packages: HashMap<String, PackageName>,
    import_core: String,
    #[allow(dead_code)]
    model: Option<&'model Model>,
}

impl<'model> GoCodeGen<'model> {
    pub fn new(model: Option<&'model Model>) -> Self {
        Self {
            model,
            namespace: None,
            packages: HashMap::default(),
            import_core: String::default(),
        }
    }
}

impl<'model> CodeGen for GoCodeGen<'model> {
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
        Ok(Box::new(crate::format::GoSourceFormatter::default()))
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
        //    Some(JsonValue::String(c)) if c == WASMBUS_RPC_CRATE => "gopkg".to_string(),
        //    _ => WASMBUS_RPC_CRATE.to_string(),
        //};
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
               // WARNING WARNING
               // GO language code generation is still in development and probably broken
               //
            "#);
        match &self.namespace {
            Some(n) if n == wasmcloud_model_namespace() => {
                // the base model has minimal dependencies
                w.write(
                    r#"
                package lib
                import "github.com/vmihailenco/msgpack/v5"
                import "wasmbus_rpc"
             "#,
                );
            }
            _ => {
                // all others use standard frontmatter

                // special case for imports:
                // if the crate we are generating is "wasmbus_rpc" then we have to import it with "crate::".
                w.write(
                    r#"
                package lib
                import "github.com/vmihailenco/msgpack/v5"
                import "wasmbus_rpc"
                "#,
                );
            }
        }
        w.write(&format!(
            "\nconst SMITHY_VERSION = \"{}\";\n\n",
            model.smithy_version().to_string()
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

        w.write(b"var (\n");
        for (id, traits, shape) in shapes.into_iter() {
            match shape {
                ShapeKind::Simple(simple) => {
                    self.declare_simple_shape(w, id.shape_name(), traits, simple)?;
                }
                ShapeKind::Map(map) => {
                    self.declare_map_shape(w, id.shape_name(), traits, map)?;
                }
                ShapeKind::List(list) => {
                    self.declare_list_shape(w, id.shape_name(), traits, list)?;
                }
                ShapeKind::Set(_set) => {
                    print_warning(&format!(
                        "'Set' shape type is not implemented ({})",
                        id.shape_name()
                    ));
                    /*
                    self.declare_list_or_set_shape(
                        w,
                        id.shape_name(),
                        traits,
                        set,
                    )?;
                     */
                }
                _ => {}
            }
        }
        w.write(b")\n\n");

        let mut shapes = model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
            .filter(|(_, _, b)| matches!(b, ShapeKind::Structure(_)))
            .collect::<ShapeList>();
        // sort shapes (they are all in the same namespace if ns.is_some(), which is usually true)
        shapes.sort_by_key(|v| v.0);
        for (id, traits, shape) in shapes.into_iter() {
            if let ShapeKind::Structure(strukt) = shape {
                //if !traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap()) {
                self.declare_structure_shape(w, id.shape_name(), traits, strukt)?;
                //}
            }
        }

        Ok(())
    }

    #[allow(unused_variables)]
    fn write_services(&mut self, w: &mut Writer, model: &Model, params: &ParamMap) -> Result<()> {
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
        w.write(match kind {
            CommentKind::Documentation => "/// ",
            CommentKind::Inner => "// ",
        });
        w.write(line);
        w.write(b"\n");
    }

    /// returns go source file extension "go"
    fn get_file_extension(&self) -> &'static str {
        "go"
    }
}

/// returns true if the file path ends in ".go"
#[allow(clippy::ptr_arg)]
pub(crate) fn is_go_source(path: &Path) -> bool {
    match path.extension() {
        Some(s) => s.to_string_lossy().as_ref() == "go",
        _ => false,
    }
}

impl<'model> GoCodeGen<'model> {
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

        // deprecated
        if let Some(Some(Value::Object(_map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            self.write_documentation(w, id, "Deprecated");
        }

        // unstable
        if traits
            .get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap())
            .is_some()
        {
            self.write_documentation(w, id, "Unstable");
        }
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    fn write_type(&mut self, w: &mut Writer, ty: Ty<'_>) -> Result<()> {
        match ty {
            Ty::Opt(id) => {
                self.write_type(w, Ty::Shape(id))?;
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
                        SHAPE_BLOB => "[]byte",
                        SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool",
                        SHAPE_STRING => "string",
                        SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "int8",
                        SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "int16",
                        SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "int32",
                        SHAPE_LONG | SHAPE_PRIMITIVELONG => "int64",
                        SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "float32",
                        SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "float64",
                        // if declared as members (of a struct, list, or map), we don't have trait data here to write
                        // as anything other than a blob. Instead, a type should be created for the Document that can have traits,
                        // and that type used for the member. This should probably be a lint rule.
                        SHAPE_DOCUMENT => "[]byte",
                        SHAPE_TIMESTAMP => {
                            // FIXME: NOT IMPLEMENTED
                            return Err(Error::UnsupportedType(
                                "Timestamp is unsupported for go".to_string(),
                            ));
                        }
                        SHAPE_BIGINTEGER => {
                            // FIXME: NOT IMPLEMENTED
                            return Err(Error::UnsupportedBigInteger);
                        }
                        SHAPE_BIGDECIMAL => {
                            // FIXME: NOT IMPLEMENTED
                            return Err(Error::UnsupportedBigDecimal);
                        }
                        _ => return Err(Error::UnsupportedType(name)),
                    };
                    w.write(ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    match name.as_bytes() {
                        b"U64" | b"U32" | b"U16" | b"U8" => {
                            w.write(b"uint");
                            w.write(&name.as_bytes()[1..])
                        }
                        b"I64" | b"I32" | b"I16" | b"I8" => {
                            w.write(b"int");
                            w.write(&name.as_bytes()[1..])
                        }
                        _ => {
                            if self.namespace.is_none()
                                || self.namespace.as_ref().unwrap() != wasmcloud_model_namespace()
                            {
                                w.write(&self.import_core);
                                w.write(b".model.");
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
                            w.write(b".");
                            w.write(&self.to_type_name(&id.shape_name().to_string()));
                        }
                        None => {
                            return Err(Error::Model(format!("undefined gopkg for namespace {} for symbol {}. Make sure codegen.toml includes all dependent namespaces",
                                    &id.namespace(), &id)));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    #[allow(dead_code)]
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
        w.write(b" ");
        self.write_ident(w, id);
        w.write(b" ");
        let ty = match simple {
            Simple::Blob => "[]byte",
            Simple::Boolean => "bool",
            Simple::String => "string",
            Simple::Byte => "int8",
            Simple::Short => "int16",
            Simple::Integer => "int32",
            Simple::Long => "int64",
            Simple::Float => "float32",
            Simple::Double => "float64",

            // note: in the future, codegen traits may modify this
            Simple::Document => {
                print_warning(&format!("'Document' type is not implemented ({})", id));
                "[]byte"
            }
            Simple::Timestamp => {
                return Err(Error::UnsupportedType(
                    "Timestamp is unsupported for go".to_string(),
                ));
            }
            Simple::BigInteger => {
                return Err(Error::UnsupportedBigInteger);
            }
            Simple::BigDecimal => {
                return Err(Error::UnsupportedBigDecimal);
            }
        };
        w.write(ty);
        w.write(b";\n");
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
        w.write(b" ");
        self.write_ident(w, id);
        w.write(b" map");
        w.write(b"[");
        self.write_type(w, Ty::Shape(shape.key().target()))?;
        w.write(b"]");
        self.write_type(w, Ty::Shape(shape.value().target()))?;
        w.write(b";\n");
        Ok(())
    }

    fn declare_list_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &ListOrSet,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        w.write(b" ");
        self.write_ident(w, id);
        w.write(b" []");
        self.write_type(w, Ty::Shape(shape.member().target()))?;
        w.write(b";\n");
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
        w.write(b"type ");
        self.write_ident(w, id);
        w.write(b" struct {\n");
        // sort fields for deterministic output
        let mut fields = strukt
            .members()
            .map(|m| m.to_owned())
            .collect::<Vec<MemberShape>>();
        fields.sort_by_key(|f| f.id().to_owned());
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
            let go_field_name = self.to_field_name(member.id())?;

            let is_optional =
                is_optional_type(member) || (is_trait_struct && !member.is_required());
            let is_opt_label = if is_optional { ",omitempty" } else { "" };
            w.write(&go_field_name);
            w.write(b" ");
            self.write_field_type(w, member)?;

            if ser_name != go_field_name {
                w.write(&format!(
                    " `msgpack:\"{}{}\",json:\"{}{}\"`",
                    ser_name, is_opt_label, ser_name, is_opt_label,
                ));
            }
            w.write(b";\n"); // use semicolon at end of line
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// write field type, wrapping with Option if field is not required
    fn write_field_type(&mut self, w: &mut Writer, field: &MemberShape) -> Result<()> {
        self.write_type(
            w,
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
        w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        self.apply_documentation_traits(w, service_id, service_traits);

        #[cfg(feature = "wasmbus")]
        self.add_wasmbus_comments(w, service_id, service_traits)?;

        w.write(b"type ");
        self.write_ident(w, service_id);
        w.write(b" interface {\n");
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
            self.write_method_signature(w, method_id, op_traits, op)?;
            w.write(b";\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    #[cfg(feature = "wasmbus")]
    fn add_wasmbus_comments(
        &mut self,
        w: &mut Writer,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<()> {
        // currently the only thing we do with Wasmbus in codegen is add comments
        let wasmbus: Option<Wasmbus> = get_trait(service_traits, crate::model::wasmbus_trait())?;
        if let Some(wasmbus) = wasmbus {
            if let Some(contract) = wasmbus.contract_id {
                let text = format!("wasmbus.contractId: {}", &contract);
                self.write_documentation(w, service_id, &text);
            }
            if wasmbus.provider_receive {
                let text = "wasmbus.providerReceive";
                self.write_documentation(w, service_id, text);
            }
            if wasmbus.actor_receive {
                let text = "wasmbus.actorReceive";
                self.write_documentation(w, service_id, text);
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
    ) -> Result<()> {
        let method_name = self.to_method_name(method_id)?;
        self.apply_documentation_traits(w, method_id, method_traits);
        w.write(b" ");
        w.write(&method_name);
        w.write(b"(ctx  &context.Context");
        if let Some(input_type) = op.input() {
            w.write(b", arg  "); // pass arg by reference
            self.write_type(w, Ty::Ref(input_type))?;
        }
        w.write(b") ");
        if let Some(output_type) = op.output() {
            self.write_type(w, Ty::Shape(output_type))?;
        }
        Ok(())
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        w: &mut Writer,
        _model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        _service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Receiver receives messages defined in the {} service trait",
            service_id, service_id
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service_id, service_traits);
        w.write(&format!("// {}Receiver not implemented\n", service_id));

        /*
        w.write(b"#[async_trait]\npub trait ");
        self.write_ident_with_suffix(w, service_id, "Receiver")?;
        w.write(b" : MessageDispatch + ");
        self.write_ident(w, service_id);
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
                self.write_type(w, Ty::Shape(op.input().as_ref().unwrap()))?;
                w.write(b" = deserialize(message.arg.as_ref())?;\n");
            }
            // let resp = Trait::method(self, ctx, &value).await?;
            w.write(b"let resp = ");
            self.write_ident(w, service_id); // Service::method
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
            w.write(b"Ok(Message { method: ");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            w.write(b", arg: buf })},\n");
        }
        w.write(b"_ => Err(RpcError::MethodNotHandled(format!(\"");
        self.write_ident(w, service_id);
        w.write(b"::{}\", message.method))),\n");
        w.write(b"}\n}\n}\n\n"); // end match, end fn dispatch, end trait
         */

        Ok(())
    }

    /// writes the service sender struct and constructor
    // pub struct FooSender{ ... }
    fn write_service_sender(
        &mut self,
        w: &mut Writer,
        _model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        _service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Sender sends messages to a {} service",
            service_id, service_id
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service_id, service_traits);

        w.write(&format!("// {}Sender not implemented\n", service_id));
        /*
        w.write(b"#[derive(Debug)]\npub struct ");
        self.write_ident_with_suffix(w, service_id, "Sender")?;
        w.write(b"<T> { transport: T, config: client::SendConfig }\n\n");

        // implement constructor for TraitClient
        w.write(b"impl<T:Transport>  ");
        self.write_ident_with_suffix(w, service_id, "Sender")?;
        w.write(b"<T> { \n");
        w.write(b" pub fn new(config: client::SendConfig, transport: T) -> Self { ");
        self.write_ident_with_suffix(w, service_id, "Sender")?;
        w.write(b"{ transport, config }\n}\n}\n\n");

        // implement Trait for TraitSender
        w.write(b"#[async_trait]\nimpl<T:Transport + std::marker::Sync + std::marker::Send> ");
        self.write_ident(w, service_id);
        w.write(b" for ");
        self.write_ident_with_suffix(w, service_id, "Sender")?;
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
            self.write_method_signature(w, method_ident, method_traits, op)?;
            w.write(b" {\n");

            if op.has_input() {
                w.write(b"let arg = serialize(arg)?;\n");
            } else {
                w.write(b"let arg = *b\"\";\n");
            }
            w.write(b"let resp = self.transport.send(ctx, &self.config, Message{ method: ");
            // TODO: switch to quoted full method (increment api version # if not legacy)
            //w.write(self.full_dispatch_name(trait_base.id(), method_id));
            // note: legacy is just the latter part
            w.write(b"\"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\", arg: Cow::Borrowed(&arg)}).await?;\n");
            if op.has_output() {
                w.write(b"let value = deserialize(resp.arg.as_ref())?; Ok(value)");
            } else {
                w.write(b"Ok(())");
            }
            w.write(b" }\n");
        }
        w.write(b"}\n\n");
         */
        Ok(())
    }
    /// Convert field name to its target-language-idiomatic case style
    fn to_field_name(&self, member_id: &Identifier) -> std::result::Result<String, Error> {
        Ok(crate::strings::to_pascal_case(&member_id.to_string()))
    }
    /// Convert method name to its target-language-idiomatic case style
    fn to_method_name(&self, method: &Identifier) -> std::result::Result<String, Error> {
        Ok(crate::strings::to_pascal_case(&method.to_string()))
    }
} // impl CodeGenGo

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
