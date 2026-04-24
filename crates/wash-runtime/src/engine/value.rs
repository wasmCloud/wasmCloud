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
        Val::Map(vs) => {
            let vs = vs
                .iter()
                .map(|(k, v)| {
                    let k = lower(store, k)?;
                    let v = lower(store, v)?;
                    Ok((k, v))
                })
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Map(vs))
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
        Val::Map(vs) => {
            let vs = vs
                .into_iter()
                .map(|(k, v)| {
                    let k = lift(store, k)?;
                    let v = lift(store, v)?;
                    Ok((k, v))
                })
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::Map(vs))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ctx::Ctx;

    fn make_store() -> wasmtime::Store<SharedCtx> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let ctx = Ctx::builder("test-workload", "test-component").build();
        wasmtime::Store::new(&engine, SharedCtx::new(ctx))
    }

    #[test]
    fn map_lower_scalars() {
        let mut store = make_store();
        let mut cx = store.as_context_mut();
        let val = Val::Map(vec![
            (Val::String("foo".into()), Val::U32(1)),
            (Val::String("bar".into()), Val::U32(2)),
        ]);
        assert_eq!(lower(&mut cx, &val).unwrap(), val);
    }

    #[test]
    fn map_lift_scalars() {
        let mut store = make_store();
        let mut cx = store.as_context_mut();
        let val = Val::Map(vec![
            (Val::Bool(true), Val::String("yes".into())),
            (Val::Bool(false), Val::String("no".into())),
        ]);
        assert_eq!(lift(&mut cx, val.clone()).unwrap(), val);
    }

    #[test]
    fn map_empty() {
        let mut store = make_store();
        let mut cx = store.as_context_mut();
        let val = Val::Map(vec![]);
        assert_eq!(lower(&mut cx, &val).unwrap(), val);
        assert_eq!(lift(&mut cx, val.clone()).unwrap(), val);
    }

    #[test]
    fn map_preserves_order_and_duplicate_keys() {
        // Val::Map is a Vec — no deduplication, insertion order preserved
        let mut store = make_store();
        let mut cx = store.as_context_mut();
        let val = Val::Map(vec![
            (Val::U32(1), Val::String("first".into())),
            (Val::U32(1), Val::String("duplicate".into())),
        ]);
        assert_eq!(lower(&mut cx, &val).unwrap(), val);
    }

    #[test]
    fn map_nested_roundtrip() {
        let mut store = make_store();
        let mut cx = store.as_context_mut();
        let val = Val::Map(vec![(
            Val::String("nums".into()),
            Val::List(vec![Val::U32(1), Val::U32(2), Val::U32(3)]),
        )]);
        let lowered = lower(&mut cx, &val).unwrap();
        let lifted = lift(&mut cx, lowered).unwrap();
        assert_eq!(lifted, val);
    }
}
