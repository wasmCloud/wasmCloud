//! CBOR Decode functions

use std::{fmt::Write as _, string::ToString};

use atelier_core::{
    model::{
        shapes::{HasTraits, ShapeKind, StructureOrUnion},
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

fn decode_type() -> &'static str {
    "d.decode()"
}

impl<'model> PythonCodeGen<'model> {
    /// Generates cbor decode expressions "d.func()" for the id.
    /// If id is a primitive type, writes the direct decode function, otherwise,
    /// delegates to a decode_* function created in the same module where the symbol is defined
    pub(crate) fn decode_shape_id(&self, id: &ShapeID) -> Result<String> {
        let name = id.shape_name().to_string();
        let stmt = if id.namespace() == prelude_namespace_id() {
            match name.as_ref() {
                SHAPE_BLOB
                | SHAPE_BIGDECIMAL
                | SHAPE_BIGINTEGER
                | SHAPE_BOOLEAN
                | SHAPE_BYTE
                | SHAPE_DOCUMENT
                | SHAPE_DOUBLE
                | SHAPE_FLOAT
                | SHAPE_INTEGER
                | SHAPE_LONG
                | SHAPE_PRIMITIVEBOOLEAN
                | SHAPE_PRIMITIVEBYTE
                | SHAPE_PRIMITIVEDOUBLE
                | SHAPE_PRIMITIVEFLOAT
                | SHAPE_PRIMITIVEINTEGER
                | SHAPE_PRIMITIVELONG
                | SHAPE_PRIMITIVESHORT
                | SHAPE_SHORT
                | SHAPE_STRING
                | SHAPE_TIMESTAMP => decode_type(),
                _ => return Err(Error::UnsupportedType(name)),
            }
            .to_string()
        } else if id.namespace() == wasmcloud_model_namespace() {
            match name.as_bytes() {
                b"U64" | b"U32" | b"U16" | b"U8" | b"I64" | b"I32" | b"I16" | b"I8" => {
                    decode_type().to_string()
                }
                _ => {
                    let mut s = String::new();
                    if self.namespace.is_none()
                        || self.namespace.as_ref().unwrap() != wasmcloud_model_namespace()
                    {
                        s.push_str(&self.import_core);
                        s.push('.');
                    }
                    write!(
                        s,
                        "decode_{}(d)",
                        crate::strings::to_snake_case(&id.shape_name().to_string()),
                    )
                    .unwrap();
                    s
                }
            }
        } else if self.namespace.is_some() && id.namespace() == self.namespace.as_ref().unwrap() {
            format!(
                "decode_{}(d)",
                crate::strings::to_snake_case(&id.shape_name().to_string()),
            )
        } else {
            match self.packages.get(&id.namespace().to_string()) {
                Some(crate::model::PackageName { py_module: Some(py_module), .. }) => {
                    // the crate name should be valid rust syntax. If not, they'll get an error with rustc
                    format!(
                        "{}.decode_{}(d)",
                        &py_module,
                        crate::strings::to_snake_case(&id.shape_name().to_string())
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

    fn decode_shape_kind(&mut self, w: &mut Writer, id: &ShapeID, kind: &ShapeKind) -> Result<()> {
        match kind {
            ShapeKind::Simple(_) => {
                w.write(spaces(self.indent_level));
                w.write("return ");
                w.write(decode_type());
                w.write(b"\n");
            }
            ShapeKind::Map(map) => {
                w.write(spaces(self.indent_level));
                w.write(b"(kind,num) = decode_map_or_array(d)\n");
                w.write(spaces(self.indent_level));
                w.write(b"if kind != 5:");
                w.write(spaces(self.indent_level + 1));
                w.write(b"raise Exception(\"Decode error: expected map, got {}\".format(kind))\n");
                w.write(spaces(self.indent_level));
                w.write(b"if num is None:");
                w.write(spaces(self.indent_level + 1));
                w.write(
                    b"raise Exception(\"Decode error: indefinite length maps not supported\")\n",
                );
                w.write(spaces(self.indent_level));
                w.write(b"map = {}\n");
                w.write(spaces(self.indent_level));
                w.write(b"for _ in range(0,num):\n");
                {
                    self.indent_level += 1;
                    w.write(spaces(self.indent_level));
                    w.write(&format!(
                        "k = {}\n",
                        &self.decode_shape_id(map.key().target())?
                    ));
                    w.write(spaces(self.indent_level));
                    w.write(&format!(
                        "v = {}\n",
                        &self.decode_shape_id(map.value().target())?
                    ));
                    w.write(spaces(self.indent_level));
                    w.write(b"map[k] = v\n");
                    self.indent_level -= 1;
                }
                w.write(spaces(self.indent_level));
                w.write("return map\n");
            }
            ShapeKind::List(list) | ShapeKind::Set(list) => {
                w.write(spaces(self.indent_level));
                w.write(b"(kind,num) = decode_map_or_array(d)\n");
                w.write(spaces(self.indent_level));
                w.write(b"if kind != 4:");
                w.write(spaces(self.indent_level + 1));
                w.write(
                    b"raise Exception(\"Decode error: expected array, got {}\".format(kind))\n",
                );
                w.write(spaces(self.indent_level));
                w.write(b"if num is None:");
                w.write(spaces(self.indent_level + 1));
                w.write(
                    b"raise Exception(\"Decode error: indefinite length array not supported\")\n",
                );
                w.write(spaces(self.indent_level));
                w.write(b"items = []\n");
                w.write(spaces(self.indent_level));
                w.write(b"for _ in range(0,num):\n");
                {
                    self.indent_level += 1;
                    w.write(spaces(self.indent_level));
                    w.write(&format!(
                        "item = {}\n",
                        &self.decode_shape_id(list.member().target())?
                    ));
                    w.write(spaces(self.indent_level));
                    w.write(b"items.append(item)\n");
                    self.indent_level -= 1;
                }
                w.write(spaces(self.indent_level));
                w.write("return items\n");
            }
            ShapeKind::Structure(strukt) => {
                self.decode_struct(w, id, strukt)?;
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

    /// write decode statements for a structure
    /// This always occurs inside a dedicated function for the struct type
    fn decode_struct(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let (fields, _is_numbered) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        for field in fields.iter() {
            let field_name = self.to_field_name(field.id(), field.traits())?;
            //let field_type = self.field_type_string(field)?;
            w.write(spaces(self.indent_level));
            w.write(&format!("{field_name} = None\n"));
        }
        //if is_optional_type(field) {
        w.write(spaces(self.indent_level));
        w.write(b"(__kind,__num) = decode_map_or_array(d)\n");
        w.write(spaces(self.indent_level));
        w.write(b"if __kind == 4:\n");
        {
            // array
            self.indent_level += 1;
            w.write(spaces(self.indent_level));
            w.write(b"for __i in range(0,__num):\n");
            {
                self.indent_level += 1;
                for (ix, field) in fields.iter().enumerate() {
                    let field_name = self.to_field_name(field.id(), field.traits())?;
                    let field_decoder = self.decode_shape_id(field.target())?;
                    w.write(spaces(self.indent_level));
                    if ix != 0 {
                        w.write(b"el"); // 'elif'
                    }
                    w.write(&format!("if __i == {ix}:\n"));
                    {
                        w.write(spaces(self.indent_level + 1));
                        w.write(&format!("{field_name} = {field_decoder}\n"));
                    }
                }
                // if no fields, generate skip for every item
                w.write(spaces(self.indent_level));
                if !fields.is_empty() {
                    // after chain of if/elif .., catch extra values
                    w.write(b"else:\n");
                    // note extra indent here before falling through to decode
                    w.write(spaces(self.indent_level + 1));
                }
                w.write(b"d.decode() #skip\n");

                self.indent_level -= 1; // end field counter
            }
            self.indent_level -= 1; // end array items
        }
        w.write(spaces(self.indent_level));
        w.write(b"elif __kind == 5:\n");
        {
            // map
            self.indent_level += 1;
            w.write(spaces(self.indent_level));
            w.write(b"for __i in range(0,__num):\n");
            {
                self.indent_level += 1;
                w.write(spaces(self.indent_level));
                w.write(b"__key = d.decode()\n"); // read key
                for (ix, field) in fields.iter().enumerate() {
                    let field_name = self.to_field_name(field.id(), field.traits())?;
                    let field_decoder = self.decode_shape_id(field.target())?;
                    w.write(spaces(self.indent_level));
                    if ix != 0 {
                        w.write(b"el"); // 'elif'
                    }
                    w.write(&format!("if __key == \"{}\":\n", field.id()));
                    {
                        w.write(spaces(self.indent_level + 1));
                        w.write(&format!("{field_name} = {field_decoder}\n"));
                    }
                }
                w.write(spaces(self.indent_level));
                if fields.is_empty() {
                    // we think struct is empty but array is not empty,
                    // so generate skip for each item
                    w.write(b"d.decode() #skip key\n");
                    w.write(spaces(self.indent_level));
                } else {
                    // after chain of if/elif .., catch extra values
                    w.write(b"else:\n");
                    w.write(spaces(self.indent_level + 1));
                }
                w.write(b"d.decode() #skip val\n");
                self.indent_level -= 1; // end field counter
            }
            self.indent_level -= 1; // end map items
        }
        // check whether all the non-optional fields have values
        for field in fields.iter() {
            if !is_optional_type(field) {
                let field_name = self.to_field_name(field.id(), field.traits())?;
                w.write(spaces(self.indent_level));
                w.write(&format!("if {} is None:\n", &field_name));
                w.write(spaces(self.indent_level + 1));
                w.write(&format!(
                    "raise Exception(\"missing field {}.{}\")\n",
                    id.shape_name(),
                    field_name
                ));
            }
        }
        let init_fields = fields
            .iter()
            .map(|f| self.to_field_name(f.id(), f.traits()))
            .collect::<Result<Vec<String>>>()?
            .join(",");
        w.write(spaces(self.indent_level));
        w.write(&format!("return {}({})\n", id.shape_name(), init_fields,));
        Ok(())
    }

    /// generate decode_* function for every declared type
    /// name of the function is encode_<S> where <S> is the camel_case type name
    /// It is generated in the module that declared type S, so it can always
    /// be found by prefixing the function name with the module path.
    pub(crate) fn declare_shape_decoder(
        &mut self,
        w: &mut Writer,
        id: &ShapeID,
        kind: &ShapeKind,
    ) -> Result<()> {
        match kind {
            ShapeKind::Simple(_)
            | ShapeKind::Structure(_)
            | ShapeKind::Map(_)
            | ShapeKind::List(_)
            | ShapeKind::Set(_) => {
                let name = id.shape_name();
                w.write(spaces(self.indent_level));
                w.write(&format!("# decode {} as CBOR\n", &name));
                w.write(spaces(self.indent_level));
                w.write(&format!(
                    "def decode_{}(d) -> {}:\n",
                    crate::strings::to_snake_case(&name.to_string()),
                    self.to_type_name_case(&name.to_string())
                ));
                self.indent_level += 1;
                self.decode_shape_kind(w, id, kind)?;
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
