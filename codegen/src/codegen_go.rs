//! Go language code-generator
//!
use std::{collections::HashMap, fmt, fmt::Write as _, path::Path, str::FromStr, string::ToString};

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

#[cfg(feature = "wasmbus")]
use crate::wasmbus_model::Wasmbus;
use crate::{
    config::{LanguageConfig, OutputLanguage},
    error::{print_warning, Error, Result},
    format::{self, SourceFormatter},
    gen::CodeGen,
    model::{
        get_operation, get_sorted_fields, get_trait, is_opt_namespace, value_to_json,
        wasmcloud_core_namespace, wasmcloud_model_namespace, CommentKind, PackageName, Ty,
    },
    render::Renderer,
    writer::Writer,
    BytesMut, ParamMap,
};
/// declarations for sorting. First sort key is the type (simple, then map, then struct).
#[derive(Eq, Ord, PartialOrd, PartialEq)]
struct Declaration(u8, BytesMut);

type ShapeList<'model> = Vec<(&'model ShapeID, &'model AppliedTraits, &'model ShapeKind)>;

fn codec_crate(has_cbor: bool) -> &'static str {
    if has_cbor {
        "cbor"
    } else {
        "msgpack"
    }
}

// cbor function name modifier
fn codec_pfx(has_cbor: bool) -> &'static str {
    if has_cbor {
        "C"
    } else {
        "M"
    }
}

enum MethodSigType {
    Interface,
    Sender(String),
    //Receiver,
}

#[derive(Clone, Copy)]
pub(crate) enum DecodeRef {
    Plain,
    ByRef,
}
impl fmt::Display for DecodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                DecodeRef::Plain => "d",
                DecodeRef::ByRef => "&d",
            }
        )
    }
}

#[allow(dead_code)]
pub struct GoCodeGen<'model> {
    /// if set, limits declaration output to this namespace only
    pub(crate) namespace: Option<NamespaceID>,
    pub(crate) packages: HashMap<String, PackageName>,
    pub(crate) import_core: String,
    pub(crate) model: Option<&'model Model>,
    pub(crate) package: String,
    pub(crate) is_tinygo: bool,
}

impl<'model> GoCodeGen<'model> {
    pub fn new(model: Option<&'model Model>, is_tinygo: bool) -> Self {
        Self {
            model,
            namespace: None,
            packages: HashMap::default(),
            import_core: String::default(),
            package: String::default(),
            is_tinygo,
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
            Ok(Some(Wasmbus { contract_id: Some(contract_id), .. })) => Some(contract_id),
            _ => None,
        }
    }
}

#[non_exhaustive]
enum MethodArgFlags {
    Normal,
    // ToString,
}

/// Returns the nil or zero of the type
pub fn zero_of(id: &ShapeID, kind: Option<&ShapeKind>) -> &'static str {
    if let Some(ShapeKind::Simple(simple)) = kind {
        // matching on kind is preferable to using id, because it de-aliases to get to the root type
        return match simple {
            Simple::Blob => "make([]byte,0)",
            Simple::Boolean => "false",
            Simple::String => "\"\"",
            Simple::Byte
            | Simple::Short
            | Simple::Integer
            | Simple::Long
            | Simple::Float
            | Simple::Double => "0",
            _ => "nil",
        };
    }

    if id.namespace() == prelude_namespace_id() {
        return match id.shape_name().to_string().as_str() {
            SHAPE_BYTE | SHAPE_SHORT | SHAPE_INTEGER | SHAPE_LONG | SHAPE_FLOAT | SHAPE_DOUBLE => {
                "0"
            }
            SHAPE_BOOLEAN => "false",
            // technically this should be 'nil', but if someone accidentally serializes it,
            // it will cause a panic in the msgpack decoder, so using empty array to avoid panics.
            SHAPE_BLOB => "make([]byte,0)",
            SHAPE_STRING => "\"\"",
            // BigInteger, BigDecimal, Timestamp, Document, unit
            _ => "nil",
        };
    }
    if id.namespace() == wasmcloud_model_namespace() {
        return match id.shape_name().to_string().as_str() {
            "U64" | "U32" | "U16" | "U8" | "I64" | "I32" | "I16" | "I8" | "F64" | "F32" => "0",
            _ => "nil",
        };
    }
    "nil"
}

/// Returns true if the type is returned by value
///  (i.e., not nillable)
pub fn by_value(id: &ShapeID) -> bool {
    let name = id.shape_name().to_string();
    (id.namespace() == prelude_namespace_id()
        && matches!(
            name.as_str(),
            SHAPE_BYTE
                | SHAPE_SHORT
                | SHAPE_INTEGER
                | SHAPE_LONG
                | SHAPE_FLOAT
                | SHAPE_DOUBLE
                | SHAPE_BOOLEAN
                | SHAPE_BLOB
                | SHAPE_STRING
        ))
        || (id.namespace() == wasmcloud_model_namespace()
            && matches!(
                name.as_str(),
                "U64" | "U32" | "U16" | "U8" | "I64" | "I32" | "I16" | "I8" | "F64" | "F32"
            ))
}

impl<'model> CodeGen for GoCodeGen<'model> {
    fn output_language(&self) -> OutputLanguage {
        if self.is_tinygo {
            OutputLanguage::TinyGo
        } else {
            OutputLanguage::Go
        }
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
                            if lang.as_str() == "go" {
                                if let Some(Value::String(ver)) = map.get("min_version") {
                                    let min_ver = semver::Version::parse(ver).map_err(|e| {
                                        Error::InvalidModel(format!(
                                            "metadata parse error for codegen {{ language=go, \
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
                                        "missing 'min_version' in metadata.codegen for lang=go"
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

    /// Set up go formatter based on 'tinygo.formatter' settings in codegen.toml
    fn source_formatter(&self, mut args: Vec<String>) -> Result<Box<dyn SourceFormatter>> {
        if args.is_empty() {
            return Err(Error::Formatter("missing tinygo.formatter setting".into()));
        }
        let program = args.remove(0);
        Ok(Box::new(GoSourceFormatter { program, args }))
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
            None => {
                return Err(Error::Other(format!(
                    "namespace must be defined (in codegen.toml) for go output file {}",
                    file_config.path.display()
                )));
            }
        };
        let ns = self.namespace.as_ref().unwrap();
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
        self.package = if let Some(toml::Value::String(package)) = file_config.params.get("package")
        {
            package.to_string()
        } else {
            match ns.to_string().rsplit_once('.') {
                Some((_, last)) => last.to_string(),
                None => self.namespace.as_ref().unwrap().to_string(),
            }
        };
        self.import_core = if ns == wasmcloud_model_namespace() || ns == wasmcloud_core_namespace()
        {
            String::new()
        } else {
            "actor.".to_string()
        };
        Ok(())
    }

    fn write_source_file_header(
        &mut self,
        w: &mut Writer,
        _model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        if let Some(package_doc) = self
            .packages
            .get(&self.namespace.as_ref().unwrap().to_string())
            .and_then(|p| p.doc.as_ref())
        {
            // for 'godoc', package-level documentation before the package declaration
            writeln!(w, "// {}", &package_doc).unwrap();
        }
        let ns = self.namespace.as_ref().unwrap();
        writeln!(
            w,
            r#"package {}
            import (
                {}
                msgpack "github.com/wasmcloud/tinygo-msgpack" //nolint
                cbor "github.com/wasmcloud/tinygo-cbor" //nolint
            )"#,
            &self.package,
            if ns != wasmcloud_model_namespace() && ns != wasmcloud_core_namespace() {
                "\"github.com/wasmcloud/actor-tinygo\" //nolint"
            } else {
                // avoid circular dependencies - core and model are part of the actor-tinygo package
                ""
            }
        )
        .unwrap();
        Ok(())
    }

    /// Complete generation and return the output bytes
    fn finalize(&mut self, w: &mut Writer) -> Result<bytes::Bytes> {
        writeln!(
            w,
            "\n// This file is generated automatically using wasmcloud/weld-codegen {}",
            env!("CARGO_PKG_VERSION"),
        )
        .unwrap();
        Ok(w.take().freeze())
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
            // traits are only needed for the rust implementation of codegen, so skip them in go
            if traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap()) {
                continue;
            }
            let mut want_serde = !params.contains_key("no_serde");
            match shape {
                ShapeKind::Simple(simple) => {
                    self.declare_simple_shape(w, id.shape_name(), traits, simple)?;
                    // don't generate encoder/decoder for primitive type aliases
                    want_serde = want_serde
                        && (id.namespace() != wasmcloud_model_namespace()
                            || !matches!(
                                id.shape_name().to_string().as_str(),
                                "F64"
                                    | "F32"
                                    | "U64"
                                    | "U32"
                                    | "U16"
                                    | "U8"
                                    | "I64"
                                    | "I32"
                                    | "I16"
                                    | "I8"
                            ));
                }
                ShapeKind::Map(map) => {
                    self.declare_map_shape(w, id.shape_name(), traits, map)?;
                }
                ShapeKind::List(list) => {
                    self.declare_list_or_set_shape(w, id.shape_name(), traits, list)?;
                }
                ShapeKind::Set(set) => {
                    self.declare_list_or_set_shape(w, id.shape_name(), traits, set)?;
                }
                ShapeKind::Structure(strukt) => {
                    self.declare_structure_shape(w, id, traits, strukt)?;
                }
                ShapeKind::Union(_strukt) => {
                    eprintln!(
                        "Warning: Union types are not currently supported for Go: skipping {}",
                        id.shape_name()
                    );
                    want_serde = false;
                    //TODO: union
                    // self.declare_union_shape(w, id, traits, strukt)?;
                }
                ShapeKind::Operation(_)
                | ShapeKind::Resource(_)
                | ShapeKind::Service(_)
                | ShapeKind::Unresolved => {
                    want_serde = false;
                }
            }
            if want_serde {
                self.declare_codec(w, id, shape, false)?;
                self.declare_codec(w, id, shape, true)?;
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
                let service = ServiceInfo { id: id.shape_name(), service, traits };
                self.write_service_interface(w, model, &service)?;
                self.write_service_receiver(w, model, &service)?;
                self.write_service_sender(w, model, &service)?;
            }
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, w: &mut Writer, _kind: CommentKind, line: &str) {
        // all comment types same for Go
        writeln!(w, "// {}", line).unwrap();
    }

    /// generate Go method name: capitalized to make public
    fn to_method_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }

    /// generate Go field name: capitalized to make public
    fn to_field_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }

    /// generate Go type name: PascalCase
    fn to_type_name_case(&self, name: &str) -> String {
        crate::strings::to_pascal_case(name)
    }

    /// returns Go source file extension
    fn get_file_extension(&self) -> &'static str {
        "go"
    }
}

// generate code to invoke encoder, handling alias if applicable. Generates, for example
//      encode_alias!(id,"val","String","String","string")
// generates
//      "encoder.WriteString(val)"  or "encoder.WriteString(string(*val))"
// depending on whether 'id' is a prelude simple shape (left side) or an alias (right side)
macro_rules! encode_alias {
    ( $id:ident, $var:ident, $shape:expr, $encodeFn:expr, $cast:expr ) => {
        format!(
            "encoder.Write{}({})",
            $encodeFn,
            if $id == &ShapeID::new_unchecked(PRELUDE_NAMESPACE, $shape, None) {
                $var.to_string()
            } else {
                format!("{}(*{})", $cast, $var)
            }
        )
    };
}

impl<'model> GoCodeGen<'model> {
    /// Write encoder and decoder for top-level shapes in this package
    fn declare_codec(
        &self,
        w: &mut Writer,
        id: &ShapeID,
        kind: &ShapeKind,
        has_cbor: bool,
    ) -> Result<()> {
        let name = self.type_string(Ty::Shape(id))?;
        let (decode_fn, base_shape) = self.shape_decoder(id, kind, has_cbor)?;
        let fn_prefix = codec_pfx(has_cbor);
        let serde_crate = codec_crate(has_cbor);
        writeln!(
            w,
            r#"// {}Encode serializes a {} using {}
            func (o *{}) {}Encode(encoder {}.Writer) error {{
                {}
                return encoder.CheckError()
            }}
            
            // {}Decode{} deserializes a {} using {}
            func {}Decode{}(d *{}.Decoder) ({},error) {{
                {}
            }}"#,
            fn_prefix,
            &name,
            serde_crate,
            &name,
            fn_prefix,
            serde_crate,
            self.shape_encoder(id, kind, "o", has_cbor)?,
            //
            fn_prefix,
            &name,
            &name,
            serde_crate,
            fn_prefix,
            &name,
            serde_crate,
            &name,
            if self.is_decoder_function(kind) {
                decode_fn
            } else {
                format!(
                    r#"val,err := {}
                  if err != nil {{
                    return {},err
                  }}
                  return {},nil"#,
                    decode_fn,
                    zero_of(&base_shape, Some(kind)), // not needed anymore
                    if &base_shape != id {
                        format!(
                            "{}(val)",
                            self.to_type_name_case(&id.shape_name().to_string())
                        )
                    } else {
                        "val".into()
                    }
                )
            }
            .trim_end_matches('\n')
        )
        .unwrap();
        Ok(())
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
            self.write_documentation(w, id, text);
        }

        // '@deprecated' trait
        if let Some(Some(Value::Object(map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            w.write(b"// @deprecated ");
            if let Some(Value::String(since)) = map.get("since") {
                w.write(&format!("since=\"{}\"\n", since));
            }
            if let Some(Value::String(message)) = map.get("message") {
                w.write(&format!("note=\"{}\"\n", message));
            }
            w.write(b")\n");
        }

        // '@unstable' trait
        if traits.get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap()).is_some() {
            self.write_comment(w, CommentKind::Documentation, "@unstable");
        }
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    /// The lifetime parameter will only be used if the type requires a lifetime
    pub(crate) fn type_string(&self, ty: Ty<'_>) -> Result<String> {
        let mut s = String::new();
        match ty {
            Ty::Opt(id) => {
                s.push_str(&self.type_string(Ty::Shape(id))?);
            }
            Ty::Ref(id) => {
                s.push_str(&self.type_string(Ty::Shape(id))?);
            }
            Ty::Ptr(id) => {
                if !by_value(id) {
                    s.push('*');
                }
                s.push_str(&self.type_string(Ty::Shape(id))?);
            }
            Ty::Shape(id) => {
                let name = id.shape_name().to_string();
                if id.namespace() == prelude_namespace_id() {
                    let ty: String = match name.as_ref() {
                        SHAPE_BLOB => "[]byte".into(),
                        SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool".into(),
                        SHAPE_STRING => "string".into(),
                        SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "int8".into(),
                        SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "int16".into(),
                        SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "int32".into(),
                        SHAPE_LONG | SHAPE_PRIMITIVELONG => "int64".into(),
                        SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "float32".into(),
                        SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "float64".into(),
                        SHAPE_DOCUMENT => format!("{}Document", self.import_core),
                        SHAPE_TIMESTAMP => format!("{}Timestamp", self.import_core),
                        SHAPE_BIGINTEGER => {
                            // TODO: not supported
                            todo!()
                        }
                        SHAPE_BIGDECIMAL => {
                            // TODO: not supported
                            todo!()
                        }
                        _ => return Err(Error::UnsupportedType(name)),
                    };
                    s.push_str(&ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    match name.as_str() {
                        "U64" | "U32" | "U16" | "U8" => {
                            s.push_str("uint");
                            s.push_str(&name[1..])
                        }
                        "I64" | "I32" | "I16" | "I8" => {
                            s.push_str("int");
                            s.push_str(&name[1..]);
                        }
                        "F64" => s.push_str("float64"),
                        "F32" => s.push_str("float32"),
                        _ => {
                            if self.namespace.is_none()
                                || self.namespace.as_ref().unwrap() != id.namespace()
                            {
                                s.push_str(&self.import_core);
                            }
                            s.push_str(&self.to_type_name_case(&name));
                        }
                    };
                } else if self.namespace.is_some()
                    && id.namespace() == self.namespace.as_ref().unwrap()
                {
                    // we are in the same namespace so we don't need to specify namespace
                    s.push_str(&self.to_type_name_case(&id.shape_name().to_string()));
                } else {
                    s.push_str(&self.get_crate_path(id)?);
                    s.push_str(&self.to_type_name_case(&id.shape_name().to_string()));
                }
            }
        }
        Ok(s)
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
        writeln!(
            w,
            "type {} {}",
            &self.to_type_name_case(&id.to_string()),
            match simple {
                Simple::Blob => "[]byte".into(),
                Simple::Boolean => "bool".into(),
                Simple::String => "string".into(),
                Simple::Byte => "int8".into(),
                Simple::Short => "int16".into(),
                Simple::Integer => "int32".into(),
                Simple::Long => "int64".into(),
                Simple::Float => "float32".into(),
                Simple::Double => "float64".into(),
                Simple::Document => format!("{}Document", &self.import_core),
                Simple::Timestamp => format!("{}Timestamp", &self.import_core),
                Simple::BigInteger => {
                    // TODO: unsupported
                    todo!()
                }
                Simple::BigDecimal => {
                    // TODO: unsupported
                    todo!()
                }
            }
        )
        .unwrap();
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
        writeln!(
            w,
            "type {} map[{}]{}",
            &self.to_type_name_case(&id.to_string()),
            &self.type_string(Ty::Shape(shape.key().target()))?,
            &self.type_string(Ty::Shape(shape.value().target()))?,
        )
        .unwrap();
        Ok(())
    }

    fn declare_list_or_set_shape(
        &mut self,
        w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &ListOrSet,
    ) -> Result<()> {
        self.apply_documentation_traits(w, id, traits);
        writeln!(
            w,
            "type {} []{}",
            &self.to_type_name_case(&id.to_string()),
            &self.type_string(Ty::Shape(shape.member().target()))?
        )
        .unwrap();
        Ok(())
    }

    fn declare_structure_shape(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let ident = id.shape_name();
        self.apply_documentation_traits(w, ident, traits);
        writeln!(
            w,
            "type {} struct {{",
            &self.to_type_name_case(&ident.to_string())
        )
        .unwrap();
        let (fields, _is_numbered) = get_sorted_fields(ident, strukt)?;
        for member in fields.iter() {
            self.apply_documentation_traits(w, member.id(), member.traits());
            let (field_name, _ser_name) = self.get_field_name_and_ser_name(member)?;
            let target = member.target();
            let field_tags = "";
            writeln!(
                w,
                "  {} {} {}",
                &field_name,
                self.type_string(if is_optional_field(member, self.shape_kind(target)) {
                    Ty::Ptr(target)
                } else {
                    Ty::Shape(target)
                })?,
                field_tags,
            )
            .unwrap();
        }
        w.write(b"}\n\n");
        Ok(())
    }

    #[allow(dead_code)]
    fn declare_union_shape(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let ident = id.shape_name();
        let (fields, is_numbered) = get_sorted_fields(ident, strukt)?;
        if !is_numbered {
            return Err(Error::Model(format!(
                "union {} must have numbered fields",
                ident
            )));
        }
        self.apply_documentation_traits(w, ident, traits);
        writeln!(w, "// enum {}", ident).unwrap();
        writeln!(w, "type {} uint16", ident).unwrap();
        writeln!(w, "const (").unwrap();
        let mut first_value = true;
        for member in fields.iter() {
            self.apply_documentation_traits(w, member.id(), member.traits());
            let field_num = member.field_num().unwrap();
            let variant_name = self.to_type_name_case(&member.id().to_string());
            if member.target() == crate::model::unit_shape() {
                if first_value {
                    writeln!(w, "    {} {} = {}", variant_name, ident, field_num).unwrap();
                    first_value = false;
                } else {
                    writeln!(w, "    {} = {}", variant_name, field_num).unwrap();
                }
            } else {
                // TODO: not supported yet
                w.write(&format!(
                    "{}({}),\n",
                    variant_name,
                    self.type_string(Ty::Shape(member.target()))?
                ));
            }
        }
        w.write(b")\n\n");

        // generate String method
        writeln!(w, "func (x {}) String() string {{", ident).unwrap();
        w.write(b"  switch x {\n");
        for member in fields.iter() {
            let variant_name = self.to_type_name_case(&member.id().to_string());
            writeln!(
                w,
                "    case {}:\n      return \"{}\"",
                variant_name, variant_name
            )
            .unwrap();
        }
        w.write(b"  }\n  return \"UNDEFINED\"\n}\n");

        Ok(())
    }

    /// Declares the service as a go interface whose methods are the smithy service operations
    fn write_service_interface(
        &mut self,
        w: &mut Writer,
        model: &Model,
        service: &ServiceInfo,
    ) -> Result<()> {
        self.apply_documentation_traits(w, service.id, service.traits);
        writeln!(w, "type {} interface {{", &service.id).unwrap();
        for operation in service.service.operations() {
            // if operation is not declared in this namespace, don't define it here
            if let Some(ref ns) = self.namespace {
                if operation.namespace() != ns {
                    continue;
                }
            }
            let (op, op_traits) = get_operation(model, operation, service.id)?;
            let method_id = operation.shape_name();
            let _flags =
                self.write_method_signature(w, method_id, op_traits, op, MethodSigType::Interface)?;
            w.write(b"\n");
        }
        w.write(b"}\n\n");

        writeln!(
            w,
            r#"
        // {}Handler is called by an actor during `main` to generate a dispatch handler
        // The output of this call should be passed into `actor.RegisterHandlers`
        func {}Handler(actor_ {}) {}Handler {{
            return {}NewHandler("{}", &{}Receiver{{}}, actor_)
        }}"#,
            &service.id,
            &service.id,
            &service.id,
            &self.import_core,
            &self.import_core,
            &service.id,
            &service.id,
        )
        .unwrap();

        self.write_service_contract_getter(w, service)?;
        Ok(())
    }

    /// add getter for capability contract id
    fn write_service_contract_getter(
        &mut self,
        w: &mut Writer,
        service: &ServiceInfo,
    ) -> Result<()> {
        if let Some(contract_id) = service.wasmbus_contract_id() {
            writeln!(
                w,
                r#"// {}ContractId returns the capability contract id for this interface
                func {}ContractId() string {{ return "{}" }} 
                "#,
                service.id, service.id, contract_id
            )
            .unwrap();
        }
        Ok(())
    }

    /// write trait function declaration "async fn method(args) -> Result< return_type, actor.RpcError >"
    /// does not write trailing semicolon so this can be used for declaration and implementation
    fn write_method_signature(
        &mut self,
        w: &mut Writer,
        method_id: &Identifier,
        method_traits: &AppliedTraits,
        op: &Operation,
        sig_type: MethodSigType,
    ) -> Result<MethodArgFlags> {
        let method_name = self.to_method_name(method_id, method_traits);
        let arg_flags = MethodArgFlags::Normal;
        self.apply_documentation_traits(w, method_id, method_traits);
        match sig_type {
            MethodSigType::Interface => {}
            MethodSigType::Sender(s) => write!(w, "func (s *{}Sender) ", s).unwrap(),
        }
        w.write(&method_name);
        write!(w, "(ctx *{}Context", &self.import_core).unwrap();
        // optional input parameter
        if let Some(input_type) = op.input() {
            write!(w, ", arg {}", self.type_string(Ty::Ref(input_type))?).unwrap();
        }
        w.write(b") ");
        // return type with output output parameter
        if let Some(output_type) = op.output() {
            write!(w, "({}, error)", self.type_string(Ty::Ptr(output_type))?).unwrap();
        } else {
            w.write(b"error");
        }
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
            "{}Receiver receives messages defined in the {} service interface",
            service.id, service.id
        );
        self.write_comment(w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(w, service.id, service.traits);
        let proto = crate::model::wasmbus_proto(service.traits)?;
        let has_cbor = proto.map(|pv| pv.has_cbor()).unwrap_or(false);
        writeln!(w, "type {}Receiver struct {{}}", service.id).unwrap();
        writeln!(
            w,
            r#"func (r* {}Receiver) Dispatch(ctx *{}Context, svc interface{{}}, message *{}Message) (*{}Message, error) {{
                svc_,_ := svc.({})
                switch message.Method {{
                 "#,
            &service.id,
            &self.import_core,
            &self.import_core,
            &self.import_core,
            &service.id,
        ).unwrap();

        for method_id in service.service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, method_traits) = get_operation(model, method_id, service.id)?;
            w.write(b"case \"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\" : {\n");
            if let Some(op_input) = op.input() {
                // decode input into 'value'
                writeln!(
                    w,
                    r#"
                        d := {}.NewDecoder(message.Arg)
                        value,err_ := {}
                        if err_ != nil {{ 
                            return nil,err_
                        }}
                        "#,
                    codec_crate(has_cbor),
                    self.value_decoder(op_input, DecodeRef::ByRef, has_cbor)?,
                )
                .unwrap();
            }
            // resp, err := svc_.method(ctx, &value);
            let method_name = self.to_method_name(method_ident, method_traits);
            writeln!(
                w,
                r#"{} := svc_.{} (ctx{})
                if err != nil {{ 
                    return nil,err
                }}"#,
                if op.output().is_some() { "resp, err" } else { "err" },
                &method_name,
                if op.has_input() { ", value" } else { "" },
            )
            .unwrap();
            // serialize result
            if let Some(op_output) = op.output() {
                writeln!(
                    w,
                    r#"
            	    var sizer {}.Sizer
            	    size_enc := &sizer
            	    {} 
            	    buf := make([]byte, sizer.Len())
            	    encoder := {}.NewEncoder(buf)
            	    enc := &encoder
                    {}"#,
                    codec_crate(has_cbor),
                    self.value_encoder(op_output, "resp", "size_enc", has_cbor)?,
                    codec_crate(has_cbor),
                    self.value_encoder(op_output, "resp", "enc", has_cbor)?,
                )
                .unwrap();
            } else {
                w.write(b"buf := make([]byte, 0)\n");
            }
            // return result,
            writeln!(
                w,
                r#" return &{}Message {{ Method: "{}", Arg: buf }}, nil
                    }}"#,
                &self.import_core,
                &self.full_dispatch_name(service.id, method_ident),
            )
            .unwrap();
        }
        // handle fallthrough case of unrcognied operation
        // end case, end dispatch function
        writeln!(
            w,
            r#"default: 
                   return nil, {}NewRpcError("MethodNotHandled", "{}." + message.Method)
               }}
            }}
            "#,
            &self.import_core, service.id,
        )
        .unwrap();
        Ok(())
    }

    /// writes the service sender struct and constructor
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
        let core_prefix = &self.import_core;
        writeln!(
            w,
            r#"type {}Sender struct {{ transport {}Transport }}
            {}
            {}"#,
            service.id,
            core_prefix,
            self.actor_receive_sender_constructors(service.id, service.traits)?,
            self.provider_receive_sender_constructors(service.id, service.traits)?,
        )
        .unwrap();

        for method_id in service.service.operations() {
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, op_traits) = get_operation(model, method_id, service.id)?;
            let _arg_is_string = false;
            let _flags = self.write_method_signature(
                w,
                method_ident,
                op_traits,
                op,
                MethodSigType::Sender(service.id.to_string()),
            )?;
            w.write(b" {\n");
            if let Some(op_input) = op.input() {
                writeln!(
                    w,
                    r#"
            	    var sizer {}.Sizer
            	    size_enc := &sizer
            	    {} 
            	    buf := make([]byte, sizer.Len())
            	    
            	    var encoder = {}.NewEncoder(buf)
            	    enc := &encoder
                    {}
            	"#,
                    codec_crate(has_cbor),
                    self.value_encoder(op_input, "arg", "size_enc", has_cbor,)?,
                    codec_crate(has_cbor),
                    self.value_encoder(op_input, "arg", "enc", has_cbor,)?,
                )
                .unwrap();
            } else {
                w.write(b"buf := make([]byte,0)\n");
            }
            writeln!(
                w,
                r#"{}s.transport.Send(ctx, {}Message{{ Method: "{}", Arg:buf }})"#,
                if op.output().is_some() { "out_buf,_ := " } else { "" },
                &self.import_core,
                &self.full_dispatch_name(service.id, method_ident),
            )
            .unwrap();
            if let Some(op_output) = op.output() {
                let out_kind = self.shape_kind(op_output);
                writeln!(
                    w,
                    r#"d := {}.NewDecoder(out_buf)
                        resp,err_ := {}
                        if err_ != nil {{ 
                            return {},err_
                        }}
                        return {}resp,nil
                     }}"#,
                    codec_crate(has_cbor),
                    self.value_decoder(op_output, DecodeRef::ByRef, has_cbor)?,
                    zero_of(op_output, out_kind),
                    // use ptr for nillable return types
                    if by_value(op_output) { "" } else { "&" }
                )
                .unwrap();
            } else {
                w.write(b"return nil\n}\n");
            }
        }
        Ok(())
    }

    /// add sender constructors for calling actors, for services that declare actorReceive
    #[cfg(feature = "wasmbus")]
    fn actor_receive_sender_constructors(
        &self,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
    ) -> Result<String> {
        let ctors = if let Some(Wasmbus { actor_receive: true, .. }) =
            get_trait(service_traits, crate::model::wasmbus_trait())?
        {
            format!(
                r#"
                // NewActorSender constructs a client for actor-to-actor messaging
                // using the recipient actor's public key
                func NewActor{}Sender(actor_id string) *{}Sender {{
                    transport := {}ToActor(actor_id)
                    return &{}Sender {{ transport: transport }}
                }}
                "#,
                service_id, service_id, &self.import_core, service_id
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
        &self,
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
                // NewProvider constructs a client for sending to a {} provider
                // implementing the '{}' capability contract, with the "default" link
                func NewProvider{}() *{}Sender {{
                    transport := {}ToProvider("{}", "default")
                    return &{}Sender {{ transport: transport }}
                }}

                // NewProvider{}Link constructs a client for sending to a {} provider
                // implementing the '{}' capability contract, with the specified link name
                func NewProvider{}Link(linkName string) *{}Sender {{
                    transport :=  {}ToProvider("{}", linkName)
                    return &{}Sender {{ transport: transport }}
                }}
                "#,
                // new provider
                service_id,
                contract,
                service_id,
                service_id,
                &self.import_core,
                contract,
                service_id,
                // new provider link
                service_id,
                service_id,
                contract,
                service_id,
                service_id,
                &self.import_core,
                contract,
                service_id,
            )
        } else {
            String::new()
        };
        Ok(ctors)
    }

    /// returns the package prefix for the symbol, using metadata crate declarations
    pub(crate) fn get_crate_path(&self, id: &ShapeID) -> Result<String> {
        let namespace = id.namespace();
        if namespace == self.namespace.as_ref().unwrap() {
            return Ok(String::new());
        }
        if namespace == wasmcloud_model_namespace() || namespace == wasmcloud_core_namespace() {
            return Ok(self.import_core.clone());
        }
        if namespace == prelude_namespace_id() {
            if id.shape_name() == &Identifier::new_unchecked(SHAPE_TIMESTAMP)
                || id.shape_name() == &Identifier::new_unchecked(SHAPE_DOCUMENT)
            {
                return Ok(self.import_core.clone());
            }
            return Ok(String::new());
        }

        // look up the crate name, which should be valid go syntax
        match self.packages.get(&namespace.to_string()) {
            Some(crate::model::PackageName { go_package: Some(go_package), .. }) => {
                Ok(format!("{}.", go_package))
            }
            _ => Err(Error::Model(format!(
                "undefined go_package for namespace '{}' symbol '{}'. Make sure codegen.toml includes \
                 all dependent namespaces, and that the dependent .smithy file contains package \
                 metadata with go_package: value",
                namespace,
                id.shape_name(),
            ))),
        }
    }

    /// Generate string to encode structure.
    /// Second Result field is true if structure has no fields, e.g., "MyStruct {}"
    fn struct_encode(
        &self,
        id: &ShapeID,
        strukt: &StructureOrUnion,
        val: &str,
        has_cbor: bool,
    ) -> Result<String> {
        let (fields, _) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        let mut s = String::new();
        writeln!(s, "encoder.WriteMapSize({})", fields.len()).unwrap();
        for field in fields.iter() {
            let (field_name, ser_name) = self.get_field_name_and_ser_name(field)?;
            writeln!(s, "encoder.WriteString(\"{}\")", &ser_name).unwrap();
            let field_val = self.value_encoder(
                field.target(),
                &format!("{}.{}", val, &field_name),
                "encoder",
                has_cbor,
            )?;
            if is_optional_field(field, self.shape_kind(field.target())) {
                writeln!(
                    s,
                    r#"if {}.{} == nil {{
                        encoder.WriteNil()
                    }} else {{
                        {}
                    }}"#,
                    val, &field_name, &field_val
                )
                .unwrap();
            } else {
                writeln!(s, "{}", &field_val).unwrap();
            }
        }
        Ok(s)
    }

    /// Generate string to decode structure.
    fn struct_decode(
        &self,
        id: &ShapeID,
        strukt: &StructureOrUnion,
        has_cbor: bool,
    ) -> Result<String> {
        let (fields, _) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        let mut s = String::new();
        writeln!(
            s,
            r#"var val {}
            isNil,err := d.IsNextNil()
            if err != nil || isNil {{ 
                return val,err 
            }}
            {}
            if err != nil {{ return val,err }}
            for i := uint32(0); i < size; i++ {{
                field,err := d.ReadString()
                if err != nil {{ return val,err }}
                switch field {{"#,
            self.to_type_name_case(&id.shape_name().to_string()),
            if has_cbor {
                r#"size,indef,err := d.ReadMapSize()
                if err != nil && indef { err = cbor.NewReadError("indefinite maps not supported")}"#
            } else {
                r#"size,err := d.ReadMapSize()"#
            },
        )
        .unwrap();
        for field in fields.iter() {
            let (field_name, ser_name) = self.get_field_name_and_ser_name(field)?;
            writeln!(s, "case \"{}\":", ser_name).unwrap();
            if is_optional_field(field, self.shape_kind(field.target())) {
                writeln!(
                    s,
                    r#"fval,err := {}
                  if err != nil {{ return val, err }}
                  val.{} = &fval"#,
                    &self.value_decoder(field.target(), DecodeRef::Plain, has_cbor)?,
                    &field_name,
                )
                .unwrap();
            } else {
                writeln!(
                    s,
                    r#"val.{},err = {}"#,
                    &field_name,
                    &self.value_decoder(field.target(), DecodeRef::Plain, has_cbor)?,
                )
                .unwrap();
            }
        }
        writeln!(
            s,
            r#" default: 
                err = d.Skip()
            }}
            if err != nil {{
                return val, err
            }}
            }}
            return val,nil"#,
        )
        .unwrap();
        Ok(s)
    }

    /// Generates statements to encode the shape.
    fn shape_encoder(
        &self,
        id: &ShapeID,
        kind: &ShapeKind,
        val: &str,
        has_cbor: bool,
    ) -> Result<String> {
        let s = match kind {
            ShapeKind::Simple(simple) => match simple {
                Simple::Blob => encode_alias!(id, val, SHAPE_BLOB, "ByteArray", "[]byte"),
                Simple::Boolean => encode_alias!(id, val, SHAPE_BOOLEAN, "Bool", "boolean"),
                Simple::String => encode_alias!(id, val, SHAPE_STRING, "String", "string"),
                Simple::Byte => encode_alias!(id, val, SHAPE_BYTE, "Uint8", "uint8"),
                Simple::Short => encode_alias!(id, val, SHAPE_SHORT, "Uint16", "uint16"),
                Simple::Integer => encode_alias!(id, val, SHAPE_INTEGER, "Uint32", "uint32"),
                Simple::Long => encode_alias!(id, val, SHAPE_LONG, "Uint64", "uint64"),
                Simple::Float => encode_alias!(id, val, SHAPE_FLOAT, "Float32", "float32"),
                Simple::Double => encode_alias!(id, val, SHAPE_DOUBLE, "Float64", "float64"),
                Simple::Timestamp => {
                    format!(
                        "{}{}EncodeTimestamp(encoder,{}))",
                        &self.import_core,
                        codec_pfx(has_cbor),
                        val
                    )
                }
                Simple::Document => format!(
                    "{}.{}EncodeDocument({}))",
                    &self.import_core,
                    codec_pfx(has_cbor),
                    val
                ),
                Simple::BigInteger => todo!(),
                Simple::BigDecimal => todo!(),
            },
            ShapeKind::Map(map) => {
                // make sure key & val names are unique within scope of the current struct
                // (in case there are >1 map)
                let key_var = format!("key_{}", crate::strings::to_camel_case(val));
                let val_var = format!("val_{}", crate::strings::to_camel_case(val));
                format!(
                    r#"encoder.WriteMapSize(uint32(len(*{})))
                    for {},{} := range *{} {{
                        {}
                        {}
                    }}        
                    "#,
                    val,
                    &key_var,
                    &val_var,
                    val,
                    &self.value_encoder(map.key().target(), &key_var, "encoder", has_cbor,)?,
                    &self.value_encoder(map.value().target(), &val_var, "encoder", has_cbor,)?,
                )
            }
            ShapeKind::List(list) => {
                let item_var = format!("item_{}", crate::strings::to_camel_case(val));
                format!(
                    r#"
                    encoder.WriteArraySize(uint32(len(*{})))
                    for _,{} := range *{} {{
                        {}
                    }}
                    "#,
                    val,
                    &item_var,
                    val,
                    &self.value_encoder(list.member().target(), &item_var, "encoder", has_cbor,)?
                )
            }
            ShapeKind::Set(set) => {
                let item_var = format!("item_{}", crate::strings::to_camel_case(val));
                let mut s = format!(
                    r#"
                    encoder.WriteArraySize(len({}))
                    for _,{} := range {} {{
                    "#,
                    val, &item_var, val,
                );
                s.push_str(&self.value_encoder(
                    set.member().target(),
                    &item_var,
                    "encoder",
                    has_cbor,
                )?);
                s.push_str("\n}\n");
                s
            }
            ShapeKind::Structure(struct_) => {
                if id != crate::model::unit_shape() {
                    self.struct_encode(id, struct_, val, has_cbor)?
                } else {
                    "encoder.WriteNil()".to_string()
                }
            }
            ShapeKind::Union(_) => {
                todo!("unions not supported");
            }
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => String::new(),
        };
        Ok(s)
    }

    /// returns true if the decoder expects to be in a function and has a return statement
    fn is_decoder_function(&self, kind: &ShapeKind) -> bool {
        matches!(
            kind,
            ShapeKind::Map(_)
                | ShapeKind::List(_)
                | ShapeKind::Set(_)
                | ShapeKind::Structure(_)
                | ShapeKind::Union(_)
        )
    }

    /// Generates statements to decode the shape.
    /// set 'return_val' true to force expression to include 'return' statement on simple types
    fn shape_decoder(
        &self,
        id: &ShapeID,
        kind: &ShapeKind,
        has_cbor: bool,
    ) -> Result<(String, ShapeID)> {
        let res = match kind {
            ShapeKind::Simple(simple) => match simple {
                Simple::Blob => (
                    "d.ReadByteArray()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_BLOB, None),
                ),
                Simple::Boolean => (
                    "d.ReadBool()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_BOOLEAN, None),
                ),
                Simple::String => (
                    "d.ReadString()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_STRING, None),
                ),
                Simple::Byte => (
                    "d.ReadUint8()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_BYTE, None),
                ),
                Simple::Short => (
                    "d.ReadUint16()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_SHORT, None),
                ),
                Simple::Integer => (
                    "d.ReadUint32()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_INTEGER, None),
                ),

                Simple::Long => (
                    "d.ReadUint64()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_LONG, None),
                ),
                Simple::Float => (
                    "d.ReadFloat32()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_FLOAT, None),
                ),
                Simple::Double => (
                    "d.ReadFloat64()".into(),
                    ShapeID::new_unchecked(PRELUDE_NAMESPACE, SHAPE_DOUBLE, None),
                ),
                Simple::Timestamp => (
                    format!("{}Timestamp.Encode()", &self.import_core),
                    id.clone(),
                ),
                Simple::Document => (
                    format!("{}Document.Encode()", &self.import_core),
                    id.clone(),
                ),
                Simple::BigInteger => todo!(),
                Simple::BigDecimal => todo!(),
            },
            ShapeKind::Map(map) => {
                let key_type = self.type_string(Ty::Shape(map.key().target()))?;
                let val_type = self.type_string(Ty::Shape(map.value().target()))?;
                (
                    format!(
                        r#"isNil,err := d.IsNextNil()
                        if err != nil || isNil {{
                       		return make(map[{}]{}, 0), err
                        }}
                       	{}
                        if err != nil {{ return make(map[{}]{}, 0),err }}
                        val := make(map[{}]{}, size)
                        for i := uint32(0); i < size; i++ {{
                           k,_ := {}
                           v,err := {}
                           if err != nil {{ return val, err }}
                           val[k] = v
                        }}
                        return val,nil"#,
                        &key_type,
                        &val_type,
                        if has_cbor {
                            r#"size,indef,err := d.ReadMapSize()
                if err != nil && indef { err = cbor.NewReadError("indefinite maps not supported") }"#
                        } else {
                            r#"size,err := d.ReadMapSize()"#
                        },
                        &key_type,
                        &val_type,
                        &key_type,
                        &val_type,
                        &self.value_decoder(map.key().target(), DecodeRef::Plain, has_cbor)?,
                        &self.value_decoder(map.value().target(), DecodeRef::Plain, has_cbor)?,
                    ),
                    id.clone(),
                )
            }
            ShapeKind::List(list) => {
                let item_type = self.type_string(Ty::Shape(list.member().target()))?;
                (
                    format!(
                        r#"isNil,err := d.IsNextNil()
                        if err != nil || isNil {{
                       		return make([]{}, 0), err
                        }}
                       	{}
                        if err != nil {{ return make([]{}, 0 ), err }}
                        val := make([]{}, size)
                        for i := uint32(0); i < size; i++ {{
                           item,err := {}
                           if err != nil {{ return val, err }}
                           val = append(val,item)
                        }}
                        return val,nil"#,
                        &item_type,
                        if has_cbor {
                            r#"size,indef,err := d.ReadArraySize()
                if err != nil && indef { err = cbor.NewReadError("indefinite arrays not supported") }"#
                        } else {
                            r#"size,err := d.ReadArraySize()"#
                        },
                        &item_type,
                        &item_type,
                        &self.value_decoder(list.member().target(), DecodeRef::Plain, has_cbor)?,
                    ),
                    id.clone(),
                )
            }
            ShapeKind::Set(set) => {
                let item_type = self.to_type_name_case(&set.member().target().to_string());
                (
                    format!(
                        r#"isNil,err := d.IsNextNil()
                        if err != nil || isNil {{
                       		return make([]{}, 0), err
                        }}
                       	{}
                        if err != nil {{ return make([]{},0),err }}
                        val := make([]{}, size)
                        for i := uint32(0); i < size; i++ {{
                           item,err := {}
                           if err != nil {{ return val, err }}
                           val = append(val,item)
                        }}
                        return val,nil"#,
                        &item_type,
                        if has_cbor {
                            r#"size,indef,err := d.ReadArraySize()
                        if err != nil && indef { err = cbor.NewReadError("indefinite arrays not supported")}"#
                        } else {
                            r#"size,err := d.ReadArraySize()"#
                        },
                        &item_type,
                        &item_type,
                        &self.value_decoder(set.member().target(), DecodeRef::Plain, has_cbor)?,
                    ),
                    id.clone(),
                )
            }
            ShapeKind::Structure(struct_) => {
                if id != crate::model::unit_shape() {
                    (self.struct_decode(id, struct_, has_cbor)?, id.clone())
                } else {
                    (
                        r#"_ = d.Skip()
                        return Unit{},nil"#
                            .into(),
                        id.clone(),
                    )
                }
            }
            ShapeKind::Union(_) => {
                todo!("unions not supported");
            }
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => (String::new(), id.clone()),
        };
        Ok(res)
    }

    /// write statement(s) to encode an object
    pub(crate) fn value_encoder(
        &self,
        id: &ShapeID,
        val: &str,
        e: &str,
        has_cbor: bool,
    ) -> Result<String> {
        let name = id.shape_name().to_string();
        let serde_fn = codec_pfx(has_cbor);
        let stmt = if id.namespace() == prelude_namespace_id() {
            match name.as_ref() {
                SHAPE_BLOB => format!("{}.WriteByteArray({})", e, val),
                SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => format!("{}.WriteBool({})", e, val),
                SHAPE_STRING => format!("{}.WriteString({})", e, val),
                SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => format!("{}.WriteUint8({})", e, val),
                SHAPE_SHORT | SHAPE_PRIMITIVESHORT => format!("{}.WriteUint16({})", e, val),
                SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => {
                    format!("{}.WriteUint32({})", e, val)
                }
                SHAPE_LONG | SHAPE_PRIMITIVELONG => format!("{}.WriteUint64({})", e, val),
                SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => format!("{}.WriteFloat32({})", e, val),
                SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => {
                    format!("{}.WriteFloat64({})", e, val)
                }
                SHAPE_TIMESTAMP => format!("{}.{}Encode({})", val, serde_fn, e),
                //SHAPE_DOCUMENT => todo!(),
                //SHAPE_BIGINTEGER => todo!(),
                //SHAPE_BIGDECIMAL => todo!(),
                _ => return Err(Error::UnsupportedType(name)),
            }
        } else if id.namespace() == wasmcloud_model_namespace() {
            match name.as_bytes() {
                b"U64" => format!("{}.WriteUint64({})", e, val),
                b"U32" => format!("{}.WriteUint32({})", e, val),
                b"U16" => format!("{}.WriteUint16({})", e, val),
                b"U8" => format!("{}.WriteUint8({})", e, val),
                b"I64" => format!("{}.WriteInt64({})", e, val),
                b"I32" => format!("{}.WriteInt32({})", e, val),
                b"I16" => format!("{}.WriteInt16({})", e, val),
                b"I8" => format!("{}.WriteInt8({})", e, val),
                b"F64" => format!("{}.WriteFloat64({})", e, val),
                b"F32" => format!("{}.WriteFloat32({})", e, val),
                _ => format!("{}.{}Encode({})", val, serde_fn, e,),
            }
        } else {
            format!("{}.{}Encode({})", val, serde_fn, e)
        };
        Ok(stmt)
    }

    /// write statement(s) to decode an object
    pub(crate) fn value_decoder(
        &self,
        id: &ShapeID,
        d_byref: DecodeRef,
        has_cbor: bool,
    ) -> Result<String> {
        let name = id.shape_name().to_string();
        let stmt = if id.namespace() == prelude_namespace_id() {
            match name.as_ref() {
                SHAPE_BLOB => "d.ReadByteArray()".into(),
                SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "d.ReadBool()".into(),
                SHAPE_STRING => "d.ReadString()".into(),
                SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "d.ReadUint8()".into(),
                SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "d.ReadUint16()".into(),
                SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "d.ReadUint32()".into(),
                SHAPE_LONG | SHAPE_PRIMITIVELONG => "d.ReadUint64()".into(),
                SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "d.ReadFloat32()".into(),
                SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "d.ReadFloat64()".into(),
                SHAPE_TIMESTAMP => format!(
                    "{}{}DecodeTimestamp({})",
                    &self.import_core,
                    codec_pfx(has_cbor),
                    &d_byref
                ),
                SHAPE_DOCUMENT => format!(
                    "{}{}DecodeDocument({})",
                    &self.import_core,
                    codec_pfx(has_cbor),
                    &d_byref
                ),
                //SHAPE_BIGINTEGER => todo!(),
                //SHAPE_BIGDECIMAL => todo!(),
                _ => return Err(Error::UnsupportedType(name)),
            }
        } else if id.namespace() == wasmcloud_model_namespace() {
            match name.as_bytes() {
                b"U64" => "d.ReadUint64()".into(),
                b"U32" => "d.ReadUint32()".into(),
                b"U16" => "d.ReadUint16()".into(),
                b"U8" => "d.ReadUint8()".into(),
                b"I64" => "d.ReadInt64()".into(),
                b"I32" => "d.ReadInt32()".into(),
                b"I16" => "d.ReadInt16()".into(),
                b"I8" => "d.ReadInt8()".into(),
                b"F64" => "d.ReadFloat64()".into(),
                b"F32" => "d.ReadFloat32()".into(),
                _ => format!(
                    "{}{}Decode{}({})",
                    &self.import_core,
                    codec_pfx(has_cbor),
                    crate::strings::to_pascal_case(&id.shape_name().to_string()),
                    &d_byref,
                ),
            }
        } else {
            format!(
                "{}{}Decode{}({})",
                self.get_crate_path(id)?,
                codec_pfx(has_cbor),
                crate::strings::to_pascal_case(&id.shape_name().to_string()),
                &d_byref,
            )
        };
        Ok(stmt)
    }

    fn shape_kind(&self, id: &ShapeID) -> Option<&ShapeKind> {
        self.model.unwrap().shape(id).map(|ts| ts.body())
    }
} // impl GoCodeGen

/// is_optional_type determines whether the field should be declared as *Field in its struct.
/// the value is true if it is nillable and either isn't required or has an explicit `box` trait
pub(crate) fn is_optional_field(field: &MemberShape, kind: Option<&ShapeKind>) -> bool {
    (field.is_boxed() || !field.is_required()) && zero_of(field.target(), kind) == "nil"
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

pub struct GoSourceFormatter {
    pub program: String,
    pub args: Vec<String>,
}

impl SourceFormatter for GoSourceFormatter {
    fn run(&self, source_files: &[&str]) -> Result<()> {
        // we get an error if the files are in different packages,
        // so run once per file in case output packages differ
        for f in source_files {
            // TODO(future): caller converts array of paths to array of str, and we convert back to path again.
            // ... we could change the api to this fn to take array of Path or PathBuf instead
            let mut args = self.args.clone();
            let path = std::fs::canonicalize(f)?;
            args.push(path.to_string_lossy().to_string());
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            if let Err(e) = format::run_command(&self.program, &str_args) {
                eprintln!("Warning:  formatting '{}': {}", path.display(), e);
            }
        }
        Ok(())
    }
}
