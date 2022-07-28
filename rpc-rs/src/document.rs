use crate::error::{RpcError, RpcResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Document Type
///
/// Document types represents protocol-agnostic open content that is accessed like JSON data.
/// Open content is useful for modeling unstructured data that has no schema, data that can't be
/// modeled using rigid types, or data that has a schema that evolves outside of the purview of a model.
/// The serialization format of a document is an implementation detail of a protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Document {
    /// object
    // n(0)
    Object(HashMap<String, Document>),
    /// Blob
    // n(1)
    Blob(Vec<u8>),
    /// array
    // n(2)
    Array(Vec<Document>),
    /// number
    // n(3)
    Number(Number),
    /// UTF8 String
    // n(4)
    String(String),
    /// boolean
    // n(5)
    Bool(bool),
    /// null
    // n(6)
    Null,
}

impl Default for Document {
    fn default() -> Self {
        Document::Null
    }
}

/// Borrowed Document Type
///
/// Document types represents protocol-agnostic open content that is accessed like JSON data.
/// Open content is useful for modeling unstructured data that has no schema, data that can't be
/// modeled using rigid types, or data that has a schema that evolves outside of the purview of a model.
/// The serialization format of a document is an implementation detail of a protocol.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum DocumentRef<'v> {
    /// object
    // n(0)
    Object(HashMap<String, DocumentRef<'v>>),
    /// Blob
    // n(1)
    Blob(&'v [u8]),
    /// array
    // n(2)
    Array(Vec<DocumentRef<'v>>),
    /// number
    // n(3)
    Number(Number),
    /// UTF8 String
    // n(4)
    String(&'v str),
    /// boolean
    // n(5)
    Bool(bool),
    /// null
    // n(6)
    Null,
}

impl Default for DocumentRef<'_> {
    fn default() -> Self {
        DocumentRef::Null
    }
}

impl<'v> DocumentRef<'v> {
    pub fn to_owned(&self) -> Document {
        match self {
            DocumentRef::Object(o) => {
                Document::Object(o.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect())
            }
            DocumentRef::Blob(b) => Document::Blob(b.to_vec()),
            DocumentRef::Array(a) => Document::Array(a.iter().map(|v| v.to_owned()).collect()),
            DocumentRef::Number(n) => Document::Number(*n),
            DocumentRef::String(s) => Document::String((*s).into()),
            DocumentRef::Bool(b) => Document::Bool(*b),
            DocumentRef::Null => Document::Null,
        }
    }
}

impl Document {
    pub fn as_ref(&self) -> DocumentRef<'_> {
        match self {
            Document::Object(o) => {
                DocumentRef::Object(o.iter().map(|(k, v)| (k.to_string(), v.as_ref())).collect())
            }
            Document::Blob(b) => DocumentRef::Blob(b.as_ref()),
            Document::Array(a) => DocumentRef::Array(a.iter().map(|v| v.as_ref()).collect()),
            Document::Number(n) => DocumentRef::Number(*n),
            Document::String(s) => DocumentRef::String(s.as_str()),
            Document::Bool(b) => DocumentRef::Bool(*b),
            Document::Null => DocumentRef::Null,
        }
    }
}

impl Document {
    /// Returns the map, if Document is an Object, otherwise None
    pub fn to_object(self) -> Option<HashMap<String, Document>> {
        if let Document::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns reference to the map, if Document is an Object, otherwise None
    pub fn as_object(&self) -> Option<&HashMap<String, Document>> {
        if let Document::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an object
    pub fn is_object(&self) -> bool {
        self.as_object().is_some()
    }

    /// Returns the blob, if Document is an Blob, otherwise None
    pub fn as_blob(&self) -> Option<&[u8]> {
        if let Document::Blob(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns the blob, if Document is an Blob, otherwise None
    pub fn to_blob(self) -> Option<Vec<u8>> {
        if let Document::Blob(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is a blob (byte array)
    pub fn is_blob(&self) -> bool {
        self.as_blob().is_some()
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn as_array(&self) -> Option<&Vec<Document>> {
        if let Document::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn to_array(self) -> Option<Vec<Document>> {
        if let Document::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an array
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }

    /// Returns the Number, if Document is a Number, otherwise None
    pub fn as_number(&self) -> Option<Number> {
        if let Document::Number(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_number(self) -> Option<Number> {
        self.as_number()
    }

    /// returns true if the Document is a number (signed/unsigned int or float)
    pub fn is_number(&self) -> bool {
        self.as_number().is_some()
    }

    /// Returns the positive int, if Document is a positive int, otherwise None
    pub fn as_pos_int(&self) -> Option<u64> {
        if let Document::Number(Number::PosInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_pos_int(self) -> Option<u64> {
        self.as_pos_int()
    }

    /// returns true if the Document is a positive integer
    pub fn is_pos_int(&self) -> bool {
        self.as_pos_int().is_some()
    }

    /// Returns the signed int, if Document is a signed int, otherwise None
    pub fn as_int(&self) -> Option<i64> {
        if let Document::Number(Number::NegInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_int(self) -> Option<i64> {
        self.as_int()
    }

    /// returns true if the Document is a signed integer
    pub fn is_int(&self) -> bool {
        self.as_int().is_some()
    }

    /// Returns the float value, if Document is a float value, otherwise None
    pub fn as_float(&self) -> Option<f64> {
        if let Document::Number(Number::Float(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_float(self) -> Option<f64> {
        self.as_float()
    }

    /// returns true if the Document is a float
    pub fn is_float(&self) -> bool {
        self.as_float().is_some()
    }

    /// Returns the bool value, if Document is a bool value, otherwise None
    pub fn as_bool(&self) -> Option<bool> {
        if let Document::Bool(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_bool(self) -> Option<bool> {
        self.as_bool()
    }

    /// returns true if the Document is a boolean
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// Returns borrowed str, if Document is a string value, otherwise None
    pub fn as_str(&self) -> Option<&str> {
        if let Document::String(val) = self {
            Some(val.as_ref())
        } else {
            None
        }
    }

    /// returns owned String, if Document is a String value
    pub fn to_string(self) -> Option<String> {
        if let Document::String(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is a string type
    pub fn is_str(&self) -> bool {
        self.as_str().is_some()
    }

    /// returns true if the Document is null
    pub fn is_null(&self) -> bool {
        matches!(self, Document::Null)
    }

    pub fn from_null() -> Self {
        Document::Null
    }
}

impl<'v> DocumentRef<'v> {
    /// Returns the map, if Document is an Object, otherwise None
    pub fn to_object(self) -> Option<HashMap<String, DocumentRef<'v>>> {
        if let DocumentRef::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns reference to the map, if Document is an Object, otherwise None
    pub fn as_object(&self) -> Option<&HashMap<String, DocumentRef<'v>>> {
        if let DocumentRef::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an object
    pub fn is_object(&self) -> bool {
        self.as_object().is_some()
    }

    /// Returns the blob, if Document is an Blob, otherwise None
    pub fn as_blob(&self) -> Option<&'v [u8]> {
        if let DocumentRef::Blob(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is a blob (byte array)
    pub fn is_blob(&self) -> bool {
        self.as_blob().is_some()
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn as_array(&self) -> Option<&Vec<DocumentRef<'v>>> {
        if let DocumentRef::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn to_array(self) -> Option<Vec<DocumentRef<'v>>> {
        if let DocumentRef::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an array
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }

    /// Returns the Number, if Document is a Number, otherwise None
    pub fn as_number(&self) -> Option<Number> {
        if let DocumentRef::Number(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_number(self) -> Option<Number> {
        self.as_number()
    }

    /// returns true if the Document is a number (signed/unsigned int or float)
    pub fn is_number(&self) -> bool {
        self.as_number().is_some()
    }

    /// Returns the positive int, if Document is a positive int, otherwise None
    pub fn as_pos_int(&self) -> Option<u64> {
        if let DocumentRef::Number(Number::PosInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_pos_int(self) -> Option<u64> {
        self.as_pos_int()
    }

    /// returns true if the Document is a positive integer
    pub fn is_pos_int(&self) -> bool {
        self.as_pos_int().is_some()
    }

    /// Returns the signed int, if Document is a signed int, otherwise None
    pub fn as_int(&self) -> Option<i64> {
        if let DocumentRef::Number(Number::NegInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_int(self) -> Option<i64> {
        self.as_int()
    }

    /// returns true if the Document is a signed integer
    pub fn is_int(&self) -> bool {
        self.as_int().is_some()
    }

    /// Returns the float value, if Document is a float value, otherwise None
    pub fn as_float(&self) -> Option<f64> {
        if let DocumentRef::Number(Number::Float(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_float(self) -> Option<f64> {
        self.as_float()
    }

    /// returns true if the Document is a float
    pub fn is_float(&self) -> bool {
        self.as_float().is_some()
    }

    /// Returns the bool value, if Document is a bool value, otherwise None
    pub fn as_bool(&self) -> Option<bool> {
        if let DocumentRef::Bool(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_bool(self) -> Option<bool> {
        self.as_bool()
    }

    /// returns true if the Document is a boolean
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// Returns borrowed str, if Document is a string value, otherwise None
    pub fn as_str(&self) -> Option<&str> {
        if let DocumentRef::String(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns owned String, if Document is a String value
    pub fn to_string(self) -> Option<String> {
        if let DocumentRef::String(val) = self {
            Some(val.to_string())
        } else {
            None
        }
    }

    /// returns true if the Document is a string type
    pub fn is_str(&self) -> bool {
        self.as_str().is_some()
    }

    /// returns true if the Document is null
    pub fn is_null(&self) -> bool {
        matches!(self, DocumentRef::Null)
    }

    pub fn from_null() -> Self {
        DocumentRef::Null
    }
}

/// A number type that implements Javascript / JSON semantics, modeled on serde_json:
/// <https://docs.serde.rs/src/serde_json/number.rs.html#20-22>
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Number {
    /// Unsigned 64-bit integer value
    // n(0)
    PosInt(u64),
    /// Signed 64-bit integer value
    // n(1)
    NegInt(i64),
    /// 64-bit floating-point value
    // n(2)
    Float(f64),
}

macro_rules! to_num_fn {
    ($name:ident, $typ:ident) => {
        /// Converts to a `$typ`. This conversion may be lossy.
        impl Number {
            pub fn $name(self) -> $typ {
                match self {
                    Number::PosInt(val) => val as $typ,
                    Number::NegInt(val) => val as $typ,
                    Number::Float(val) => val as $typ,
                }
            }
        }
    };
}

macro_rules! from_num_fn {
    ($typ:ident, $nt:ident) => {
        impl From<$typ> for Document {
            fn from(val: $typ) -> Self {
                Document::Number(Number::$nt(val.into()))
            }
        }
        impl From<$typ> for DocumentRef<'_> {
            fn from(val: $typ) -> Self {
                DocumentRef::Number(Number::$nt(val.into()))
            }
        }
    };
}

macro_rules! impl_try_from {
    ($t:ty, $p:ident) => {
        impl TryFrom<Document> for $t {
            type Error = Document;

            fn try_from(val: Document) -> Result<$t, Self::Error> {
                match val {
                    Document::$p(v) => Ok(v),
                    v => Err(v),
                }
            }
        }
    };
}

macro_rules! impl_try_from_num {
    ($t:ty, $n:ident) => {
        impl TryFrom<Document> for $t {
            type Error = Document;

            fn try_from(val: Document) -> Result<$t, Self::Error> {
                match val {
                    Document::Number(Number::$n(v)) => Ok(v),
                    v => Err(v),
                }
            }
        }
    };
}

macro_rules! impl_try_from_ref {
    ($t:ty, $p:ident) => {
        impl<'v> TryFrom<DocumentRef<'v>> for $t {
            type Error = DocumentRef<'v>;

            fn try_from(val: DocumentRef<'v>) -> Result<$t, Self::Error> {
                match val {
                    DocumentRef::$p(v) => Ok(v),
                    v => Err(v),
                }
            }
        }
    };
}

macro_rules! impl_try_from_ref_num {
    ($t:ty, $n:ident) => {
        impl<'v> TryFrom<DocumentRef<'v>> for $t {
            type Error = DocumentRef<'v>;

            fn try_from(val: DocumentRef<'v>) -> Result<$t, Self::Error> {
                match val {
                    DocumentRef::Number(Number::$n(v)) => Ok(v),
                    v => Err(v),
                }
            }
        }
    };
}

macro_rules! impl_from {
    ($t:ty, $p:ident) => {
        impl From<$t> for Document {
            fn from(val: $t) -> Self {
                Document::$p(val)
            }
        }
    };
}
macro_rules! impl_from_ref {
    ($t:ty, $p:ident) => {
        impl<'v> From<$t> for DocumentRef<'v> {
            fn from(val: $t) -> Self {
                DocumentRef::$p(val)
            }
        }
    };
}

to_num_fn!(to_f32, f32);
to_num_fn!(to_f64, f64);
to_num_fn!(to_i8, i8);
to_num_fn!(to_i16, i16);
to_num_fn!(to_i32, i32);
to_num_fn!(to_i64, i64);
to_num_fn!(to_u8, u8);
to_num_fn!(to_u16, u16);
to_num_fn!(to_u32, u32);
to_num_fn!(to_u64, u64);

from_num_fn!(u64, PosInt);
from_num_fn!(u32, PosInt);
from_num_fn!(u16, PosInt);
from_num_fn!(u8, PosInt);
from_num_fn!(i64, NegInt);
from_num_fn!(i32, NegInt);
from_num_fn!(i16, NegInt);
from_num_fn!(i8, NegInt);
from_num_fn!(f64, Float);
from_num_fn!(f32, Float);

impl_try_from!(HashMap<String,Document>, Object);
impl_try_from!(Vec<u8>, Blob);
impl_try_from!(Vec<Document>, Array);
impl_try_from!(String, String);
impl_try_from!(bool, Bool);

impl_try_from_num!(u64, PosInt);
impl_try_from_num!(i64, NegInt);
impl_try_from_num!(f64, Float);

impl_try_from_ref!(HashMap<String,DocumentRef<'v>>, Object);
impl_try_from_ref!(&'v [u8], Blob);
impl_try_from_ref!(Vec<DocumentRef<'v>>, Array);
impl_try_from_ref!(&'v str, String);
impl_try_from_ref!(bool, Bool);

impl_try_from_ref_num!(u64, PosInt);
impl_try_from_ref_num!(i64, NegInt);
impl_try_from_ref_num!(f64, Float);

impl_from!(HashMap<String,Document>, Object);
impl_from!(Vec<u8>, Blob);
impl_from!(Vec<Document>, Array);
impl_from!(String, String);
impl_from!(bool, Bool);

impl_from_ref!(HashMap<String,DocumentRef<'v>>, Object);
impl_from_ref!(&'v [u8], Blob);
impl_from_ref!(Vec<DocumentRef<'v>>, Array);
impl_from_ref!(&'v str, String);
impl_from_ref!(bool, Bool);

impl FromIterator<(String, Document)> for Document {
    fn from_iter<I: IntoIterator<Item = (String, Document)>>(iter: I) -> Self {
        let o: HashMap<String, Document> = iter.into_iter().collect();
        Document::Object(o)
    }
}

impl FromIterator<Document> for Document {
    fn from_iter<I: IntoIterator<Item = Document>>(iter: I) -> Self {
        let a: Vec<Document> = iter.into_iter().collect();
        Document::Array(a)
    }
}

/// Encode Document as cbor
#[doc(hidden)]
pub fn encode_document<W: crate::cbor::Write>(
    e: &mut crate::cbor::Encoder<W>,
    val: &Document,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.array(2)?;
    match val {
        Document::Object(map) => {
            e.u8(0)?;
            e.map(map.len() as u64)?;
            for (k, v) in map.iter() {
                e.str(k.as_str())?;
                encode_document(e, v)?;
            }
        }
        Document::Blob(blob) => {
            e.u8(1)?;
            e.bytes(blob)?;
        }
        Document::Array(vec) => {
            e.u8(2)?;
            e.array(vec.len() as u64)?;
            for v in vec.iter() {
                encode_document(e, v)?;
            }
        }
        Document::Number(n) => {
            e.u8(3)?;
            encode_number(e, n)?;
        }
        Document::String(s) => {
            e.u8(4)?;
            e.str(s)?;
        }
        Document::Bool(b) => {
            e.u8(5)?;
            e.bool(*b)?;
        }
        Document::Null => {
            e.u8(6)?;
            e.null()?;
        }
    }
    Ok(())
}
/// Encode DocumentRef as cbor
#[doc(hidden)]
pub fn encode_document_ref<'v, W: crate::cbor::Write>(
    e: &mut crate::cbor::Encoder<W>,
    val: &DocumentRef<'v>,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.array(2)?;
    match val {
        DocumentRef::Object(map) => {
            e.u8(0)?;
            e.map(map.len() as u64)?;
            for (k, v) in map.iter() {
                e.str(k.as_str())?;
                encode_document_ref(e, v)?;
            }
        }
        DocumentRef::Blob(blob) => {
            e.u8(1)?;
            e.bytes(blob)?;
        }
        DocumentRef::Array(vec) => {
            e.u8(2)?;
            e.array(vec.len() as u64)?;
            for v in vec.iter() {
                encode_document_ref(e, v)?;
            }
        }
        DocumentRef::Number(n) => {
            e.u8(3)?;
            encode_number(e, n)?;
        }
        DocumentRef::String(s) => {
            e.u8(4)?;
            e.str(s)?;
        }
        DocumentRef::Bool(b) => {
            e.u8(5)?;
            e.bool(*b)?;
        }
        DocumentRef::Null => {
            e.u8(6)?;
            e.null()?;
        }
    }
    Ok(())
}

/// Encode Number as cbor
#[doc(hidden)]
pub fn encode_number<W: crate::cbor::Write>(
    e: &mut crate::cbor::Encoder<W>,
    val: &Number,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.array(2)?;
    match val {
        Number::PosInt(val) => {
            e.u8(0)?;
            e.u64(*val)?;
        }
        Number::NegInt(val) => {
            e.u8(1)?;
            e.i64(*val)?;
        }
        Number::Float(val) => {
            e.u8(2)?;
            e.f64(*val)?;
        }
    }
    Ok(())
}

#[doc(hidden)]
pub fn decode_document(d: &mut crate::cbor::Decoder<'_>) -> RpcResult<Document> {
    let len = d.fixed_array()?;
    if len != 2 {
        return Err(RpcError::Deser("invalid Document".to_string()));
    }
    match d.u8()? {
        0 => {
            // Object
            let map_len = d.fixed_map()? as usize;
            let mut map = HashMap::with_capacity(map_len);
            for _ in 0..map_len {
                let k = d.str()?.to_string();
                let v = decode_document(d)?;
                map.insert(k, v);
            }
            Ok(Document::Object(map))
        }
        1 => {
            // Blob
            Ok(Document::Blob(d.bytes()?.to_vec()))
        }
        2 => {
            // Array
            let arr_len = d.fixed_array()? as usize;
            let mut arr = Vec::with_capacity(arr_len);
            for _ in 0..arr_len {
                arr.push(decode_document(d)?);
            }
            Ok(Document::Array(arr))
        }
        3 => {
            // Number
            Ok(Document::Number(decode_number(d)?))
        }
        4 => {
            // String
            Ok(Document::String(d.str()?.into()))
        }
        5 => {
            // Bool
            Ok(Document::Bool(d.bool()?))
        }
        6 => {
            // Null
            d.null()?;
            Ok(Document::Null)
        }
        _ => Err(RpcError::Deser("invalid Document field".to_string())),
    }
}

impl<'b> crate::cbor::Decode<'b> for Document {
    fn decode(d: &mut crate::cbor::Decoder<'b>) -> Result<Self, RpcError> {
        decode_document(d)
    }
}

#[doc(hidden)]
pub fn decode_number(d: &mut crate::cbor::Decoder) -> RpcResult<Number> {
    let len = d.fixed_array()?;
    if len != 2 {
        return Err(RpcError::Deser("invalid Number".into()));
    }
    match d.u8()? {
        0 => Ok(Number::PosInt(d.u64()?)),
        1 => Ok(Number::NegInt(d.i64()?)),
        2 => Ok(Number::Float(d.f64()?)),
        _ => Err(RpcError::Deser("invalid Number field".to_string())),
    }
}

impl<'b> crate::cbor::Decode<'b> for Number {
    fn decode(d: &mut crate::cbor::Decoder<'b>) -> Result<Self, RpcError> {
        decode_number(d)
    }
}
