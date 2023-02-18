// CBOR Encode functions
//
// The encoder is written as a plain function "encode_<S>" where S is the type name
// (camel cased for the fn name), and scoped to the module where S is defined.
use std::{fmt::Write as _, string::ToString};

use atelier_core::{
    model::{
        shapes::{HasTraits, ShapeKind, Simple, StructureOrUnion},
        HasIdentity, ShapeID,
    },
    prelude::{
        prelude_namespace_id, SHAPE_BIGDECIMAL, SHAPE_BIGINTEGER, SHAPE_BLOB, SHAPE_BOOLEAN,
        SHAPE_BYTE, SHAPE_DOCUMENT, SHAPE_DOUBLE, SHAPE_FLOAT, SHAPE_INTEGER, SHAPE_LONG,
        SHAPE_PRIMITIVEBOOLEAN, SHAPE_PRIMITIVEBYTE, SHAPE_PRIMITIVEDOUBLE, SHAPE_PRIMITIVEFLOAT,
        SHAPE_PRIMITIVEINTEGER, SHAPE_PRIMITIVELONG, SHAPE_PRIMITIVESHORT, SHAPE_SHORT,
        SHAPE_STRING, SHAPE_TIMESTAMP,
    },
};

use crate::{
    codegen_py::PythonCodeGen,
    codegen_rust::is_optional_type,
    error::{Error, Result},
    gen::{spaces, CodeGen},
    model::wasmcloud_model_namespace,
    writer::Writer,
};

#[derive(Clone, Copy)]
pub(crate) enum ValExpr<'s> {
    Plain(&'s str),
    Ref(&'s str),
}
impl<'s> ValExpr<'s> {
    /// returns borrowed reference to value
    pub(crate) fn as_ref(&self) -> &str {
        self.as_str()
    }

    /// returns value as-is
    pub(crate) fn as_str(&self) -> &str {
        match self {
            ValExpr::Plain(s) => s,
            ValExpr::Ref(s) => s,
        }
    }

    /// returns value for copyable types
    pub(crate) fn as_copy(&self) -> &str {
        self.as_str()
    }
}

fn encode_blob(val: ValExpr) -> String {
    format!("e.encode_bytearray({})\n", &val.as_ref())
}
fn encode_boolean(val: ValExpr) -> String {
    format!("e.encode_boolean({})\n", val.as_copy())
}
fn encode_str(val: ValExpr) -> String {
    format!("e.encode_string({})\n", val.as_ref())
}
fn encode_integer(val: ValExpr) -> String {
    format!("e.encode_int({})\n", val.as_copy())
}
fn encode_float(val: ValExpr) -> String {
    format!("e.encode_float({})\n", val.as_str())
}
fn encode_document(val: ValExpr) -> String {
    format!("e.encode_bytearray({})\n", val.as_ref())
}
fn encode_timestamp(_val: ValExpr) -> String {
    todo!(); // tag timestamp
}
fn encode_big_integer(val: ValExpr) -> String {
    format!("e.encode_int({})\n", val.as_copy())
}
fn encode_big_decimal(val: ValExpr) -> String {
    format!("e.encode_decimal({})\n", val.as_copy())
}

impl<'model> PythonCodeGen<'model> {
    /// Generates cbor encode statements "e.func()" for the id.
    /// If id is a primitive type, writes the direct encode function, otherwise,
    /// delegates to an encode_* function created in the same module where the symbol is defined
    pub(crate) fn encode_shape_id(&self, id: &ShapeID, val: ValExpr) -> Result<String> {
        let name = id.shape_name().to_string();
        let stmt = if id.namespace() == prelude_namespace_id() {
            match name.as_ref() {
                SHAPE_BLOB => encode_blob(val),
                SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => encode_boolean(val),
                SHAPE_STRING => encode_str(val),
                SHAPE_BYTE
                | SHAPE_PRIMITIVEBYTE
                | SHAPE_SHORT
                | SHAPE_PRIMITIVESHORT
                | SHAPE_INTEGER
                | SHAPE_PRIMITIVEINTEGER
                | SHAPE_LONG
                | SHAPE_PRIMITIVELONG => encode_integer(val),

                SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT | SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => {
                    encode_float(val)
                }
                SHAPE_TIMESTAMP => encode_timestamp(val),
                SHAPE_BIGINTEGER => encode_big_integer(val),
                SHAPE_BIGDECIMAL => encode_big_decimal(val),
                SHAPE_DOCUMENT => encode_document(val),
                _ => return Err(Error::UnsupportedType(name)),
            }
        } else if id.namespace() == wasmcloud_model_namespace() {
            match name.as_bytes() {
                b"U64" | b"U32" | b"U16" | b"U8" | b"I64" | b"I32" | b"I16" | b"I8" => {
                    encode_integer(val)
                }
                _ => {
                    let mut s = String::new();
                    if self.namespace.is_none()
                        || self.namespace.as_ref().unwrap() != wasmcloud_model_namespace()
                    {
                        s.push_str(&self.import_core);
                        s.push('.');
                    }
                    writeln!(
                        s,
                        "encode_{}(e,{})",
                        crate::strings::to_camel_case(&id.shape_name().to_string()),
                        val.as_ref()
                    )
                    .unwrap();
                    s
                }
            }
        } else if self.namespace.is_some() && id.namespace() == self.namespace.as_ref().unwrap() {
            format!(
                "encode_{}(e,{})\n",
                crate::strings::to_camel_case(&id.shape_name().to_string()),
                val.as_ref()
            )
        } else {
            match self.packages.get(&id.namespace().to_string()) {
                Some(crate::model::PackageName { py_module: Some(py_module), .. }) => {
                    // the crate name should be valid rust syntax. If not, they'll get an error with rustc
                    format!(
                        "{}::encode_{}(e, {})\n",
                        &py_module,
                        crate::strings::to_snake_case(&id.shape_name().to_string()),
                        val.as_ref(),
                    )
                }
                _ => {
                    return Err(Error::Model(format!(
                        "undefined py_module for namespace {} for symbol {}. Make sure \
                         codegen.toml includes all dependent namespaces, and that the dependent \
                         .smithy file contains package metadata with py_module: value",
                        &id.namespace(),
                        &id
                    )));
                }
            }
        };
        Ok(stmt)
    }

    /// Generates and writes statements to encode the shape.
    fn encode_shape_kind(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        kind: &ShapeKind,
        val: ValExpr,
    ) -> Result<()> {
        match kind {
            ShapeKind::Simple(simple) => {
                let s = match simple {
                    Simple::Blob => encode_blob(val),
                    Simple::Boolean => encode_boolean(val),
                    Simple::String => encode_str(val),
                    Simple::Byte | Simple::Short | Simple::Long | Simple::Integer => {
                        encode_integer(val)
                    }
                    Simple::Float | Simple::Double => encode_float(val),
                    Simple::Timestamp => encode_timestamp(val),
                    Simple::BigInteger => encode_big_integer(val),
                    Simple::BigDecimal => encode_big_decimal(val),
                    Simple::Document => encode_document(val),
                };
                w.write(spaces(self.indent_level));
                w.write(&s);
                w.write(b"\n");
            }
            ShapeKind::Map(map) => {
                w.write(spaces(self.indent_level));
                w.write(&format!("e.encode_length(5, len({}))\n", val.as_str()));
                w.write(spaces(self.indent_level));
                w.write(&format!("for (k,v) in {}:\n", val.as_str()));
                {
                    self.indent_level += 1;
                    w.write(spaces(self.indent_level));
                    w.write(&self.encode_shape_id(map.key().target(), ValExpr::Ref("k"))?);
                    w.write(spaces(self.indent_level));
                    w.write(&self.encode_shape_id(map.value().target(), ValExpr::Ref("v"))?);
                    self.indent_level -= 1;
                }
            }
            ShapeKind::List(list) => {
                w.write(spaces(self.indent_level));
                w.write(&format!("e.encode_length(4, len({}))\n", val.as_str()));
                w.write(spaces(self.indent_level));
                w.write(&format!("for item in {}:\n", val.as_str()));
                {
                    self.indent_level += 1;
                    w.write(spaces(self.indent_level));
                    w.write(&self.encode_shape_id(list.member().target(), ValExpr::Ref("item"))?);
                    self.indent_level -= 1;
                }
            }
            ShapeKind::Set(set) => {
                w.write(spaces(self.indent_level));
                w.write(&format!("e.encode_length(4, len({}))\n", val.as_str()));
                w.write(spaces(self.indent_level));
                w.write(&format!("for item in {}:\n", val.as_str()));
                {
                    self.indent_level += 1;
                    w.write(spaces(self.indent_level));
                    w.write(&self.encode_shape_id(set.member().target(), ValExpr::Ref("item"))?);
                    self.indent_level -= 1;
                }
            }
            ShapeKind::Structure(struct_) => {
                self.encode_struct(w, id, struct_, val)?;
            }
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => {}

            ShapeKind::Union(_) => {
                unimplemented!();
            }
        };
        Ok(())
    }

    /// Generates and writes statements to encode the struct
    fn encode_struct(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        strukt: &StructureOrUnion,
        val: ValExpr,
    ) -> Result<()> {
        let (fields, is_numbered) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        // use array encoding if fields are declared with numbers
        let as_array = is_numbered;
        let field_max_index = if as_array && !fields.is_empty() {
            fields.iter().map(|f| f.field_num().unwrap()).max().unwrap()
        } else {
            fields.len() as u16
        };

        w.write(spaces(self.indent_level));
        if as_array {
            w.write(&format!("e.encode_length(4, {field_max_index})\n"));
        } else {
            w.write(&format!("e.encode_length(5, len({}))\n", val.as_str()));
        }
        let mut current_index = 0;
        for field in fields.iter() {
            if let Some(field_num) = field.field_num() {
                if as_array {
                    while current_index < *field_num {
                        w.write(spaces(self.indent_level));
                        w.write(b"e.encode_none()\n");
                        current_index += 1;
                    }
                }
            }
            let field_name = self.to_field_name(field.id(), field.traits())?;
            let field_val = self.encode_shape_id(field.target(), ValExpr::Ref("val"))?;
            if is_optional_type(field) {
                w.write(spaces(self.indent_level));
                w.write(&format!(
                    "if not {}.{} is None:\n",
                    val.as_str(),
                    &field_name
                ));
                {
                    self.indent_level += 1;
                    if !as_array {
                        // map key is declared name, not target language name
                        w.write(spaces(self.indent_level));
                        w.write(&format!("e.encode_string(\"{}\")\n", field.id()));
                    }
                    w.write(spaces(self.indent_level));
                    w.write(&field_val);
                    self.indent_level -= 1;
                }
                w.write(spaces(self.indent_level));
                w.write(b"else:\n");
                w.write(spaces(self.indent_level + 1));
                w.write(b"e.encode_none()\n");
            } else {
                if !as_array {
                    w.write(spaces(self.indent_level));
                    // map key is declared name, not target language name
                    w.write(&format!("e.encode_string(\"{}\")\n", field.id()));
                }
                let val = format!("{}.{}", val.as_str(), &field_name);
                w.write(spaces(self.indent_level));
                w.write(&self.encode_shape_id(field.target(), ValExpr::Plain(&val))?);
            }
            current_index += 1;
        }
        Ok(())
    }

    pub(crate) fn declare_shape_encoder(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        kind: &ShapeKind,
    ) -> Result<()> {
        // The encoder is written as a plain function "encode_<S>" where S is the type name
        // (camel cased for the fn name), and scoped to the module where S is defined. This could
        // have been implemented as 'impl Encode for TYPE ...', but that would make the code more
        // rust-specific. This code is structured to be easier to port to other target languages.
        match kind {
            ShapeKind::Simple(_)
            | ShapeKind::Structure(_)
            | ShapeKind::Map(_)
            | ShapeKind::List(_)
            | ShapeKind::Set(_) => {
                let name = id.shape_name();
                w.write(spaces(self.indent_level));
                w.write(&format!("# encode {} as CBOR\n", &name));
                w.write(spaces(self.indent_level));
                w.write(&format!(
                    "def encode_{}(e, val: {}):\n",
                    crate::strings::to_snake_case(&name.to_string()),
                    self.to_type_name_case(&name.to_string())
                ));
                self.indent_level += 1;
                self.encode_shape_kind(w, id, kind, ValExpr::Ref("val"))?;
                w.write(b"\n\n");
                self.indent_level -= 1;
            }
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Union(_)
            | ShapeKind::Unresolved => { /* write nothing */ }
        }
        Ok(())
    }
}
