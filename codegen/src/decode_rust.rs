use crate::codegen_rust::{RustCodeGen, Ty};
use crate::{
    codegen_rust::is_optional_type,
    error::{Error, Result},
    gen::CodeGen,
    model::wasmcloud_model_namespace,
    writer::Writer,
};
use atelier_core::model::shapes::ShapeKind;
use atelier_core::{
    model::{
        shapes::{MemberShape, Simple, StructureOrUnion},
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
use std::string::ToString;

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
    todo!(); // tag timestamp
}
fn decode_big_integer() -> &'static str {
    todo!(); // tag big int
}
fn decode_big_decimal() -> &'static str {
    todo!() // tag big decimal
}
fn decode_document() -> &'static str {
    "d.bytes()?"
}

impl<'model> RustCodeGen<'model> {
    /// Generates cbor decode expressions "d.func()" for the id.
    /// If id is a primitive type, writes the direct decode function, otherwise,
    /// delegates to a decode_* function created in the same module where the symbol is defined
    fn decode_shape_id(&self, id: &ShapeID) -> Result<String> {
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
                _ => {
                    let mut s = String::new();
                    if self.namespace.is_none()
                        || self.namespace.as_ref().unwrap() != wasmcloud_model_namespace()
                    {
                        s.push_str(&self.import_core);
                        s.push_str("::model::");
                    }
                    s.push_str(&format!(
                        "decode_{}(d)?",
                        self.to_method_name(id.shape_name()),
                    ));
                    s
                }
            }
        } else if self.namespace.is_some() && id.namespace() == self.namespace.as_ref().unwrap() {
            format!("decode_{}(d)?", self.to_method_name(id.shape_name()),)
        } else {
            match self.packages.get(&id.namespace().to_string()) {
                Some(package) => {
                    let mut s = package.crate_name.clone();
                    s.push_str("::");
                    s.push_str(&format!(
                        "decode_{}(d)?",
                        self.to_method_name(id.shape_name()),
                    ));
                    s
                }
                None => {
                    return Err(Error::Model(format!("undefined create for namespace {} for symbol {}. Make sure codegen.toml includes all dependent namespaces",
                                                    &id.namespace(), &id)));
                }
            }
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
                Simple::Document => decode_blob(),
            }
            .to_string(),
            ShapeKind::Map(map) => {
                format!(
                    r#"
                    {{
                        let mut m: std::collections::HashMap<{},{}> = std::collections::HashMap::default();
                        if let Some(n) = d.map()? {{
                            for _ in 0..(n as usize) {{
                                let k = {};
                                let v = {};
                                m.insert(k,v);
                            }}
                        }} else {{
                            return Err(RpcError::Deser("indefinite maps not supported".to_string()));
                        }}
                        m
                    }}
                    "#,
                    &self.type_string(Ty::Shape(map.key().target()))?,
                    &self.type_string(Ty::Shape(map.value().target()))?,
                    &self.decode_shape_id(map.key().target())?,
                    &self.decode_shape_id(map.value().target())?,
                )
            }
            ShapeKind::List(list) | ShapeKind::Set(list) => {
                let member_decoder = self.decode_shape_id(list.member().target())?;
                let member_type = self.type_string(Ty::Shape(list.member().target()))?;
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
                                Ok(minicbor::data::Type::Break) => break,
                                Ok(_) => arr.push({})
                            }}
                        }}
                        arr
                    }}
                    "#,
                    &member_type, &member_decoder, &member_type, &member_decoder,
                )
            }
            ShapeKind::Structure(strukt) => self.decode_struct(id, strukt)?,
            ShapeKind::Operation(_)
            | ShapeKind::Resource(_)
            | ShapeKind::Service(_)
            | ShapeKind::Unresolved => String::new(),

            ShapeKind::Union(_) => {
                unimplemented!();
            }
        };
        Ok(s)
    }

    /// write decode statements for a structure
    /// This always occurs inside a dedicated function for the struct type
    fn decode_struct(&self, id: &ShapeID, strukt: &StructureOrUnion) -> Result<String> {
        let mut fields = strukt
            .members()
            .map(|m| m.to_owned())
            .collect::<Vec<MemberShape>>();
        // FIXME: want to do field annotations with field numbers [n]
        fields.sort_by_key(|f| f.id().to_owned());
        // todo: lifetime constraints
        let mut s = String::new();
        for field in fields.iter() {
            let field_name = self.to_field_name(field.id())?;
            let field_type = self.field_type_string(field)?;
            if is_optional_type(field) {
                // allows adding Optional fields at end of struct
                // and maintaining backwards compatibility to read structs
                // that did not define those fields.
                s.push_str(&format!(
                    "let mut {}: Option<{}> = Some(None);\n",
                    field_name, field_type
                ))
            } else {
                s.push_str(&format!(
                    "let mut {}: Option<{}> = None;\n",
                    field_name, field_type
                ))
            }
        }
        // we aren't doing the encoder optimization that omits None values
        // at the end of the struct, but we can still decode it if used
        s.push_str(&format!(
            r#"
            if let Some(len) = d.array()? {{
                for __i in 0..(len as usize) {{
                    match __i {{
                    "#
        ));
        for (ix, field) in fields.iter().enumerate() {
            let field_name = self.to_field_name(field.id())?;
            let field_decoder = self.decode_shape_id(field.target())?;
            if is_optional_type(field) {
                s.push_str(&format!(
                    r#"{} => {} = if minicbor::data::Type::Null == d.datatype()? {{
                                        d.skip()?;
                                        Some(None)
                                    }} else {{
                                        Some(Some( {} ))
                                    }},
                   "#,
                    ix, field_name, field_decoder,
                ));
            } else {
                s.push_str(&format!(
                    "{} => {} = Some({}),\n",
                    ix, field_name, field_decoder,
                ));
            }
        }
        s.push_str(&format!(
            r#"         _ => d.skip()?,
                    }}
                }}
            }} else {{
                return Err(RpcError::Deser("{}: indefinite arrays in struct are not supported".to_string()));
            }}
            "#,
            id.shape_name())
        );
        // build the return struct
        s.push_str(&format!("{} {{\n", id.shape_name()));
        for (ix, field) in fields.iter().enumerate() {
            let field_name = self.to_field_name(field.id())?;
            s.push_str(&format!(
                r#"
                {}: if let Some(__x) = {} {{
                    __x 
                }} else {{
                    return Err(RpcError::Deser("missing field {}::{} (#{})".to_string()));
                }},
                "#,
                &field_name,
                &field_name,
                id.shape_name(),
                &field_name,
                ix,
            ));
        }
        s.push_str("}\n"); // close struct initializer and Ok(...)
        Ok(s)
    }

    /// generate decode_* function for every declared type
    /// name of the function is encode_<S> where <S> is the camel_case type name
    /// It is generated in the module that declared type S, so it can always
    /// be found by prefixing the function name with the module path.
    pub(crate) fn declare_shape_decoder(
        &self,
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
                let mut s = format!(
                    r#"
                #[doc(hidden)]
                pub fn decode_{}<'b>(d: &mut minicbor::Decoder<'b>) -> Result<{},RpcError>
                {{
                    let __result = {{ "#,
                    self.to_method_name(name),
                    &id.shape_name()
                );
                let body = self.decode_shape_kind(id, kind)?;
                s.push_str(&body);
                s.push_str("};\n Ok(__result)\n}\n");
                w.write(s.as_bytes());
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
