use core::fmt::{self, Display};
use core::future::Future;
use core::iter::zip;
use core::mem;
use core::pin::{pin, Pin};
use core::task::{Context, Poll};

use std::error::Error;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use bytes::{BufMut as _, Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use tracing::{instrument, warn};
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{ResourceType, Type, Val};
use wasmtime::AsContextMut;
use wasmtime_wasi::{
    FileInputStream, HostInputStream, InputStream, Pollable, StreamError, StreamResult, Subscribe,
    WasiView,
};

/// WRPC input stream
pub struct OutgoingHostInputStream(Box<dyn HostInputStream>);

/// Stream Error
#[derive(Debug)]
pub enum OutgoingStreamError {
    /// Fail
    Failed(anyhow::Error),
    /// Trap
    Trap(anyhow::Error),
}

impl Display for OutgoingStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(err) => write!(f, "last operation failed: {err:#}"),
            Self::Trap(err) => write!(f, "trap: {err:#}"),
        }
    }
}

impl Error for OutgoingStreamError {}

impl Stream for OutgoingHostInputStream {
    type Item = Result<Bytes, OutgoingStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match pin!(self.0.ready()).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(()) => {}
        }
        match self.0.read(8096) {
            Ok(buf) => Poll::Ready(Some(Ok(buf))),
            Err(StreamError::LastOperationFailed(err)) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Failed(err))))
            }
            Err(StreamError::Trap(err)) => Poll::Ready(Some(Err(OutgoingStreamError::Trap(err)))),
            Err(StreamError::Closed) => Poll::Ready(None),
        }
    }
}

/// WRPC input stream
pub struct OutgoingFileInputStream(FileInputStream);

impl Stream for OutgoingFileInputStream {
    type Item = Result<Bytes, OutgoingStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match pin!(self.0.read(8096)).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(buf)) => Poll::Ready(Some(Ok(buf))),
            Poll::Ready(Err(StreamError::LastOperationFailed(err))) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Failed(err))))
            }
            Poll::Ready(Err(StreamError::Trap(err))) => {
                Poll::Ready(Some(Err(OutgoingStreamError::Trap(err))))
            }
            Poll::Ready(Err(StreamError::Closed)) => Poll::Ready(None),
        }
    }
}

/// Converts value into wrpc value
#[instrument(level = "trace", skip(store))]
pub fn to_wrpc_value<T: WasiView>(
    mut store: impl AsContextMut<Data = T>,
    v: &Val,
    ty: &Type,
) -> anyhow::Result<wrpc_transport_legacy::Value> {
    let mut store = store.as_context_mut();
    match (v, ty) {
        (Val::Bool(v), Type::Bool) => Ok(wrpc_transport_legacy::Value::Bool(*v)),
        (Val::S8(v), Type::S8) => Ok(wrpc_transport_legacy::Value::S8(*v)),
        (Val::U8(v), Type::U8) => Ok(wrpc_transport_legacy::Value::U8(*v)),
        (Val::S16(v), Type::S16) => Ok(wrpc_transport_legacy::Value::S16(*v)),
        (Val::U16(v), Type::U16) => Ok(wrpc_transport_legacy::Value::U16(*v)),
        (Val::S32(v), Type::S32) => Ok(wrpc_transport_legacy::Value::S32(*v)),
        (Val::U32(v), Type::U32) => Ok(wrpc_transport_legacy::Value::U32(*v)),
        (Val::S64(v), Type::S64) => Ok(wrpc_transport_legacy::Value::S64(*v)),
        (Val::U64(v), Type::U64) => Ok(wrpc_transport_legacy::Value::U64(*v)),
        (Val::Float32(v), Type::Float32) => Ok(wrpc_transport_legacy::Value::F32(*v)),
        (Val::Float64(v), Type::Float64) => Ok(wrpc_transport_legacy::Value::F64(*v)),
        (Val::Char(v), Type::Char) => Ok(wrpc_transport_legacy::Value::Char(*v)),
        (Val::String(v), Type::String) => Ok(wrpc_transport_legacy::Value::String(v.to_string())),
        (Val::List(vs), Type::List(ty)) => {
            let ty = ty.ty();
            vs.iter()
                .map(|v| to_wrpc_value(&mut store, v, &ty))
                .collect::<anyhow::Result<_>>()
                .map(wrpc_transport_legacy::Value::List)
        }
        (Val::Record(vs), Type::Record(ty)) => zip(vs, ty.fields())
            .map(|((_, v), Field { ty, .. })| to_wrpc_value(&mut store, v, &ty))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport_legacy::Value::Record),
        (Val::Tuple(vs), Type::Tuple(ty)) => zip(vs, ty.types())
            .map(|(v, ty)| to_wrpc_value(&mut store, v, &ty))
            .collect::<anyhow::Result<_>>()
            .map(wrpc_transport_legacy::Value::Tuple),
        (Val::Variant(discriminant, v), Type::Variant(ty)) => {
            let (discriminant, ty) = zip(0.., ty.cases())
                .find_map(|(i, Case { name, ty })| (name == discriminant).then_some((i, ty)))
                .context("unknown variant discriminant")?;
            let nested = match (v, ty) {
                (Some(v), Some(ty)) => {
                    let v = to_wrpc_value(store, v, &ty)?;
                    Some(Box::new(v))
                }
                (Some(_v), None) => {
                    bail!("variant value of unknown type")
                }
                (None, Some(_ty)) => {
                    bail!("variant value missing")
                }
                (None, None) => None,
            };
            Ok(wrpc_transport_legacy::Value::Variant {
                discriminant,
                nested,
            })
        }
        (Val::Enum(discriminant), Type::Enum(ty)) => zip(0.., ty.names())
            .find_map(|(i, name)| (name == discriminant).then_some(i))
            .context("unknown enum discriminant")
            .map(wrpc_transport_legacy::Value::Enum),
        (Val::Option(v), Type::Option(ty)) => v
            .as_ref()
            .map(|v| to_wrpc_value(store, v, &ty.ty()).map(Box::new))
            .transpose()
            .map(wrpc_transport_legacy::Value::Option),
        (Val::Result(v), Type::Result(ty)) => {
            let v = match v {
                Ok(v) => match (v, ty.ok()) {
                    (Some(v), Some(ty)) => {
                        let v = to_wrpc_value(store, v, &ty)?;
                        Ok(Some(Box::new(v)))
                    }
                    (Some(_v), None) => bail!("`result::ok` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::ok` value missing"),
                    (None, None) => Ok(None),
                },
                Err(v) => match (v, ty.err()) {
                    (Some(v), Some(ty)) => {
                        let v = to_wrpc_value(store, v, &ty)?;
                        Err(Some(Box::new(v)))
                    }
                    (Some(_v), None) => bail!("`result::err` value of unknown type"),
                    (None, Some(_ty)) => bail!("`result::err` value missing"),
                    (None, None) => Err(None),
                },
            };
            Ok(wrpc_transport_legacy::Value::Result(v))
        }
        (Val::Flags(vs), Type::Flags(ty)) => {
            let mut v = 0;
            for name in vs {
                let i = zip(0.., ty.names())
                    .find_map(|(i, flag_name)| (name == flag_name).then_some(i))
                    .context("unknown flag")?;
                ensure!(
                    i < 64,
                    "flag discriminants over 64 currently cannot be represented"
                );
                v |= 1 << i
            }
            Ok(wrpc_transport_legacy::Value::Flags(v))
        }
        (Val::Resource(resource), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<InputStream>() {
                let stream = resource
                    .try_into_resource::<InputStream>(&mut store)
                    .context("failed to downcast `wasi:io/input-stream`")?;
                let stream = if stream.owned() {
                    store
                        .data_mut()
                        .table()
                        .delete(stream)
                        .context("failed to delete input stream")?
                } else {
                    store
                        .data_mut()
                        .table()
                        .get_mut(&stream)
                        .context("failed to get input stream")?;
                    // NOTE: In order to handle this we'd need to know how many bytes has the
                    // receiver read. That means that some kind of callback would be required from
                    // the receiver. This is not trivial and generally should be a very rare use case.
                    bail!("borrowed `wasi:io/input-stream` not supported yet");
                };
                Ok(wrpc_transport_legacy::Value::Stream(match stream {
                    InputStream::Host(stream) => {
                        Box::pin(OutgoingHostInputStream(stream).map(|buf| {
                            let buf = buf?;
                            Ok(buf
                                .into_iter()
                                .map(wrpc_transport_legacy::Value::U8)
                                .map(Some)
                                .collect())
                        }))
                    }
                    InputStream::File(stream) => {
                        Box::pin(OutgoingFileInputStream(stream).map(|buf| {
                            let buf = buf?;
                            Ok(buf
                                .into_iter()
                                .map(wrpc_transport_legacy::Value::U8)
                                .map(Some)
                                .collect())
                        }))
                    }
                }))
            } else if *ty == ResourceType::host::<Pollable>() {
                let pollable = resource
                    .try_into_resource::<Pollable>(&mut store)
                    .context("failed to downcast `wasi:io/pollable")?;
                if pollable.owned() {
                    store
                        .data_mut()
                        .table()
                        .delete(pollable)
                        .context("failed to delete pollable")?;
                } else {
                    store
                        .data_mut()
                        .table()
                        .get_mut(&pollable)
                        .context("failed to get pollable")?;
                };
                Ok(wrpc_transport_legacy::Value::Future(Box::pin(async {
                    bail!("`wasi:io/pollable` not supported yet")
                })))
            } else {
                bail!("resources not supported yet")
            }
        }
        _ => bail!("value type mismatch"),
    }
}

/// Converts wrpc value into value
#[instrument(level = "trace", skip(store, val))]
pub fn from_wrpc_value<T: WasiView>(
    mut store: impl AsContextMut<Data = T>,
    val: wrpc_transport_legacy::Value,
    ty: &Type,
) -> anyhow::Result<Val> {
    let mut store = store.as_context_mut();
    match (val, ty) {
        (wrpc_transport_legacy::Value::Bool(v), Type::Bool) => Ok(Val::Bool(v)),
        (wrpc_transport_legacy::Value::U8(v), Type::U8) => Ok(Val::U8(v)),
        (wrpc_transport_legacy::Value::U16(v), Type::U16) => Ok(Val::U16(v)),
        (wrpc_transport_legacy::Value::U32(v), Type::U32) => Ok(Val::U32(v)),
        (wrpc_transport_legacy::Value::U64(v), Type::U64) => Ok(Val::U64(v)),
        (wrpc_transport_legacy::Value::S8(v), Type::S8) => Ok(Val::S8(v)),
        (wrpc_transport_legacy::Value::S16(v), Type::S16) => Ok(Val::S16(v)),
        (wrpc_transport_legacy::Value::S32(v), Type::S32) => Ok(Val::S32(v)),
        (wrpc_transport_legacy::Value::S64(v), Type::S64) => Ok(Val::S64(v)),
        (wrpc_transport_legacy::Value::F32(v), Type::Float32) => Ok(Val::Float32(v)),
        (wrpc_transport_legacy::Value::F64(v), Type::Float64) => Ok(Val::Float64(v)),
        (wrpc_transport_legacy::Value::Char(v), Type::Char) => Ok(Val::Char(v)),
        (wrpc_transport_legacy::Value::String(v), Type::String) => Ok(Val::String(v.into())),
        (wrpc_transport_legacy::Value::List(vs), Type::List(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            let el_ty = ty.ty();
            for v in vs {
                let v = from_wrpc_value(&mut store, v, &el_ty)
                    .context("failed to convert list element")?;
                w_vs.push(v);
            }
            Ok(Val::List(w_vs))
        }
        (wrpc_transport_legacy::Value::Record(vs), Type::Record(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, Field { name, ty }) in zip(vs, ty.fields()) {
                let v = from_wrpc_value(&mut store, v, &ty)
                    .context("failed to convert record field")?;
                w_vs.push((name.to_string(), v));
            }
            Ok(Val::Record(w_vs))
        }
        (wrpc_transport_legacy::Value::Tuple(vs), Type::Tuple(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, ty) in zip(vs, ty.types()) {
                let v = from_wrpc_value(&mut store, v, &ty)
                    .context("failed to convert tuple element")?;
                w_vs.push(v);
            }
            Ok(Val::Tuple(w_vs))
        }
        (
            wrpc_transport_legacy::Value::Variant {
                discriminant,
                nested,
            },
            Type::Variant(ty),
        ) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            let Case { name, ty } = ty
                .cases()
                .nth(discriminant)
                .context("variant discriminant not found")?;
            let v = if let Some(ty) = ty {
                let v = nested.context("nested value missing")?;
                let v =
                    from_wrpc_value(store, *v, &ty).context("failed to convert variant value")?;
                Some(Box::new(v))
            } else {
                None
            };
            Ok(Val::Variant(name.to_string(), v))
        }
        (wrpc_transport_legacy::Value::Enum(discriminant), Type::Enum(ty)) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            ty.names()
                .nth(discriminant)
                .context("enum discriminant not found")
                .map(ToString::to_string)
                .map(Val::Enum)
        }
        (wrpc_transport_legacy::Value::Option(v), Type::Option(ty)) => {
            let v = if let Some(v) = v {
                let v = from_wrpc_value(store, *v, &ty.ty())
                    .context("failed to convert option value")?;
                Some(Box::new(v))
            } else {
                None
            };
            Ok(Val::Option(v))
        }
        (wrpc_transport_legacy::Value::Result(v), Type::Result(ty)) => match v {
            Ok(None) => Ok(Val::Result(Ok(None))),
            Ok(Some(v)) => {
                let ty = ty.ok().context("`result::ok` type missing")?;
                let v = from_wrpc_value(store, *v, &ty)
                    .context("failed to convert `result::ok` value")?;
                Ok(Val::Result(Ok(Some(Box::new(v)))))
            }
            Err(None) => Ok(Val::Result(Err(None))),
            Err(Some(v)) => {
                let ty = ty.err().context("`result::err` type missing")?;
                let v = from_wrpc_value(store, *v, &ty)
                    .context("failed to convert `result::err` value")?;
                Ok(Val::Result(Err(Some(Box::new(v)))))
            }
        },
        (wrpc_transport_legacy::Value::Flags(v), Type::Flags(ty)) => {
            // NOTE: Currently flags are limited to 64
            let mut names = Vec::with_capacity(64);
            for (i, name) in zip(0..64, ty.names()) {
                if v & (1 << i) != 0 {
                    names.push(name.to_string())
                }
            }
            Ok(Val::Flags(names))
        }
        (wrpc_transport_legacy::Value::Future(_v), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<Pollable>() {
                // TODO: Implement once https://github.com/bytecodealliance/wasmtime/issues/7714
                // is addressed
                bail!("`wasi:io/pollable` not supported yet")
            } else {
                // TODO: Implement in preview3 or via a wasmCloud-specific interface
                bail!("dynamically-typed futures not supported yet")
            }
        }
        (wrpc_transport_legacy::Value::Stream(v), Type::Own(ty) | Type::Borrow(ty)) => {
            if *ty == ResourceType::host::<InputStream>() {
                let res = store
                    .data_mut()
                    .table()
                    .push(InputStream::Host(Box::new(IncomingValueInputStream {
                        stream: v,
                        item: None,
                        buffer: Bytes::default(),
                    })))
                    .context("failed to push stream resource to table")?;
                res.try_into_resource_any(store)
                    .context("failed to convert resource to ResourceAny")
                    .map(Val::Resource)
            } else {
                // TODO: Implement in preview3 or via a wrpc-specific interface
                bail!("dynamically-typed streams not supported yet")
            }
        }
        (wrpc_transport_legacy::Value::String(_), Type::Own(_ty) | Type::Borrow(_ty)) => {
            // TODO: Implement guest resource handling
            bail!("resources not supported yet")
        }
        _ => bail!("type mismatch"),
    }
}

struct IncomingValueInputStream {
    stream: Pin<
        Box<dyn Stream<Item = anyhow::Result<Vec<Option<wrpc_transport_legacy::Value>>>> + Send>,
    >,
    item: Option<Option<anyhow::Result<Vec<Option<wrpc_transport_legacy::Value>>>>>,
    buffer: Bytes,
}

#[async_trait]
impl Subscribe for IncomingValueInputStream {
    async fn ready(&mut self) {
        if self.item.is_some() || !self.buffer.is_empty() {
            return;
        }
        self.item = Some(self.stream.next().await);
    }
}

impl HostInputStream for IncomingValueInputStream {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if !self.buffer.is_empty() {
            if self.buffer.len() > size {
                return Ok(self.buffer.split_to(size));
            } else {
                return Ok(mem::take(&mut self.buffer));
            }
        }
        let Some(mut item) = self.item.take() else {
            // `ready` was not called yet
            return Ok(Bytes::default());
        };
        let Some(item) = item.take() else {
            // `next` returned `None`, assume stream is closed
            return Err(StreamError::Closed);
        };
        let values = item.map_err(StreamError::LastOperationFailed)?;
        let mut buffer = BytesMut::with_capacity(values.len());
        for value in values {
            let Some(wrpc_transport_legacy::Value::U8(v)) = value else {
                Err(StreamError::LastOperationFailed(anyhow!(
                    "stream item type mismatch"
                )))?
            };
            buffer.put_u8(v)
        }
        let buffer = buffer.freeze();
        if buffer.len() > size {
            self.buffer = buffer;
            Ok(self.buffer.split_to(size))
        } else {
            Ok(buffer)
        }
    }
}
