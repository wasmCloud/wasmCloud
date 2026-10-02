//! Contains [`lift`] and [`lower`] functions to convert values from one component
//! instance to another in the context of a single [`wasmtime::Store`].

use tracing::trace;
use wasmtime::component::{ResourceAny, ResourceType, Val, types::Type};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};

use crate::engine::ctx::SharedCtx;

pub(crate) fn carries_cross_store_handle(ty: &Type) -> bool {
    match ty {
        Type::Bool
        | Type::S8
        | Type::U8
        | Type::S16
        | Type::U16
        | Type::S32
        | Type::U32
        | Type::S64
        | Type::U64
        | Type::Float32
        | Type::Float64
        | Type::Char
        | Type::String
        | Type::Enum(_)
        | Type::Flags(_) => false,
        Type::List(list) => carries_cross_store_handle(&list.ty()),
        Type::FixedLengthList(list) => carries_cross_store_handle(&list.ty()),
        Type::Map(map) => {
            carries_cross_store_handle(&map.key()) || carries_cross_store_handle(&map.value())
        }
        Type::Record(record) => record
            .fields()
            .any(|field| carries_cross_store_handle(&field.ty)),
        Type::Tuple(tuple) => tuple.types().any(|ty| carries_cross_store_handle(&ty)),
        Type::Variant(variant) => variant
            .cases()
            .any(|case| case.ty.as_ref().is_some_and(carries_cross_store_handle)),
        Type::Option(option) => carries_cross_store_handle(&option.ty()),
        Type::Result(result) => {
            result.ok().as_ref().is_some_and(carries_cross_store_handle)
                || result
                    .err()
                    .as_ref()
                    .is_some_and(carries_cross_store_handle)
        }
        Type::Own(_) | Type::Borrow(_) | Type::Future(_) | Type::Stream(_) | Type::ErrorContext => {
            true
        }
    }
}

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
        Val::FixedLengthList(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::FixedLengthList(vs))
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
        Val::Future(v) => Ok(Val::Future(v.clone())),
        Val::Stream(v) => Ok(Val::Stream(v.clone())),
        Val::ErrorContext(v) => Ok(Val::ErrorContext(v.clone())),
    }
}

/// Dynamic-linker call params ready for the callee, plus the borrows that
/// crossed by identity and must be released once the callee returns.
pub(crate) struct LoweredParams {
    pub(crate) vals: Vec<Val>,
    identity_borrows: Vec<ResourceAny>,
}

impl LoweredParams {
    /// Release the borrow slots of the handles lowered by identity.
    ///
    /// Lifting a guest `borrow` creates a borrow slot in the host table, scoped
    /// to the import call. The copy path in [`lower`] clears it as a side
    /// effect of `try_into_resource`; the identity path hands the handle
    /// through untouched, so the slot would outlive the call and trap the
    /// caller with "borrow handles still remain at the end of the call".
    pub(crate) fn release_identity_borrows(
        &self,
        mut store: impl AsContextMut<Data = SharedCtx>,
    ) -> wasmtime::Result<()> {
        for any in &self.identity_borrows {
            trace!(resource = ?any, "releasing identity-lowered borrow after linked call");
            any.try_into_resource::<ResourceAny>(store.as_context_mut())
                .context("failed to release identity-lowered borrow")?;
        }
        Ok(())
    }
}

/// Lower dynamic-linker call params, using each param's declared type so host
/// resource handles cross the linker by identity (see [`lower_with_type`]).
pub(crate) fn lower_params(
    store: &mut StoreContextMut<'_, SharedCtx>,
    params: &[Val],
    param_tys: &[Type],
) -> wasmtime::Result<LoweredParams> {
    if params.len() != param_tys.len() {
        return Err(wasmtime::format_err!(
            "dynamic call arity mismatch: {} args vs {} param types",
            params.len(),
            param_tys.len()
        ));
    }
    let mut lowered = LoweredParams {
        vals: Vec::with_capacity(params.len()),
        identity_borrows: Vec::new(),
    };
    for (v, ty) in params.iter().zip(param_tys) {
        let v = lower_with_type(store, v, ty, &mut lowered.identity_borrows)?;
        lowered.vals.push(v);
    }
    Ok(lowered)
}

/// Lift a dynamic-linker call's result values back into the caller's store,
/// writing each into `results` (see [`lift`]).
pub(crate) fn lift_results(
    store: &mut StoreContextMut<'_, SharedCtx>,
    results_buf: Vec<Val>,
    results: &mut [Val],
) -> wasmtime::Result<()> {
    for (i, v) in results_buf.into_iter().enumerate() {
        *results.get_mut(i).context("result index out of bounds")? = lift(store, v)?;
    }
    Ok(())
}

/// Lower `v` against its declared type `ty`, walking into compound values so a
/// linked component's handle crosses by identity wherever it is nested. Each
/// borrow passed through this way is pushed onto `identity_borrows`.
fn lower_with_type(
    store: &mut StoreContextMut<SharedCtx>,
    v: &Val,
    ty: &Type,
    identity_borrows: &mut Vec<ResourceAny>,
) -> wasmtime::Result<Val> {
    if !carries_cross_store_handle(ty) {
        return lower(store, v);
    }
    match (ty, v) {
        (Type::Own(resource_ty) | Type::Borrow(resource_ty), &Val::Resource(any))
            if *resource_ty == ResourceType::host::<ResourceAny>()
                && any.ty() == ResourceType::host::<ResourceAny>() =>
        {
            trace!(resource = ?any, "lowering host resource by identity");
            if matches!(ty, Type::Borrow(_)) && !any.owned() {
                identity_borrows.push(any);
            }
            Ok(Val::Resource(any))
        }
        (Type::List(list), Val::List(vs)) => {
            let ty = list.ty();
            let vs = vs
                .iter()
                .map(|v| lower_with_type(store, v, &ty, identity_borrows));
            Ok(Val::List(vs.collect::<wasmtime::Result<_>>()?))
        }
        (Type::FixedLengthList(list), Val::FixedLengthList(vs)) => {
            let ty = list.ty();
            let vs = vs
                .iter()
                .map(|v| lower_with_type(store, v, &ty, identity_borrows));
            Ok(Val::FixedLengthList(vs.collect::<wasmtime::Result<_>>()?))
        }
        (Type::Map(map), Val::Map(vs)) => {
            let (key_ty, value_ty) = (map.key(), map.value());
            let vs = vs.iter().map(|(k, v)| {
                Ok((
                    lower_with_type(store, k, &key_ty, identity_borrows)?,
                    lower_with_type(store, v, &value_ty, identity_borrows)?,
                ))
            });
            Ok(Val::Map(vs.collect::<wasmtime::Result<_>>()?))
        }
        (Type::Record(record), Val::Record(fields)) => {
            ensure_same_len("record fields", record.fields().len(), fields.len())?;
            let vs = record.fields().zip(fields).map(|(field, (name, v))| {
                Ok((
                    name.clone(),
                    lower_with_type(store, v, &field.ty, identity_borrows)?,
                ))
            });
            Ok(Val::Record(vs.collect::<wasmtime::Result<_>>()?))
        }
        (Type::Tuple(tuple), Val::Tuple(vs)) => {
            ensure_same_len("tuple elements", tuple.types().len(), vs.len())?;
            let vs = tuple
                .types()
                .zip(vs)
                .map(|(ty, v)| lower_with_type(store, v, &ty, identity_borrows));
            Ok(Val::Tuple(vs.collect::<wasmtime::Result<_>>()?))
        }
        (Type::Variant(variant), Val::Variant(name, Some(payload))) => {
            let case_ty = variant
                .cases()
                .find(|case| case.name == name)
                .and_then(|case| case.ty)
                .with_context(|| format!("variant case '{name}' carries no payload type"))?;
            let payload = lower_with_type(store, payload, &case_ty, identity_borrows)?;
            Ok(Val::Variant(name.clone(), Some(Box::new(payload))))
        }
        (Type::Option(option), Val::Option(Some(payload))) => {
            let payload = lower_with_type(store, payload, &option.ty(), identity_borrows)?;
            Ok(Val::Option(Some(Box::new(payload))))
        }
        (Type::Result(result), Val::Result(Ok(Some(payload)))) => {
            let ok_ty = result.ok().context("result `ok` carries no payload type")?;
            let payload = lower_with_type(store, payload, &ok_ty, identity_borrows)?;
            Ok(Val::Result(Ok(Some(Box::new(payload)))))
        }
        (Type::Result(result), Val::Result(Err(Some(payload)))) => {
            let err_ty = result
                .err()
                .context("result `err` carries no payload type")?;
            let payload = lower_with_type(store, payload, &err_ty, identity_borrows)?;
            Ok(Val::Result(Err(Some(Box::new(payload)))))
        }
        _ => lower(store, v),
    }
}

/// Error when a compound value's arity differs from its declared type's, so a
/// mismatched linked signature fails instead of `zip` dropping the extras.
fn ensure_same_len(what: &str, declared: usize, actual: usize) -> wasmtime::Result<()> {
    if declared != actual {
        return Err(wasmtime::format_err!(
            "linked call {what} mismatch: callee declares {declared}, caller passed {actual}"
        ));
    }
    Ok(())
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
        Val::FixedLengthList(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, v))
                .collect::<wasmtime::Result<_>>()?;
            Ok(Val::FixedLengthList(vs))
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
        Val::Future(v) => Ok(Val::Future(v)),
        Val::Stream(v) => Ok(Val::Stream(v)),
        Val::ErrorContext(v) => Ok(Val::ErrorContext(v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ctx::Ctx;

    fn make_store() -> wasmtime::Store<SharedCtx> {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

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
