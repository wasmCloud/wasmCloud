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
//! `resource` handle relocates as a cross-store **proxy** (see
//! [`resource_bridge`]): the store of the component that defines it keeps the
//! real resource, every other holds an opaque proxy, and method calls and drops
//! route back.
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
use crate::engine::store::resource_bridge::{self, Lent, Owner, ProxyResource};
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
    /// A `resource` handle, carried as a `proxy_id` into its owner's
    /// [`resource_bridge::ResourceRegistry`]. `owned` records whether it crossed
    /// as `own` (ownership transferred) or `borrow` (lent for the call).
    Resource {
        owner: Owner,
        proxy_id: u64,
        owned: bool,
    },
    List(Vec<Relocated>),
    FixedLengthList(Vec<Relocated>),
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

/// Defines the pump-supported `stream<T>`/`future<T>` element types — the
/// scalar types and `string` — in one place, generating the classification
/// ([`bridgeable_element_type`]) and both typed dispatches ([`stream_factory`],
/// [`future_factory`]) from the same list.
macro_rules! bridgeable_elements {
    ($($variant:ident => $t:ty),* $(,)?) => {
        /// Whether a `stream<T>`/`future<T>` of this element type can be
        /// relocated across stores.
        pub fn bridgeable_element_type(ty: &Type) -> bool {
            matches!(ty, $(Type::$variant)|*)
        }

        /// Dispatch a `stream<T>` to a typed pump by its element type.
        fn stream_factory(
            src: StoreContextMut<SharedCtx>,
            any: StreamAny,
            payload: &Type,
        ) -> wasmtime::Result<(ValFactory, Done)> {
            match payload {
                $(Type::$variant => bridge_stream::<$t>(src, any),)*
                other => wasmtime::bail!(
                    "cross-store bridge: unsupported stream element type {other:?}"
                ),
            }
        }

        /// Dispatch a `future<T>` to a typed pump by its element type.
        fn future_factory(
            src: StoreContextMut<SharedCtx>,
            any: FutureAny,
            payload: &Type,
        ) -> wasmtime::Result<(ValFactory, Done)> {
            match payload {
                $(Type::$variant => bridge_future::<$t>(src, any),)*
                other => wasmtime::bail!(
                    "cross-store bridge: unsupported future element type {other:?}"
                ),
            }
        }
    };
}

bridgeable_elements!(
    Bool => bool, S8 => i8, U8 => u8, S16 => i16, U16 => u16,
    S32 => i32, U32 => u32, S64 => i64, U64 => u64,
    Float32 => f32, Float64 => f64, Char => char, String => String,
);

/// Whether `val` contains a store-bound handle (`stream`/`future`/`resource`/
/// `error-context`) anywhere, so we know whether structural relocation is
/// needed or the value can be copied wholesale.
fn contains_handle(val: &Val) -> bool {
    match val {
        Val::Stream(_) | Val::Future(_) | Val::Resource(_) | Val::ErrorContext(_) => true,
        Val::List(vs) | Val::FixedLengthList(vs) | Val::Tuple(vs) => vs.iter().any(contains_handle),
        Val::Record(fs) => fs.iter().any(|(_, v)| contains_handle(v)),
        Val::Variant(_, Some(v)) | Val::Option(Some(v)) => contains_handle(v),
        Val::Result(Ok(Some(v))) | Val::Result(Err(Some(v))) => contains_handle(v),
        Val::Map(es) => es
            .iter()
            .any(|(k, v)| contains_handle(k) || contains_handle(v)),
        _ => false,
    }
}

/// Relocate a `resource` handle across the boundary. A proxy has its owner and
/// `proxy_id` read out (the proxy is removed for an `own` transfer, left for a
/// `borrow`). Anything else is a real resource, which only the store of the
/// component that defines it may send: it is registered there — kept alive and
/// reachable by later method calls and the eventual drop.
fn extract_resource(
    mut store: StoreContextMut<SharedCtx>,
    any: ResourceAny,
    owned: bool,
) -> wasmtime::Result<Relocated> {
    if any.ty() == resource_bridge::proxy_resource_type() {
        let res = any.try_into_resource::<ProxyResource>(store.as_context_mut())?;
        let table = &mut store.data_mut().table;
        let ProxyResource { owner, proxy_id } = if owned {
            table.delete(res)?
        } else {
            table
                .get(&res)
                .map_err(|e| wasmtime::format_err!("proxy resource not in caller table: {e}"))?
                .clone()
        };
        return Ok(Relocated::Resource {
            owner,
            proxy_id,
            owned,
        });
    }
    let Some(registry) = store.data_mut().resource_registry.as_mut() else {
        wasmtime::bail!(
            "cross-store bridge: this `resource` handle cannot leave its store; only a \
             resource a linked component or host component plugin defines can"
        )
    };
    // A linked component lending its own resource would have to serve the
    // borrower's calls on it while still waiting on the call that lent it.
    wasmtime::ensure!(
        owned || *registry.owner() == Owner::Plugin,
        "a component cannot lend a resource it defines to a linked component"
    );
    Ok(Relocated::Resource {
        owner: registry.owner().clone(),
        proxy_id: registry.register(any),
        owned,
    })
}

/// What rebuilding values in a store has done so far, kept so that a failure
/// part-way, at any depth, can be undone.
#[derive(Default)]
struct Injection {
    lent: Lent,
    /// Proxies made for `own` handles.
    proxies: Vec<ResourceAny>,
    /// This store's own resources, taken out of its registry for an `own`.
    taken: Vec<ResourceAny>,
    /// The `stream`s and `future`s made.
    channels: Vec<Val>,
    /// Values a failure left unreached.
    unreached: Vec<Relocated>,
}

impl Injection {
    /// Undo everything recorded, leaving nothing behind in `store`, and return
    /// what still owns a resource in another store.
    fn undo(self, mut store: StoreContextMut<SharedCtx>) -> Vec<Relocated> {
        let mut stranded = self.unreached;
        for any in self.proxies {
            let removed = any
                .try_into_resource::<ProxyResource>(store.as_context_mut())
                .and_then(|proxy| Ok(store.data_mut().table.delete(proxy)?));
            if let Ok(ProxyResource { owner, proxy_id }) = removed {
                stranded.push(Relocated::Resource {
                    owner,
                    proxy_id,
                    owned: true,
                });
            }
        }
        if let Some(registry) = store.data_mut().resource_registry.as_mut() {
            for real in self.taken {
                registry.stage_taken(real);
            }
        }
        // Closing the near end is what stops the pump feeding it.
        for channel in self.channels {
            let _ = match channel {
                Val::Stream(mut stream) => stream.close(store.as_context_mut()),
                Val::Future(mut future) => future.close(store.as_context_mut()),
                _ => Ok(()),
            };
        }
        let _ = self.lent.release(store);
        stranded
    }
}

/// Rebuild a relocated `resource` handle in `store`. In its owner's store that
/// is the real resource (removed from the registry for an `own` transfer, lent
/// otherwise); anywhere else it is a fresh proxy.
fn inject_resource(
    mut store: StoreContextMut<SharedCtx>,
    owner: Owner,
    proxy_id: u64,
    owned: bool,
    done: &mut Injection,
) -> wasmtime::Result<Val> {
    if let Some(registry) = store.data_mut().resource_registry.as_mut()
        && *registry.owner() == owner
    {
        let real = if owned {
            let real = registry.take(proxy_id);
            done.taken.extend(real);
            real
        } else {
            let real = registry.lend(proxy_id);
            done.lent.reals.extend(real.map(|_| proxy_id));
            real
        };
        return real.map(Val::Resource).ok_or_else(|| {
            wasmtime::format_err!("cross-store bridge: unknown proxied resource {proxy_id}")
        });
    }
    let proxy = ProxyResource {
        owner: owner.clone(),
        proxy_id,
    };
    let made = match store.data_mut().table.push(proxy) {
        Ok(res) => res.try_into_resource_any(store.as_context_mut()),
        Err(e) => Err(e.into()),
    };
    match made {
        Ok(any) => {
            if owned {
                done.proxies.push(any);
            } else {
                done.lent.proxies.push(any);
            }
            Ok(Val::Resource(any))
        }
        Err(e) => {
            if owned {
                done.unreached.push(Relocated::Resource {
                    owner,
                    proxy_id,
                    owned,
                });
            }
            Err(e)
        }
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
        (Val::Stream(any), Type::Stream(st)) => {
            let payload = st
                .ty()
                .ok_or_else(|| wasmtime::format_err!("stream is missing its element type"))?;
            let (factory, done) = stream_factory(store, any.clone(), &payload)?;
            dones.push(done);
            Ok(Relocated::Stream(factory))
        }
        (Val::Future(any), Type::Future(ft)) => {
            let payload = ft
                .ty()
                .ok_or_else(|| wasmtime::format_err!("future is missing its element type"))?;
            let (factory, done) = future_factory(store, any.clone(), &payload)?;
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
        (Val::FixedLengthList(vs), Type::FixedLengthList(lt)) => {
            let et = lt.ty();
            let mut out = Vec::with_capacity(vs.len());
            for v in vs {
                out.push(extract(store.as_context_mut(), v, &et, dones)?);
            }
            Ok(Relocated::FixedLengthList(out))
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

/// Rebuild each of `rs` in `store`. On failure the ones not reached are
/// recorded in `done`.
fn inject_each(
    store: &mut StoreContextMut<SharedCtx>,
    rs: Vec<Relocated>,
    done: &mut Injection,
) -> wasmtime::Result<Vec<Val>> {
    let mut vals = Vec::with_capacity(rs.len());
    let mut rs = rs.into_iter();
    while let Some(r) = rs.next() {
        match inject(store.as_context_mut(), r, done) {
            Ok(val) => vals.push(val),
            Err(e) => {
                done.unreached.extend(rs);
                return Err(e);
            }
        }
    }
    Ok(vals)
}

/// Inject a relocated value into `store` (its destination), creating a fresh
/// `stream<T>` from each producer and recording in `done` everything made.
fn inject(
    mut store: StoreContextMut<SharedCtx>,
    r: Relocated,
    done: &mut Injection,
) -> wasmtime::Result<Val> {
    match r {
        Relocated::Val(v) => Ok(v),
        Relocated::Stream(factory) | Relocated::Future(factory) => {
            let channel = factory(store)?;
            done.channels.push(channel.clone());
            Ok(channel)
        }
        Relocated::Resource {
            owner,
            proxy_id,
            owned,
        } => inject_resource(store, owner, proxy_id, owned, done),
        Relocated::List(rs) => Ok(Val::List(inject_each(&mut store, rs, done)?)),
        Relocated::FixedLengthList(rs) => {
            Ok(Val::FixedLengthList(inject_each(&mut store, rs, done)?))
        }
        Relocated::Tuple(rs) => Ok(Val::Tuple(inject_each(&mut store, rs, done)?)),
        Relocated::Record(fs) => {
            let (names, rs): (Vec<_>, Vec<_>) = fs.into_iter().unzip();
            let vals = inject_each(&mut store, rs, done)?;
            Ok(Val::Record(names.into_iter().zip(vals).collect()))
        }
        Relocated::Variant(case, r) => {
            Ok(Val::Variant(case, Some(Box::new(inject(store, *r, done)?))))
        }
        Relocated::Option(r) => Ok(Val::Option(Some(Box::new(inject(store, *r, done)?)))),
        Relocated::Result(Ok(r)) => Ok(Val::Result(Ok(Some(Box::new(inject(store, *r, done)?))))),
        Relocated::Result(Err(r)) => Ok(Val::Result(Err(Some(Box::new(inject(store, *r, done)?))))),
        Relocated::Map(es) => {
            let flat = es.into_iter().flat_map(|(k, v)| [k, v]).collect();
            let mut vals = inject_each(&mut store, flat, done)?.into_iter();
            let mut out = Vec::with_capacity(vals.len() / 2);
            while let (Some(k), Some(v)) = (vals.next(), vals.next()) {
                out.push((k, v));
            }
            Ok(Val::Map(out))
        }
    }
}

/// Values that could not all be rebuilt in a store.
pub(crate) struct InjectError {
    pub(crate) error: wasmtime::Error,
    /// What still owns a resource somewhere else: the values never reached, and
    /// the proxies already made. Their owners have to be told to drop them.
    pub(crate) stranded: Vec<Relocated>,
}

/// Rebuild every one of `values` in `store`, as the arguments of a call about
/// to run there or the results of one that has.
///
/// On failure, at any depth, nothing is left behind: what was already rebuilt
/// is given up again, and whatever it owned elsewhere is handed back as
/// stranded.
pub(crate) fn inject_all(
    mut store: StoreContextMut<SharedCtx>,
    values: Vec<Relocated>,
) -> Result<(Vec<Val>, Lent), InjectError> {
    let mut done = Injection::default();
    match inject_each(&mut store, values, &mut done) {
        Ok(vals) => Ok((vals, done.lent)),
        Err(error) => Err(InjectError {
            error,
            stranded: done.undo(store),
        }),
    }
}

/// Finish a call that ran in `store`: take back what its arguments were lent,
/// and prepare its results to leave.
///
/// The result pumps keep running under `store`, which outlives the call, so
/// their drain signals are dropped rather than awaited.
pub(crate) fn finish_call(
    mut store: StoreContextMut<SharedCtx>,
    lent: Lent,
    results: &[Val],
    result_tys: &[Type],
) -> wasmtime::Result<Vec<Relocated>> {
    lent.release(store.as_context_mut())?;
    let mut pumps = Vec::new();
    let mut out = Vec::with_capacity(results.len());
    for (val, ty) in results.iter().zip(result_tys) {
        out.push(extract(store.as_context_mut(), val, ty, &mut pumps)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{Relocated, contains_handle, extract, inject, inject_all, stream_pump};
    use crate::engine::ctx::{Ctx, SharedCtx};
    use crate::engine::store::resource_bridge::{Owner, ResourceRegistry};
    use wasmtime::AsContextMut as _;
    use wasmtime::component::{Component, Resource, StreamReader, Val, types::ComponentItem};

    fn make_store() -> wasmtime::Store<SharedCtx> {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let ctx = Ctx::builder("test-workload", "test-component").build();
        wasmtime::Store::new(&engine, SharedCtx::new(ctx))
    }

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
        assert!(!contains_handle(&Val::FixedLengthList(vec![
            Val::U8(1),
            Val::U8(2)
        ])));
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

    #[test]
    fn inject_preserves_fixed_length_list() {
        let mut store = make_store();
        let relocated = Relocated::FixedLengthList(vec![
            Relocated::Val(Val::U8(1)),
            Relocated::Val(Val::U8(2)),
        ]);

        assert_eq!(
            inject(store.as_context_mut(), relocated, &mut Default::default()).unwrap(),
            Val::FixedLengthList(vec![Val::U8(1), Val::U8(2)])
        );
    }

    const ME: &str = "me";
    const OTHER: &str = "other";

    /// A store owning one registered resource, and that resource's id.
    fn make_owner_store() -> (wasmtime::Store<SharedCtx>, u64) {
        let mut store = make_store();
        let real = Resource::<u32>::new_own(1)
            .try_into_resource_any(&mut store)
            .unwrap();
        let mut registry = ResourceRegistry::new(Owner::Component(ME.into()));
        let id = registry.register(real);
        store.data_mut().resource_registry = Some(registry);
        (store, id)
    }

    fn resource(owner: &str, proxy_id: u64, owned: bool) -> Relocated {
        Relocated::Resource {
            owner: Owner::Component(owner.into()),
            proxy_id,
            owned,
        }
    }

    /// An `own` of a resource the store's own registry has never heard of,
    /// which cannot be rebuilt.
    fn unknown() -> Relocated {
        resource(ME, u64::MAX, true)
    }

    /// The ids of the owned resources handed back as stranded, in order.
    fn stranded_ids(values: Vec<Relocated>) -> Vec<u64> {
        let failed = {
            let (mut store, _) = make_owner_store();
            let failed = inject_all(store.as_context_mut(), values).err().unwrap();
            assert!(
                store.data().table.is_empty(),
                "a failed injection left a proxy in the store's table"
            );
            failed
        };
        let mut ids = Vec::new();
        for value in failed.stranded {
            match value {
                Relocated::Resource {
                    proxy_id,
                    owned: true,
                    ..
                } => ids.push(proxy_id),
                // An unreached map key.
                Relocated::Val(_) => {}
                _ => panic!("only owned resources are stranded here"),
            }
        }
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_failed_injection_gives_back_what_it_reached_and_what_it_did_not() {
        let values = vec![
            resource(OTHER, 1, true),
            unknown(),
            resource(OTHER, 2, true),
        ];
        assert_eq!(stranded_ids(values), [1, 2]);
    }

    #[test]
    fn a_failed_nested_injection_gives_back_every_level() {
        let values = vec![
            resource(OTHER, 1, true),
            Relocated::Tuple(vec![
                resource(OTHER, 2, true),
                Relocated::List(vec![resource(OTHER, 3, true), unknown()]),
                resource(OTHER, 4, true),
            ]),
            resource(OTHER, 5, true),
        ];
        assert_eq!(stranded_ids(values), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_failed_injection_gives_back_the_rest_of_a_record_and_a_map() {
        let record = Relocated::Record(vec![
            ("a".into(), resource(OTHER, 1, true)),
            ("b".into(), unknown()),
            ("c".into(), resource(OTHER, 2, true)),
        ]);
        assert_eq!(stranded_ids(vec![record]), [1, 2]);

        let map = Relocated::Map(vec![
            (Relocated::Val(Val::U8(0)), resource(OTHER, 1, true)),
            (unknown(), resource(OTHER, 2, true)),
            (Relocated::Val(Val::U8(1)), resource(OTHER, 3, true)),
        ]);
        assert_eq!(stranded_ids(vec![map]), [1, 2, 3]);
    }

    #[test]
    fn a_failed_injection_does_not_strand_a_borrow() {
        let values = vec![Relocated::Tuple(vec![resource(OTHER, 1, false), unknown()])];
        assert_eq!(stranded_ids(values), [0u64; 0]);
    }

    #[test]
    fn a_failed_injection_drops_the_resource_it_took_and_returns_the_one_it_borrowed() {
        let (mut store, id) = make_owner_store();
        let lent = Relocated::Tuple(vec![resource(ME, id, false), unknown()]);
        assert!(inject_all(store.as_context_mut(), vec![lent]).is_err());
        let registry = store.data_mut().resource_registry.as_mut().unwrap();
        assert!(!registry.has_pending_drops());
        assert!(registry.lend(id).is_some(), "the borrowed resource is kept");
        registry.returned(id);

        let taken = Relocated::Tuple(vec![resource(ME, id, true), unknown()]);
        assert!(inject_all(store.as_context_mut(), vec![taken]).is_err());
        let registry = store.data_mut().resource_registry.as_mut().unwrap();
        assert!(registry.has_pending_drops(), "nobody else holds it");
        assert!(registry.lend(id).is_none());
    }

    #[test]
    fn extract_preserves_fixed_length_list() {
        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_fixed_length_lists(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let wasm = wat::parse_str(
            r#"
                (component
                    (type $stream (stream u8))
                    (type $fixed (list $stream 1))
                    (type $func (func (param "value" $fixed)))
                    (import "f" (func (type $func)))
                )
            "#,
        )
        .unwrap();
        let component = Component::new(&engine, wasm).unwrap();
        let ComponentItem::ComponentFunc(func_ty) = component
            .component_type()
            .imports(&engine)
            .next()
            .unwrap()
            .1
            .ty
        else {
            panic!("expected function import");
        };
        let ty = func_ty.params().next().unwrap().1;

        let ctx = Ctx::builder("test-workload", "test-component").build();
        let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
        let (_consumer, producer, _done) = stream_pump::channel::<u8>(1);
        let reader = StreamReader::new(store.as_context_mut(), producer).unwrap();
        let stream = reader.try_into_stream_any(store.as_context_mut()).unwrap();
        let value = Val::FixedLengthList(vec![Val::Stream(stream)]);

        let relocated = extract(store.as_context_mut(), &value, &ty, &mut Vec::new()).unwrap();
        assert!(matches!(
            relocated,
            Relocated::FixedLengthList(values)
                if matches!(values.as_slice(), [Relocated::Stream(_)])
        ));
    }
}
