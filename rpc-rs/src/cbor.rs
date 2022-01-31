//! cbor encoder and decoder
//!
//! This module wraps the underlying cbor implementation (currently minicbor)
//! so that third parties do not need to have any additional dependencies,
//! and to give us freedom to change the implementation in the future.
//!
#![allow(dead_code)]

use crate::error::{RpcError, RpcResult};
use std::fmt::Debug;

#[derive(Clone)]
pub struct Decoder<'b> {
    inner: minicbor::Decoder<'b>,
}

/// A non-allocating CBOR decoder
impl<'b> Decoder<'b> {
    /// Construct a Decoder for the given byte slice
    pub fn new(bytes: &'b [u8]) -> Self {
        Self {
            inner: minicbor::Decoder::new(bytes),
        }
    }

    /// Decode a bool value
    pub fn bool(&mut self) -> RpcResult<bool> {
        Ok(self.inner.bool()?)
    }

    /// Decode a u8 value
    pub fn u8(&mut self) -> RpcResult<u8> {
        Ok(self.inner.u8()?)
    }

    /// Decode a u16 value
    pub fn u16(&mut self) -> RpcResult<u16> {
        Ok(self.inner.u16()?)
    }

    /// Decode a u32 value
    pub fn u32(&mut self) -> RpcResult<u32> {
        Ok(self.inner.u32()?)
    }

    /// Decode a u64 value
    pub fn u64(&mut self) -> RpcResult<u64> {
        Ok(self.inner.u64()?)
    }

    /// Decode an i8 value
    pub fn i8(&mut self) -> RpcResult<i8> {
        Ok(self.inner.i8()?)
    }

    /// Decode an i16 value
    pub fn i16(&mut self) -> RpcResult<i16> {
        Ok(self.inner.i16()?)
    }

    /// Decode an i32 value
    pub fn i32(&mut self) -> RpcResult<i32> {
        Ok(self.inner.i32()?)
    }

    /// Decode an i64 value
    pub fn i64(&mut self) -> RpcResult<i64> {
        Ok(self.inner.i64()?)
    }

    /// Decode an f32 value
    pub fn f32(&mut self) -> RpcResult<f32> {
        Ok(self.inner.f32()?)
    }

    /// Decode an f64 value
    pub fn f64(&mut self) -> RpcResult<f64> {
        Ok(self.inner.f64()?)
    }

    /// Decode a char value
    pub fn char(&mut self) -> RpcResult<char> {
        Ok(self.inner.char()?)
    }

    /// Decode a string slice
    pub fn str(&mut self) -> RpcResult<&'b str> {
        Ok(self.inner.str()?)
    }

    /// Decode a byte slice
    pub fn bytes(&mut self) -> RpcResult<&'b [u8]> {
        Ok(self.inner.bytes()?)
    }

    /// Begin decoding an array. If the length is known,
    /// it is returned as `Some`. For indefinite arrays, `None` is returned.
    pub fn array(&mut self) -> RpcResult<Option<u64>> {
        Ok(self.inner.array()?)
    }

    /// Begin decoding a map. If the size is known,
    /// it is returned as `Some`. For indefinite maps, `None` is returned.
    pub fn map(&mut self) -> RpcResult<Option<u64>> {
        Ok(self.inner.map()?)
    }

    /// Inspect the CBOR type at the current position
    pub fn datatype(&mut self) -> RpcResult<Type> {
        Ok(self.inner.datatype()?.into())
    }

    /// Skip over the current value
    pub fn skip(&mut self) -> RpcResult<()> {
        Ok(self.inner.skip()?)
    }

    // Pierce the veil.
    // This module exposes public functions to support code generated
    // by `weld-codegen`. Its purpose is to create an abstraction layer
    // around a cbor implementation. This function breaks that abstraction,
    // and any use of it outside the wasmbus-rpc crate risks breaking
    // if there is a change to the underlying implementation.
    //#[hidden]
    //pub(crate) fn inner(&mut self) -> &mut minicbor::Decoder {
    //    &self.inner
    //}
}

impl<'b> Debug for Decoder<'b> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.fmt(f)
    }
}

/// A type that accepts byte slices for writing
pub trait Write {
    type Error: std::fmt::Display;
    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error>;
}

struct WriteX<W: Write> {
    writer: Box<dyn Write<Error = W::Error>>,
}

impl<W: Write> WriteX<W> {
    pub fn new(writer: Box<dyn Write<Error = W::Error>>) -> Self {
        Self { writer }
    }
}

impl<W: Write> Write for WriteX<W> {
    type Error = RpcError;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), RpcError> {
        self.writer
            .write_all(buf)
            .map_err(|e| RpcError::Ser(format!("encoder write: {}", e)))
    }
}

impl<W: Write> minicbor::encode::write::Write for WriteX<W> {
    type Error = RpcError;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.writer
            .write_all(buf)
            .map_err(|e| RpcError::Ser(format!("encoding: {}", e)))
    }
}

pub struct Encoder<W: Write> {
    inner: minicbor::Encoder<WriteX<W>>,
}

/// A non-allocating CBOR encoder
impl<W: Write + 'static> Encoder<W> {
    /// Construct an Encoder that writes to the given `Write` sink
    pub fn new(writer: W) -> Encoder<W> {
        let writer = WriteX::new(Box::new(writer));
        Self {
            inner: minicbor::Encoder::new(writer),
        }
    }

    /// Encode a bool value
    pub fn bool(&mut self, x: bool) -> Result<&mut Self, RpcError> {
        self.inner.bool(x)?;
        Ok(self)
    }

    /// Encode a u8 value
    pub fn u8(&mut self, x: u8) -> Result<&mut Self, RpcError> {
        self.inner.u8(x)?;
        Ok(self)
    }

    /// Encode a u16 value
    pub fn u16(&mut self, x: u16) -> Result<&mut Self, RpcError> {
        self.inner.u16(x)?;
        Ok(self)
    }

    /// Encode a u32 value
    pub fn u32(&mut self, x: u32) -> Result<&mut Self, RpcError> {
        self.inner.u32(x)?;
        Ok(self)
    }

    /// Encode a u64 value
    pub fn u64(&mut self, x: u64) -> Result<&mut Self, RpcError> {
        self.inner.u64(x)?;
        Ok(self)
    }

    /// Encode an i8 value
    pub fn i8(&mut self, x: i8) -> Result<&mut Self, RpcError> {
        self.inner.i8(x)?;
        Ok(self)
    }

    /// Encode an i16 value
    pub fn i16(&mut self, x: i16) -> Result<&mut Self, RpcError> {
        self.inner.i16(x)?;
        Ok(self)
    }

    /// Encode an i32 value
    pub fn i32(&mut self, x: i32) -> Result<&mut Self, RpcError> {
        self.inner.i32(x)?;
        Ok(self)
    }

    /// Encode an i64 value
    pub fn i64(&mut self, x: i64) -> Result<&mut Self, RpcError> {
        self.inner.i64(x)?;
        Ok(self)
    }

    /// Encode an f32 value
    pub fn f32(&mut self, x: f32) -> Result<&mut Self, RpcError> {
        self.inner.f32(x)?;
        Ok(self)
    }

    /// Encode an f64 value
    pub fn f64(&mut self, x: f64) -> Result<&mut Self, RpcError> {
        self.inner.f64(x)?;
        Ok(self)
    }

    /// Encode a char value
    pub fn char(&mut self, x: char) -> Result<&mut Self, RpcError> {
        self.inner.char(x)?;
        Ok(self)
    }

    /// Encode a byte slice
    pub fn bytes(&mut self, x: &[u8]) -> Result<&mut Self, RpcError> {
        self.inner.bytes(x)?;
        Ok(self)
    }

    /// Encode a string slice
    pub fn str(&mut self, x: &str) -> Result<&mut Self, RpcError> {
        self.inner.str(x)?;
        Ok(self)
    }

    /// Begin encoding an array with `len` elements
    pub fn array(&mut self, len: u64) -> Result<&mut Self, RpcError> {
        self.inner.array(len)?;
        Ok(self)
    }

    /// Begin encoding an array with indefinite length
    pub fn begin_array(&mut self) -> Result<&mut Self, RpcError> {
        self.inner.begin_array()?;
        Ok(self)
    }

    /// Begin encoding a map with `len` elements
    pub fn map(&mut self, len: u64) -> Result<&mut Self, RpcError> {
        self.inner.map(len)?;
        Ok(self)
    }

    /// Begin encoding a map with indefinite length
    pub fn begin_map(&mut self) -> Result<&mut Self, RpcError> {
        self.inner.begin_map()?;
        Ok(self)
    }

    /// Begin encoding a byte slice with indefinite length
    /// Use Encoder::end to terminate
    pub fn begin_bytes(&mut self) -> Result<&mut Self, RpcError> {
        self.inner.begin_bytes()?;
        Ok(self)
    }

    /// Begin encoding an indefinite number of string slices
    /// Use Encoder::end to terminate
    pub fn begin_str(&mut self) -> Result<&mut Self, RpcError> {
        self.inner.begin_str()?;
        Ok(self)
    }

    /// Terminate an indefinite collection
    pub fn end(&mut self) -> Result<&mut Self, RpcError> {
        self.inner.end()?;
        Ok(self)
    }

    // Pierce the veil.
    // This module exposes public functions to support code generated
    // by `weld-codegen`. Its purpose is to create an abstraction layer
    // around a cbor implementation. This function breaks that abstraction,
    // and any use of it outside the wasmbus-rpc crate risks breaking
    // if there is a change to the underlying implementation.
    //#[hidden]
    //pub(crate) fn inner(&mut self) -> &mut minicbor::Encoder {
    //    &self.inner
    //}
}

/// CBOR data types.
#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Debug, Hash)]
pub enum Type {
    Bool,
    Null,
    Undefined,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    F32,
    F64,
    Simple,
    Bytes,
    BytesIndef,
    String,
    StringIndef,
    Array,
    ArrayIndef,
    Map,
    MapIndef,
    Tag,
    Break,
    Unknown(u8),
}

use minicbor::data::Type as MT;
impl From<MT> for Type {
    fn from(t: MT) -> Type {
        match t {
            MT::Bool => Type::Bool,
            MT::Null => Type::Null,
            MT::Undefined => Type::Undefined,
            MT::U8 => Type::U8,
            MT::U16 => Type::U16,
            MT::U32 => Type::U32,
            MT::U64 => Type::U64,
            MT::I8 => Type::I8,
            MT::I16 => Type::I16,
            MT::I32 => Type::I32,
            MT::I64 => Type::I64,
            MT::F16 => Type::F16,
            MT::F32 => Type::F32,
            MT::F64 => Type::F64,
            MT::Simple => Type::Simple,
            MT::Bytes => Type::Bytes,
            MT::BytesIndef => Type::BytesIndef,
            MT::String => Type::String,
            MT::StringIndef => Type::StringIndef,
            MT::Array => Type::Array,
            MT::ArrayIndef => Type::ArrayIndef,
            MT::Map => Type::Map,
            MT::MapIndef => Type::MapIndef,
            MT::Tag => Type::Tag,
            MT::Break => Type::Break,
            MT::Unknown(x) => Type::Unknown(x),
        }
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Type::Bool => f.write_str("bool"),
            Type::Null => f.write_str("null"),
            Type::Undefined => f.write_str("undefined"),
            Type::U8 => f.write_str("u8"),
            Type::U16 => f.write_str("u16"),
            Type::U32 => f.write_str("u32"),
            Type::U64 => f.write_str("u64"),
            Type::I8 => f.write_str("i8"),
            Type::I16 => f.write_str("i16"),
            Type::I32 => f.write_str("i32"),
            Type::I64 => f.write_str("i64"),
            Type::F16 => f.write_str("f16"),
            Type::F32 => f.write_str("f32"),
            Type::F64 => f.write_str("f64"),
            Type::Simple => f.write_str("simple"),
            Type::Bytes => f.write_str("bytes"),
            Type::BytesIndef => f.write_str("indefinite bytes"),
            Type::String => f.write_str("string"),
            Type::StringIndef => f.write_str("indefinite string"),
            Type::Array => f.write_str("array"),
            Type::ArrayIndef => f.write_str("indefinite array"),
            Type::Map => f.write_str("map"),
            Type::MapIndef => f.write_str("indefinite map"),
            Type::Tag => f.write_str("tag"),
            Type::Break => f.write_str("break"),
            Type::Unknown(n) => write!(f, "{:#x}", n),
        }
    }
}

#[inline]
pub fn decode_u8(d: &mut Decoder<'_>) -> RpcResult<u8> {
    d.u8()
}
#[inline]
pub fn decode_u16(d: &mut Decoder<'_>) -> RpcResult<u16> {
    d.u16()
}
#[inline]
pub fn decode_u32(d: &mut Decoder<'_>) -> RpcResult<u32> {
    d.u32()
}
#[inline]
pub fn decode_u64(d: &mut Decoder<'_>) -> RpcResult<u64> {
    d.u64()
}
#[inline]
pub fn decode_i8(d: &mut Decoder<'_>) -> RpcResult<i8> {
    d.i8()
}
#[inline]
pub fn decode_i16(d: &mut Decoder<'_>) -> RpcResult<i16> {
    d.i16()
}
#[inline]
pub fn decode_i32(d: &mut Decoder<'_>) -> RpcResult<i32> {
    d.i32()
}
#[inline]
pub fn decode_i64(d: &mut Decoder<'_>) -> RpcResult<i64> {
    d.i64()
}
#[inline]
pub fn decode_boolean(d: &mut Decoder<'_>) -> RpcResult<bool> {
    d.bool()
}
#[inline]
pub fn decode_string(d: &mut Decoder<'_>) -> RpcResult<String> {
    Ok(d.str()?.to_string())
}
#[inline]
pub fn decode_blob(d: &mut Decoder<'_>) -> RpcResult<Vec<u8>> {
    Ok(d.bytes()?.to_vec())
}
#[inline]
pub fn decode_byte(d: &mut Decoder<'_>) -> RpcResult<u8> {
    d.u8()
}
#[inline]
pub fn decode_char(d: &mut Decoder<'_>) -> RpcResult<char> {
    d.char()
}
#[inline]
pub fn decode_integer(d: &mut Decoder<'_>) -> RpcResult<i32> {
    d.i32()
}
#[inline]
pub fn decode_long(d: &mut Decoder<'_>) -> RpcResult<i64> {
    d.i64()
}
#[inline]
pub fn decode_float(d: &mut Decoder<'_>) -> RpcResult<f32> {
    d.f32()
}
#[inline]
pub fn decode_double(d: &mut Decoder<'_>) -> RpcResult<f64> {
    d.f64()
}
