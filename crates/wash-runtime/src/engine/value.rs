//! Contains [`lift`] and [`lower`] functions to convert values from one component
//! instance to another in the context of a single [`wasmtime::Store`].

use tracing::trace;
use wasmtime::component::Val;
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};

use crate::engine::ctx::SharedCtx;

pub(crate) fn lower(store: &mut StoreContextMut<SharedCtx>, v: &Val) -> wasmtime::Result<Val> {
    match v {
        &Val::Bool(v) => Ok(Val::Bool(v)),
        &Val::S8(v) => Ok(Val::S8(v)),
        &Val::U8(v) => Ok(Val::U8(v)),
        &Val::S16(v) => Ok(Val::S16(v)),
        &Val::U16(v) => Ok(Val::U16(v)),
        &Val::S32(v) => Ok(Val::S32(v)),
        &Val::U32(v) => Ok(Val::U32(v)),
        &Val::S64(v) => Ok(Val::S64(v)),
        &Val::U64(v) => Ok(Val::U64(v)),
        &Val::Float32(v) => Ok(Val::Float32(v)),
        &Val::Float64(v) => Ok(Val::Float64(v)),
        &Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v.clone())),
        Val::List(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .iter()
                .map(|(name, v)| {
                    let v = lower(store, v)?;
                    Ok((name.clone(), v))
                })
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lower(store, v)?;
                Ok(Val::Variant(k.clone(), Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k.clone(), None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v.clone())),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lower(store, v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lower(store, v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lower(store, v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v.clone())),
        &Val::Resource(any) => {
            if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi_io::bindings::wasi::io::streams::OutputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lowering output stream");
                let stream = store.data_mut().table.delete(res)?;
                let resource = store.data_mut().table.push(stream)?;
                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else {
                trace!(resource = ?any, "lowering resource");
                let res = any
                    .try_into_resource(store.as_context_mut())
                    .context("failed to lower resource")?;
                if res.owned() {
                    trace!("lowering owned resource");
                    Ok(Val::Resource(
                        store
                            .data_mut()
                            .table
                            .delete(res)
                            .context("owned resource not in table")?,
                    ))
                } else {
                    trace!("lowering borrowed resource");
                    Ok(Val::Resource(
                        store
                            .data_mut()
                            .table
                            .get(&res)
                            .context("borrowed resource not in table")
                            .cloned()?,
                    ))
                }
            }
        }
        &Val::Future(_) | &Val::Stream(_) | &Val::ErrorContext(_) => {
            wasmtime::bail!("async not supported")
        }
    }
}

pub(crate) fn lift(store: &mut StoreContextMut<SharedCtx>, v: Val) -> wasmtime::Result<Val> {
    match v {
        Val::Bool(v) => Ok(Val::Bool(v)),
        Val::S8(v) => Ok(Val::S8(v)),
        Val::U8(v) => Ok(Val::U8(v)),
        Val::S16(v) => Ok(Val::S16(v)),
        Val::U16(v) => Ok(Val::U16(v)),
        Val::S32(v) => Ok(Val::S32(v)),
        Val::U32(v) => Ok(Val::U32(v)),
        Val::S64(v) => Ok(Val::S64(v)),
        Val::U64(v) => Ok(Val::U64(v)),
        Val::Float32(v) => Ok(Val::Float32(v)),
        Val::Float64(v) => Ok(Val::Float64(v)),
        Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v)),
        Val::List(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .into_iter()
                .map(|(name, v)| {
                    let v = lift(store, v)?;
                    Ok((name, v))
                })
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lift(store, *v)?;
                Ok(Val::Variant(k, Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k, None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v)),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lift(store, *v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lift(store, *v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lift(store, *v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v)),
        Val::Resource(any) => {
            if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi_io::bindings::wasi::io::streams::OutputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lifting output stream");
                let stream = store.data_mut().table.delete(res)?;
                let resource = store.data_mut().table.push(stream)?;

                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi_io::bindings::wasi::io::streams::InputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lifting input stream");
                let stream = store.data_mut().table.delete(res)?;
                let resource = store.data_mut().table.push(stream)?;

                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else {
                trace!(resource = ?any, "lifting resource");
                let res = store.data_mut().table.push(any)?;
                Ok(Val::Resource(
                    res.try_into_resource_any(store.as_context_mut())?,
                ))
            }
        }
        Val::Future(_) | Val::Stream(_) | Val::ErrorContext(_) => {
            wasmtime::bail!("async not supported")
        }
    }
}
