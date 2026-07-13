//! Relocating `Val` trees across the store boundary for cross-store linked
//! calls.
//!
//! A linked call whose signature carries only **bridgeable** handles (p3
//! `stream<T>` of a supported element type, nested anywhere in aggregates) can
//! run in an ephemeral store even though a handle crosses the boundary: instead
//! of co-locating caller and callee in one store, we [`extract`] each argument
//! in the caller store into a [`Relocated`] tree and [`inject`] it into the
//! callee store. Handle-free values/subtrees are copied wholesale; each
//! `stream<T>` becomes a live, no-buffering channel pump (see [`stream_pump`])
//! so the body streams incrementally with backpressure rather than being
//! buffered at the boundary.
//!
//! `future<T>` relocates the same way as a stream (a one-shot pump). A
//! `resource` handle relocates as a cross-store **proxy** for host component
//! plugins (see [`resource_bridge`]): the plugin store keeps the real resource,
//! the caller holds an opaque proxy, and method calls and drops route back.
//!
//! `error-context` is rejected here, and cannot be relocated with wasmtime's
//! current public API: its `Val` carries a store-scoped table index (a `rep`
//! into the origin store's error-context tables) that is meaningless in another
//! store, and wasmtime exposes no host-side way to read an error-context's debug
//! message or mint a fresh one in the destination store (`ErrorContext` offers
//! only `from_val`/`into_val`, which round-trip the opaque index). Since an
//! error-context conveys only a debug string, a signature can carry that across
//! the boundary as a plain `result<_, string>` instead.

use wasmtime::component::{
    FutureAny, FutureReader, Lift, Lower, ResourceAny, StreamAny, StreamReader, Type, Val,
};
use wasmtime::{AsContextMut as _, StoreContextMut};

use crate::engine::ctx::SharedCtx;
use crate::engine::store::resource_bridge::{self, ProxyResource};
use crate::engine::store::stream_pump::{self, Done};

/// A value prepared to cross the store boundary: a copyable `Val`, or a
/// `stream<T>` carried as its producer (the source was `pipe`d in the origin
/// store). Aggregates recurse so streams nested anywhere are handled.
pub enum Relocated {
    Val(Val),
    /// A `stream<T>`, carried as a closure that builds the destination stream
    /// (it captures the typed producer; inject just calls it).
    Stream(ValFactory),
    /// A `future<T>`, carried the same way as a stream — a closure that builds
    /// the destination future from the paired receiver.
    Future(ValFactory),
    /// A `resource` handle, carried as a `proxy_id` into the plugin store's
    /// [`resource_bridge::ResourceRegistry`]. `owned` records whether it crossed
    /// as `own` (ownership transferred) or `borrow` (lent for the call).
    Resource {
        proxy_id: u64,
        owned: bool,
    },
    List(Vec<Relocated>),
    Tuple(Vec<Relocated>),
    Record(Vec<(String, Relocated)>),
    Variant(String, Box<Relocated>),
    Option(Box<Relocated>),
    Result(Result<Box<Relocated>, Box<Relocated>>),
    Map(Vec<(Relocated, Relocated)>),
}

/// Builds a `stream<T>`/`future<T>` value in the destination store from a
/// pre-wired producer (it captures the source side's pump endpoint).
type ValFactory = Box<dyn FnOnce(StoreContextMut<SharedCtx>) -> wasmtime::Result<Val> + Send>;

/// Set up a no-buffering pump for a `stream<T>`: `pipe` the source reader (in
/// `src`) into a channel, and return a factory that builds the destination
/// stream from the channel's producer, plus the pump's drain signal.
fn bridge_stream<T>(
    mut src: StoreContextMut<SharedCtx>,
    any: StreamAny,
) -> wasmtime::Result<(ValFactory, Done)>
where
    T: Lift + Lower + Send + Sync + 'static,
{
    let reader = StreamReader::<T>::try_from_stream_any(any)?;
    let (consumer, producer, done) = stream_pump::channel::<T>(stream_pump::DEFAULT_CAPACITY);
    reader.pipe(src.as_context_mut(), consumer)?;
    let factory: ValFactory = Box::new(move |mut dst: StoreContextMut<SharedCtx>| {
        let reader = StreamReader::new(dst.as_context_mut(), producer)?;
        let any = reader.try_into_stream_any(dst)?;
        Ok(Val::Stream(any))
    });
    Ok((factory, done))
}

/// Set up a one-shot pump for a `future<T>`: `pipe` the source future (in `src`)
/// into a [`stream_pump::FutureSink`], and return a factory that builds the
/// destination future from the paired receiver, plus the pump's completion
/// signal.
fn bridge_future<T>(
    mut src: StoreContextMut<SharedCtx>,
    any: FutureAny,
) -> wasmtime::Result<(ValFactory, Done)>
where
    T: Lift + Lower + Send + Sync + 'static,
{
    let reader = FutureReader::<T>::try_from_future_any(any)?;
    let (sink, rx, done) = stream_pump::future_channel::<T>();
    reader.pipe(src.as_context_mut(), sink)?;
    let factory: ValFactory = Box::new(move |mut dst: StoreContextMut<SharedCtx>| {
        let reader = FutureReader::new(dst.as_context_mut(), async move {
            rx.await
                .map_err(|_| wasmtime::format_err!("future bridge: source dropped before value"))
        })?;
        let any = reader.try_into_future_any(dst)?;
        Ok(Val::Future(any))
    });
    Ok((factory, done))
}

/// Whether a `stream<T>`/`future<T>` of this element type can be relocated
/// across stores. Mirrors the dispatch in [`stream_factory`]/[`future_factory`]:
/// the supported elements are the scalar types and `string`.
pub fn bridgeable_element_type(ty: &Type) -> bool {
    matches!(
        ty,
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
    )
}

/// Dispatch a `stream<T>` to a typed pump by its element type.
fn stream_factory(
    src: StoreContextMut<SharedCtx>,
    any: StreamAny,
    payload: &Type,
) -> wasmtime::Result<(ValFactory, Done)> {
    macro_rules! dispatch {
        ($($variant:ident => $t:ty),* $(,)?) => {
            match payload {
                $(Type::$variant => bridge_stream::<$t>(src, any),)*
                other => wasmtime::bail!(
                    "cross-store bridge: unsupported stream element type {other:?}"
                ),
            }
        };
    }
    dispatch!(
        Bool => bool, S8 => i8, U8 => u8, S16 => i16, U16 => u16,
        S32 => i32, U32 => u32, S64 => i64, U64 => u64,
        Float32 => f32, Float64 => f64, Char => char, String => String,
    )
}

/// Dispatch a `future<T>` to a typed pump by its element type.
fn future_factory(
    src: StoreContextMut<SharedCtx>,
    any: FutureAny,
    payload: &Type,
) -> wasmtime::Result<(ValFactory, Done)> {
    macro_rules! dispatch {
        ($($variant:ident => $t:ty),* $(,)?) => {
            match payload {
                $(Type::$variant => bridge_future::<$t>(src, any),)*
                other => wasmtime::bail!(
                    "cross-store bridge: unsupported future element type {other:?}"
                ),
            }
        };
    }
    dispatch!(
        Bool => bool, S8 => i8, U8 => u8, S16 => i16, U16 => u16,
        S32 => i32, U32 => u32, S64 => i64, U64 => u64,
        Float32 => f32, Float64 => f64, Char => char, String => String,
    )
}

/// Whether `val` contains a store-bound handle (`stream`/`future`/`resource`/
/// `error-context`) anywhere, so we know whether structural relocation is
/// needed or the value can be copied wholesale.
fn contains_handle(val: &Val) -> bool {
    match val {
        Val::Stream(_) | Val::Future(_) | Val::Resource(_) | Val::ErrorContext(_) => true,
        Val::List(vs) | Val::Tuple(vs) => vs.iter().any(contains_handle),
        Val::Record(fs) => fs.iter().any(|(_, v)| contains_handle(v)),
        Val::Variant(_, Some(v)) | Val::Option(Some(v)) => contains_handle(v),
        Val::Result(Ok(Some(v))) | Val::Result(Err(Some(v))) => contains_handle(v),
        Val::Map(es) => es
            .iter()
            .any(|(k, v)| contains_handle(k) || contains_handle(v)),
        _ => false,
    }
}

/// Relocate a `resource` handle across the boundary. On a **caller** store the
/// handle is one of our proxies, so its `proxy_id` is read out (removing the
/// proxy for an `own` transfer, leaving it for a `borrow`). On a **plugin** store
/// the handle is a real resource, so it is registered — kept alive and reachable
/// by later method calls and the eventual drop. A store with neither role rejects
/// it (resources cross only between a host component plugin and its callers).
fn extract_resource(
    mut store: StoreContextMut<SharedCtx>,
    any: ResourceAny,
    owned: bool,
) -> wasmtime::Result<Relocated> {
    if any.ty() == resource_bridge::proxy_resource_type() {
        let res = any.try_into_resource::<ProxyResource>(store.as_context_mut())?;
        let proxy_id = if owned {
            store.data_mut().table.delete(res)?.proxy_id
        } else {
            store
                .data_mut()
                .table
                .get(&res)
                .map_err(|e| wasmtime::format_err!("proxy resource not in caller table: {e}"))?
                .proxy_id
        };
        Ok(Relocated::Resource { proxy_id, owned })
    } else if let Some(registry) = store.data_mut().resource_registry.as_mut() {
        let proxy_id = registry.register(any);
        Ok(Relocated::Resource { proxy_id, owned })
    } else {
        wasmtime::bail!(
            "cross-store bridge: a `resource` handle reached a store with no resource bridge \
             (resources cross only between a host component plugin and its callers)"
        )
    }
}

/// Rebuild a relocated `resource` handle in `store`. On a **plugin** store this
/// looks up the real resource for an incoming call argument (removing it for an
/// `own` transfer, borrowing it otherwise); on a **caller** store it mints a
/// fresh proxy referencing the plugin-side real.
fn inject_resource(
    mut store: StoreContextMut<SharedCtx>,
    proxy_id: u64,
    owned: bool,
) -> wasmtime::Result<Val> {
    if store.data().resource_registry.is_some() {
        let real = {
            let registry =
                store.data_mut().resource_registry.as_mut().ok_or_else(|| {
                    wasmtime::format_err!("resource registry unexpectedly missing")
                })?;
            if owned {
                registry.take(proxy_id)
            } else {
                registry.get(proxy_id)
            }
        };
        let real = real.ok_or_else(|| {
            wasmtime::format_err!("cross-store bridge: unknown proxied resource {proxy_id}")
        })?;
        Ok(Val::Resource(real))
    } else {
        let res = store.data_mut().table.push(ProxyResource { proxy_id })?;
        Ok(Val::Resource(
            res.try_into_resource_any(store.as_context_mut())?,
        ))
    }
}

/// Extract a value from `store` (its origin), setting up a live channel pump for
/// each `stream<T>` (`reader.pipe` → no buffering) and pushing the pump's drain
/// signal into `dones`. Handle-free values/subtrees are copied wholesale.
pub fn extract(
    mut store: StoreContextMut<SharedCtx>,
    val: &Val,
    ty: &Type,
    dones: &mut Vec<Done>,
) -> wasmtime::Result<Relocated> {
    if !contains_handle(val) {
        return Ok(Relocated::Val(val.clone()));
    }
    match (val, ty) {
        (Val::Stream(_), Type::Stream(st)) => {
            let payload = st
                .ty()
                .ok_or_else(|| wasmtime::format_err!("stream is missing its element type"))?;
            let Val::Stream(any) = val.clone() else {
                unreachable!()
            };
            let (factory, done) = stream_factory(store, any, &payload)?;
            dones.push(done);
            Ok(Relocated::Stream(factory))
        }
        (Val::Future(_), Type::Future(ft)) => {
            let payload = ft
                .ty()
                .ok_or_else(|| wasmtime::format_err!("future is missing its element type"))?;
            let Val::Future(any) = val.clone() else {
                unreachable!()
            };
            let (factory, done) = future_factory(store, any, &payload)?;
            dones.push(done);
            Ok(Relocated::Future(factory))
        }
        (Val::Resource(any), Type::Own(_)) => extract_resource(store, *any, true),
        (Val::Resource(any), Type::Borrow(_)) => extract_resource(store, *any, false),
        // An error-context's `Val` is a store-scoped table index with no
        // host-side API to read its message or rebuild it in another store, so it
        // cannot cross the boundary; steer callers to a plain error type carrying
        // the same debug string. See the module docs.
        (Val::ErrorContext(_), _) => {
            wasmtime::bail!(
                "cross-store bridge does not transfer `error-context` values; return a plain \
                 error type (e.g. `result<_, string>`) across a component-host-plugin boundary"
            )
        }
        (Val::List(vs), Type::List(lt)) => {
            let et = lt.ty();
            let mut out = Vec::with_capacity(vs.len());
            for v in vs {
                out.push(extract(store.as_context_mut(), v, &et, dones)?);
            }
            Ok(Relocated::List(out))
        }
        (Val::Tuple(vs), Type::Tuple(tt)) => {
            let tys: Vec<Type> = tt.types().collect();
            let mut out = Vec::with_capacity(vs.len());
            for (v, t) in vs.iter().zip(tys.iter()) {
                out.push(extract(store.as_context_mut(), v, t, dones)?);
            }
            Ok(Relocated::Tuple(out))
        }
        (Val::Record(vs), Type::Record(rt)) => {
            let ftys: Vec<Type> = rt.fields().map(|f| f.ty).collect();
            let mut out = Vec::with_capacity(vs.len());
            for ((n, v), t) in vs.iter().zip(ftys.iter()) {
                out.push((n.clone(), extract(store.as_context_mut(), v, t, dones)?));
            }
            Ok(Relocated::Record(out))
        }
        (Val::Option(Some(v)), Type::Option(ot)) => Ok(Relocated::Option(Box::new(extract(
            store.as_context_mut(),
            v,
            &ot.ty(),
            dones,
        )?))),
        (Val::Result(Ok(Some(v))), Type::Result(rt)) => {
            let it = rt
                .ok()
                .ok_or_else(|| wasmtime::format_err!("result `ok` is missing its type"))?;
            Ok(Relocated::Result(Ok(Box::new(extract(
                store.as_context_mut(),
                v,
                &it,
                dones,
            )?))))
        }
        (Val::Result(Err(Some(v))), Type::Result(rt)) => {
            let it = rt
                .err()
                .ok_or_else(|| wasmtime::format_err!("result `err` is missing its type"))?;
            Ok(Relocated::Result(Err(Box::new(extract(
                store.as_context_mut(),
                v,
                &it,
                dones,
            )?))))
        }
        (Val::Variant(case, Some(v)), Type::Variant(vt)) => {
            let case_ty = vt
                .cases()
                .find(|c| c.name == case)
                .and_then(|c| c.ty)
                .ok_or_else(|| {
                    wasmtime::format_err!("variant case `{case}` has no payload type")
                })?;
            Ok(Relocated::Variant(
                case.clone(),
                Box::new(extract(store.as_context_mut(), v, &case_ty, dones)?),
            ))
        }
        (Val::Map(es), Type::Map(mt)) => {
            let kt = mt.key();
            let vt = mt.value();
            let mut out = Vec::with_capacity(es.len());
            for (k, v) in es {
                let k = extract(store.as_context_mut(), k, &kt, dones)?;
                let v = extract(store.as_context_mut(), v, &vt, dones)?;
                out.push((k, v));
            }
            Ok(Relocated::Map(out))
        }
        // A handle in a value/type mismatch (shouldn't happen for valid calls).
        _ => wasmtime::bail!("cross-store bridge: handle in an unexpected value/type position"),
    }
}

/// Inject a relocated value into `store` (its destination), creating a fresh
/// `stream<T>` from each producer.
pub fn inject(mut store: StoreContextMut<SharedCtx>, r: Relocated) -> wasmtime::Result<Val> {
    match r {
        Relocated::Val(v) => Ok(v),
        Relocated::Stream(factory) | Relocated::Future(factory) => factory(store),
        Relocated::Resource { proxy_id, owned } => inject_resource(store, proxy_id, owned),
        Relocated::List(rs) => {
            let mut out = Vec::with_capacity(rs.len());
            for r in rs {
                out.push(inject(store.as_context_mut(), r)?);
            }
            Ok(Val::List(out))
        }
        Relocated::Tuple(rs) => {
            let mut out = Vec::with_capacity(rs.len());
            for r in rs {
                out.push(inject(store.as_context_mut(), r)?);
            }
            Ok(Val::Tuple(out))
        }
        Relocated::Record(fs) => {
            let mut out = Vec::with_capacity(fs.len());
            for (n, r) in fs {
                out.push((n, inject(store.as_context_mut(), r)?));
            }
            Ok(Val::Record(out))
        }
        Relocated::Variant(case, r) => Ok(Val::Variant(
            case,
            Some(Box::new(inject(store.as_context_mut(), *r)?)),
        )),
        Relocated::Option(r) => Ok(Val::Option(Some(Box::new(inject(
            store.as_context_mut(),
            *r,
        )?)))),
        Relocated::Result(Ok(r)) => Ok(Val::Result(Ok(Some(Box::new(inject(
            store.as_context_mut(),
            *r,
        )?))))),
        Relocated::Result(Err(r)) => Ok(Val::Result(Err(Some(Box::new(inject(
            store.as_context_mut(),
            *r,
        )?))))),
        Relocated::Map(es) => {
            let mut out = Vec::with_capacity(es.len());
            for (k, v) in es {
                let k = inject(store.as_context_mut(), k)?;
                let v = inject(store.as_context_mut(), v)?;
                out.push((k, v));
            }
            Ok(Val::Map(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::contains_handle;
    use wasmtime::component::Val;

    // `contains_handle` decides the relocation fast path (copy wholesale) vs.
    // structural descent. Its `true` arms (`stream`/`future`/`resource`/
    // `error-context`) require a live store to construct and are covered by the
    // integration tests; here we pin the negative path and the recursion so a
    // handle-free value is never needlessly decomposed and a nested handle is
    // never missed structurally.

    #[test]
    fn primitives_have_no_handle() {
        for v in [
            Val::Bool(true),
            Val::S8(-1),
            Val::U8(1),
            Val::U32(7),
            Val::U64(9),
            Val::Float64(1.5),
            Val::Char('z'),
            Val::String("hi".into()),
            Val::Enum("a".into()),
            Val::Flags(vec!["x".into()]),
        ] {
            assert!(!contains_handle(&v), "{v:?} should have no handle");
        }
    }

    #[test]
    fn primitive_aggregates_have_no_handle() {
        let rec = Val::Record(vec![
            ("a".into(), Val::U32(1)),
            ("b".into(), Val::String("x".into())),
        ]);
        assert!(!contains_handle(&rec));
        assert!(!contains_handle(&Val::List(vec![Val::U8(1), Val::U8(2)])));
        assert!(!contains_handle(&Val::Tuple(vec![
            Val::Bool(true),
            rec.clone()
        ])));
        assert!(!contains_handle(&Val::Option(Some(Box::new(Val::U32(1))))));
        assert!(!contains_handle(&Val::Option(None)));
        assert!(!contains_handle(&Val::Result(Ok(Some(Box::new(
            rec.clone()
        ))))));
        assert!(!contains_handle(&Val::Result(Err(None))));
        assert!(!contains_handle(&Val::Variant(
            "c".into(),
            Some(Box::new(Val::U32(1)))
        )));
        assert!(!contains_handle(&Val::Variant("c".into(), None)));
        assert!(!contains_handle(&Val::Map(vec![(
            Val::String("k".into()),
            Val::U32(1)
        )])));
    }

    #[test]
    fn deeply_nested_primitives_have_no_handle() {
        let deep = Val::List(vec![Val::Record(vec![(
            "x".into(),
            Val::Option(Some(Box::new(Val::Tuple(vec![
                Val::U32(1),
                Val::Map(vec![(Val::String("k".into()), Val::List(vec![Val::U8(0)]))]),
            ])))),
        )])]);
        assert!(!contains_handle(&deep));
    }
}
