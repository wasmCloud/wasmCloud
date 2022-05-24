//! CBOR Decode functions

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
    codegen_rust::{is_optional_type, is_rust_primitive, Lifetime, RustCodeGen},
    error::{Error, Result},
    gen::CodeGen,
    model::{wasmcloud_model_namespace, Ty},
    writer::Writer,
};

// decodes byte slice of definite length; returns <&'b [u8]>
fn decode_blob() -> &'static str {
    "d.bytes()?.to_vec()"
}
fn decode_boolean() -> &'static str {
    "d.bool()?"
}
// decodes string - returns borrowed <&'bytes str>
fn decode_str() -> &'static str {
    "d.str()?.to_string()"
}
fn decode_byte() -> &'static str {
    "d.i8()?"
}
fn decode_unsigned_byte() -> &'static str {
    "d.u8()?"
}
fn decode_short() -> &'static str {
    "d.i16()?"
}
fn decode_unsigned_short() -> &'static str {
    "d.u16()?"
}
fn decode_integer() -> &'static str {
    "d.i32()?"
}
fn decode_unsigned_integer() -> &'static str {
    "d.u32()?"
}
fn decode_long() -> &'static str {
    "d.i64()?"
}
fn decode_unsigned_long() -> &'static str {
    "d.u64()?"
}
fn decode_float() -> &'static str {
    "d.f32()?"
}
fn decode_double() -> &'static str {
    "d.f64()?"
}
fn decode_timestamp() -> &'static str {
    "wasmbus_rpc::Timestamp{ sec: d.i64()?, nsec: d.u32()? }"
}
fn decode_big_integer() -> &'static str {
    todo!(); // tag big int
}
fn decode_big_decimal() -> &'static str {
    todo!() // tag big decimal
}
fn decode_document() -> &'static str {
    "wasmbus_rpc::common::decode_document(d)?"
}
fn decode_unit() -> &'static str {
    "d.null()?"
}

impl<'model> RustCodeGen<'model> {
    /// Generates cbor decode expressions "d.func()" for the id.
    /// If id is a primitive type, writes the direct decode function, otherwise,
    /// delegates to a decode_* function created in the same module where the symbol is defined
    pub(crate) fn decode_shape_id(&self, id: &ShapeID) -> Result<String> {
        let name = id.shape_name().to_string();
        let stmt = if id.namespace() == prelude_namespace_id() {
            match name.as_ref() {
                SHAPE_BLOB => decode_blob(),
                SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => decode_boolean(),
                SHAPE_STRING => decode_str(),
                SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => decode_byte(),
                SHAPE_SHORT | SHAPE_PRIMITIVESHORT => decode_short(),
                SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => decode_integer(),
                SHAPE_LONG | SHAPE_PRIMITIVELONG => decode_long(),
                SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => decode_float(),
                SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => decode_double(),
                SHAPE_TIMESTAMP => decode_timestamp(),
                SHAPE_BIGINTEGER => decode_big_integer(),
                SHAPE_BIGDECIMAL => decode_big_decimal(),
                SHAPE_DOCUMENT => decode_document(),
                _ => return Err(Error::UnsupportedType(name)),
            }
            .to_string()
        } else if id.namespace() == wasmcloud_model_namespace() {
            match name.as_bytes() {
                b"U64" => decode_unsigned_long().to_string(),
                b"U32" => decode_unsigned_integer().to_string(),
                b"U16" => decode_unsigned_short().to_string(),
                b"U8" => decode_unsigned_byte().to_string(),
                b"I64" => decode_long().to_string(),
                b"I32" => decode_integer().to_string(),
                b"I16" => decode_short().to_string(),
                b"I8" => decode_byte().to_string(),
                b"F64" => decode_double().to_string(),
                b"F32" => decode_float().to_string(),
                _ => format!(
                    "{}decode_{}(d)?",
                    self.get_model_crate(),
                    crate::strings::to_snake_case(&id.shape_name().to_string()),
                ),
            }
        } else {
            format!(
                "{}decode_{}(d).map_err(|e| format!(\"decoding '{}': {{}}\", e))?",
                self.get_crate_path(id)?,
                crate::strings::to_snake_case(&id.shape_name().to_string()),
                &id.to_string()
            )
        };
        Ok(stmt)
    }

    fn decode_shape_kind(&self, id: &ShapeID, kind: &ShapeKind) -> Result<String> {
        let s = match kind {
            ShapeKind::Simple(simple) => match simple {
                Simple::Blob => decode_blob(),
                Simple::Boolean => decode_boolean(),
                Simple::String => decode_str(),
                Simple::Byte => decode_byte(),
                Simple::Short => decode_short(),
                Simple::Integer => decode_integer(),
                Simple::Long => decode_long(),
                Simple::Float => decode_float(),
                Simple::Double => decode_double(),
                Simple::Timestamp => decode_timestamp(),
                Simple::BigInteger => decode_big_integer(),
                Simple::BigDecimal => decode_big_decimal(),
                Simple::Document => decode_document(),
            }
            .to_string(),
            ShapeKind::Map(map) => {
                format!(
                    r#"
                    {{
                        let map_len = d.fixed_map()? as usize;
                        let mut m: std::collections::HashMap<{},{}> = std::collections::HashMap::with_capacity(map_len);
                        for _ in 0..map_len {{
                            let k = {};
                            let v = {};
                            m.insert(k,v);
                        }}
                        m
                    }}
                    "#,
                    &self.type_string(Ty::Shape(map.key().target()), Lifetime::Any)?,
                    &self.type_string(Ty::Shape(map.value().target()), Lifetime::Any)?,
                    &self.decode_shape_id(map.key().target())?,
                    &self.decode_shape_id(map.value().target())?,
                )
            }
            ShapeKind::List(list) | ShapeKind::Set(list) => {
                let member_decoder = self.decode_shape_id(list.member().target())?;
                let member_type =
                    self.type_string(Ty::Shape(list.member().target()), Lifetime::Any)?;
                format!(
                    r#"
                    if let Some(n) = d.array()? {{
                        let mut arr : Vec<{}> = Vec::with_capacity(n as usize);
                        for _ in 0..(n as usize) {{
                            arr.push({})
                        }}
                        arr
                    }} else {{
                        // indefinite array
                        let mut arr : Vec<{}> = Vec::new();
                        loop {{
                            match d.datatype() {{
                                Err(_) => break,
                                Ok({}::cbor::Type::Break) => break,
                                Ok(_) => arr.push({})
                            }}
                        }}
                        arr
                    }}
                    "#,
                    &member_type, &member_decoder, &member_type, self.import_core, &member_decoder,
                )
            }
            ShapeKind::Structure(strukt) => {
                if id == crate::model::unit_shape() {
                    decode_unit().to_string()
                } else {
                    self.decode_struct(id, strukt)?
                }
            }
            ShapeKind::Union(union_) => self.decode_union(id, union_)?,
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => String::new(),
        };
        Ok(s)
    }

    fn decode_union(&self, id: &ShapeID, strukt: &StructureOrUnion) -> Result<String> {
        let (fields, _) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        let enum_name = id.shape_name();
        let mut s = format!(
            r#"
            // decoding union {}
            let len = d.fixed_array()?;
            if len != 2 {{ return Err(RpcError::Deser("decoding union '{}': expected 2-array".to_string())); }}
            match d.u16()? {{
        "#,
            enum_name, enum_name,
        );
        for field in fields.iter() {
            let field_num = field.field_num().unwrap();
            let target = field.target();
            let field_name = self.to_type_name_case(&field.id().to_string());
            if target == crate::model::unit_shape() {
                write!(
                    s,
                    r#"
                    {} => {{ 
                            {};
                            {}::{}
                          }},
                    "#,
                    &field_num,
                    &decode_unit(),
                    enum_name,
                    field_name
                )
                .unwrap();
            } else {
                let field_decoder = self.decode_shape_id(target)?;
                write!(
                    s,
                    r#"
                    {} => {{
                        let val = {};
                        {}::{}(val)
                    }},
                    "#,
                    &field_num, field_decoder, enum_name, field_name
                )
                .unwrap();
            }
        }
        writeln!(
            s,
            r#"
            n => {{ return Err(RpcError::Deser(format!("invalid field number for union '{}':{{}}", n))); }},
            }}"#,
            id
        ).unwrap();
        Ok(s)
    }

    /// write decode statements for a structure
    /// This always occurs inside a dedicated function for the struct type
    fn decode_struct(&self, id: &ShapeID, strukt: &StructureOrUnion) -> Result<String> {
        let (fields, _is_numbered) = crate::model::get_sorted_fields(id.shape_name(), strukt)?;
        let mut s = String::new();
        let lt = match self.has_lifetime(id) {
            true => Lifetime::L("v"),
            false => Lifetime::None,
        };
        for field in fields.iter() {
            let field_name = self.to_field_name(field.id(), field.traits())?;
            let field_type = self.field_type_string(field, lt)?;
            if is_optional_type(field) {
                // allows adding Optional fields at end of struct
                // and maintaining backwards compatibility to read structs
                // that did not define those fields.
                writeln!(
                    s,
                    "let mut {}: Option<{}> = Some(None);",
                    field_name, field_type
                )
                .unwrap()
            } else {
                writeln!(s, "let mut {}: Option<{}> = None;", field_name, field_type).unwrap()
            }
        }
        write!(s, r#"
            let is_array = match d.datatype()? {{
                {}::cbor::Type::Array => true,
                {}::cbor::Type::Map => false,
                _ => return Err(RpcError::Deser("decoding struct {}, expected array or map".to_string()))
            }};
            if is_array {{
                let len = d.fixed_array()?;
                for __i in 0..(len as usize) {{
        "#, self.import_core, self.import_core, id.shape_name() ).unwrap();
        if fields.is_empty() {
            s.push_str(
                r#"
                d.skip()?;
            "#,
            )
        } else {
            s.push_str(
                r#"
                   match __i {
            "#,
            )
        }
        // we aren't doing the encoder optimization that omits None values
        // at the end of the struct, but we can still decode it if used

        for (ix, field) in fields.iter().enumerate() {
            let field_name = self.to_field_name(field.id(), field.traits())?;
            let field_decoder = self.decode_shape_id(field.target())?;
            if is_optional_type(field) {
                write!(
                    s,
                    r#"{} => {} = if {}::cbor::Type::Null == d.datatype()? {{
                                        d.skip()?;
                                        Some(None)
                                    }} else {{
                                        Some(Some( {} ))
                                    }},
                   "#,
                    ix, field_name, self.import_core, field_decoder,
                )
                .unwrap();
            } else {
                write!(s, "{} => {} = Some({}),", ix, field_name, field_decoder,).unwrap();
            }
        }
        if !fields.is_empty() {
            // close match on array number
            s.push_str(
                r#"
                    _ => d.skip()?,
                    }
            "#,
            );
        }
        s.push_str(
            r#" 
                }
            } else {
                let len = d.fixed_map()?;
                for __i in 0..(len as usize) {
            "#,
        );
        if fields.is_empty() {
            // we think struct is empty (as of current definition),
            // but if len is non-zero, read each field name and skip val
            s.push_str(
                r#"
                    d.str()?; 
                    d.skip()?;
                    "#,
            );
        } else {
            s.push_str(
                r#"
                match d.str()? {
                "#,
            );
        }
        for field in fields.iter() {
            let field_name = self.to_field_name(field.id(), field.traits())?;
            let field_decoder = self.decode_shape_id(field.target())?;
            if is_optional_type(field) {
                write!(
                    s,
                    r#""{}" => {} = if {}::cbor::Type::Null == d.datatype()? {{
                                        d.skip()?;
                                        Some(None)
                                    }} else {{
                                        Some(Some( {} ))
                                    }},
                   "#,
                    field.id(),
                    field_name,
                    self.import_core,
                    field_decoder,
                )
                .unwrap();
            } else {
                write!(
                    s,
                    r#""{}" => {} = Some({}),"#,
                    field.id(),
                    field_name,
                    field_decoder,
                )
                .unwrap();
            }
        }
        if !fields.is_empty() {
            // close match
            s.push_str(
                r#"         _ => d.skip()?,
                    }
                    "#,
            );
        }
        s.push_str(
            r#"
                }
            }
            "#,
        );

        // build the return struct
        writeln!(s, "{} {{", id.shape_name()).unwrap();
        for (ix, field) in fields.iter().enumerate() {
            let field_name = self.to_field_name(field.id(), field.traits())?;
            if is_optional_type(field) {
                writeln!(s, "{}: {}.unwrap(),", &field_name, &field_name).unwrap();
            } else {
                write!(
                    s,
                    r#"
                {}: if let Some(__x) = {} {{
                    __x
                }} else {{
                    return Err(RpcError::Deser("missing field {}.{} (#{})".to_string()));
                }},
                "#,
                    &field_name,
                    &field_name,
                    id.shape_name(),
                    &field_name,
                    ix,
                )
                .unwrap();
            }
        }
        s.push_str("}\n"); // close struct initializer and Ok(...)
        Ok(s)
    }

    /// generate decode_* function for every declared type
    /// name of the function is decode_<S> where <S> is the camel_case type name
    /// It is generated in the module that declared type S, so it can always
    /// be found by prefixing the function name with the module path.
    pub(crate) fn declare_shape_decoder(
        &self,
        w: &mut Writer,
        id: &ShapeID,
        kind: &ShapeKind,
    ) -> Result<()> {
        let has_lifetime = self.has_lifetime(id);
        // since we don't have borrowing decoders yet, skip it if there are lifetimes
        if has_lifetime {
            return Ok(());
        }
        match kind {
            ShapeKind::Simple(_)
            | ShapeKind::Structure(_)
            | ShapeKind::Union(_)
            | ShapeKind::Map(_)
            | ShapeKind::List(_)
            | ShapeKind::Set(_) => {
                let name = id.shape_name();
                let is_rust_copy = is_rust_primitive(id);
                let mut s = format!(
                    r#"
                // Decode {} from cbor input stream
                #[doc(hidden)] {}
                pub fn decode_{}{}(d: &mut {}::cbor::Decoder{}) -> Result<{}{},RpcError>
                {{
                    let __result = {{ "#,
                    &name,
                    if is_rust_copy { "#[inline]" } else { "" },
                    crate::strings::to_snake_case(&name.to_string()),
                    if has_lifetime { "<'v>" } else { "" },
                    self.import_core,
                    if has_lifetime { "<'v>" } else { "<'_>" },
                    self.to_type_name_case(&id.shape_name().to_string()),
                    if has_lifetime { "<'v>" } else { "" },
                );
                let body = self.decode_shape_kind(id, kind)?;
                s.push_str(&body);
                s.push_str("};\n Ok(__result)\n}\n");
                w.write(s.as_bytes());
            }
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => { /* write nothing */ }
        }
        Ok(())
    }
}
