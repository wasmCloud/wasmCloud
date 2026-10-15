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
//! route back. A handle to a host resource relocates as the object itself (see
//! [`host_move`]), where the call's route lets it.
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
    FutureAny, FutureReader, Lift, Lower, ResourceAny, ResourceType, StreamAny, StreamReader, Type,
    Val,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut as _, StoreContextMut};

use crate::engine::ctx::SharedCtx;
use crate::engine::store::host_move::{self, Carried, Landed};
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
    /// A handle to a host resource, carried as the object behind it.
    Host(Carried),
    /// Another `borrow` of a host resource already lent to the same call, by
    /// that resource's place among the ones lent.
    HostAgain(usize),
    List(Vec<Relocated>),
    FixedLengthList(Vec<Relocated>),
    Tuple(Vec<Relocated>),
    Record(Vec<(String, Relocated)>),
    Variant(String, Box<Relocated>),
    Option(Box<Relocated>),
    Result(Result<Box<Relocated>, Box<Relocated>>),
    Map(Vec<(Relocated, Relocated)>),
}

/// What a store does with a handle to one of its host resources when the value
/// holding it leaves for another store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostHandles {
    /// None may leave.
    Kept,
    /// An `own` hands its resource over.
    Moved,
    /// An `own` hands its resource over, and a `borrow` lends it until the
    /// call returns. Only for a sync call, which stops the store meanwhile.
    MovedOrLent,
}

/// Builds a `stream<T>`/`future<T>` value in the destination store from a
/// pre-wired producer (it captures the source side's pump endpoint).
pub(crate) type ValFactory =
    Box<dyn FnOnce(StoreContextMut<SharedCtx>) -> wasmtime::Result<Val> + Send>;

/// Set up a no-buffering pump for a `stream<T>`: `pipe` the source reader (in
/// `src`) into a channel, and return a factory that builds the destination
/// stream from the channel's producer, plus the pump's drain signal.
pub(crate) fn bridge_stream<T>(
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
pub(crate) fn bridge_future<T>(
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
                other => match src.data().stream_payloads.stream(other) {
                    Some(factory) => factory(src, any),
                    None => wasmtime::bail!(
                        "cross-store bridge: unregistered stream payload {other:?}; \
                         register it with `Engine::stream_payloads().register`"
                    ),
                },
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
                other => match src.data().stream_payloads.future(other) {
                    Some(factory) => factory(src, any),
                    None => wasmtime::bail!(
                        "cross-store bridge: unregistered future payload {other:?}; \
                         register it with `Engine::stream_payloads().register`"
                    ),
                },
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

/// What taking a call's values out of a store has done so far, kept so that a
/// failure part-way, at any depth, can be undone.
struct Extraction<'a> {
    host: HostHandles,
    dones: &'a mut Vec<Done>,
    /// The host resources lent so far, by handle type and slot.
    lent: Vec<(ResourceType, u32)>,
    /// What had left an aggregate before a failure cut it short.
    cut_short: Vec<Relocated>,
}

/// Relocate a `resource` handle across the boundary. A proxy has its owner and
/// `proxy_id` read out (the proxy is removed for an `own` transfer, left for a
/// `borrow`). A host resource goes as `cx.host` says. Anything else is a real
/// resource, which only the store of the component that defines it may send: it
/// is registered there — kept alive and reachable by later method calls and the
/// eventual drop.
fn extract_resource(
    mut store: StoreContextMut<SharedCtx>,
    any: ResourceAny,
    owned: bool,
    cx: &mut Extraction,
) -> wasmtime::Result<Relocated> {
    let ty = any.ty();
    if ty == resource_bridge::proxy_resource_type() {
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
    if let Some(kind) = host_move::kind_of(&ty) {
        return match (cx.host, owned) {
            (HostHandles::Moved | HostHandles::MovedOrLent, true) => {
                kind.take(store, any).map(Relocated::Host)
            }
            (HostHandles::MovedOrLent, false) => {
                let slot = kind.slot(store.as_context_mut(), any)?;
                match cx.lent.iter().position(|lent| *lent == (ty, slot)) {
                    Some(nth) => Ok(Relocated::HostAgain(nth)),
                    None => {
                        let carried = kind.lend(store, slot)?;
                        cx.lent.push((ty, slot));
                        Ok(Relocated::Host(carried))
                    }
                }
            }
            (HostHandles::Moved, false) => wasmtime::bail!(
                "cross-store bridge: a host resource can be lent only for a sync call"
            ),
            (HostHandles::Kept, _) => wasmtime::bail!(
                "cross-store bridge: a host resource cannot leave its store on this call"
            ),
        };
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
    /// Host resources put into this store's table for `own` handles.
    landed: Vec<Landed>,
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
        // Out of the table again and dropped, as its owner dropping it would.
        for landed in self.landed {
            let _ = landed.take_back(store.as_context_mut());
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

/// Rebuild a relocated handle to a host resource in `store`, whose table the
/// resource goes into.
fn inject_host(
    store: StoreContextMut<SharedCtx>,
    carried: Carried,
    done: &mut Injection,
) -> wasmtime::Result<Val> {
    let lent = carried.lent();
    let landed = carried.land(store)?;
    let handle = landed.handle();
    if lent {
        done.lent.hosts.push(landed);
    } else {
        done.landed.push(landed);
    }
    Ok(Val::Resource(handle))
}

/// Take each of `items` out of `store`. On failure the ones already out are
/// recorded in `cx`.
fn extract_each<'v, 't>(
    store: &mut StoreContextMut<SharedCtx>,
    items: impl Iterator<Item = (&'v Val, &'t Type)>,
    cx: &mut Extraction,
) -> wasmtime::Result<Vec<Relocated>> {
    let mut out = Vec::with_capacity(items.size_hint().0);
    for (val, ty) in items {
        match extract(store.as_context_mut(), val, ty, cx) {
            Ok(relocated) => out.push(relocated),
            Err(e) => {
                cx.cut_short.append(&mut out);
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// Extract a value from `store` (its origin), setting up a live channel pump for
/// each `stream<T>` (`reader.pipe` → no buffering) and pushing the pump's drain
/// signal into `cx.dones`. Handle-free values/subtrees are copied wholesale.
fn extract(
    mut store: StoreContextMut<SharedCtx>,
    val: &Val,
    ty: &Type,
    cx: &mut Extraction,
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
            cx.dones.push(done);
            Ok(Relocated::Stream(factory))
        }
        (Val::Future(any), Type::Future(ft)) => {
            let (factory, done) = match ft.ty() {
                Some(payload) => future_factory(store, any.clone(), &payload)?,
                None => bridge_future::<()>(store, any.clone())?,
            };
            cx.dones.push(done);
            Ok(Relocated::Future(factory))
        }
        (Val::Resource(any), Type::Own(_)) => extract_resource(store, *any, true, cx),
        (Val::Resource(any), Type::Borrow(_)) => extract_resource(store, *any, false, cx),
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
            let items = vs.iter().map(|v| (v, &et));
            Ok(Relocated::List(extract_each(&mut store, items, cx)?))
        }
        (Val::FixedLengthList(vs), Type::FixedLengthList(lt)) => {
            let et = lt.ty();
            let items = vs.iter().map(|v| (v, &et));
            Ok(Relocated::FixedLengthList(extract_each(
                &mut store, items, cx,
            )?))
        }
        (Val::Tuple(vs), Type::Tuple(tt)) => {
            let tys: Vec<Type> = tt.types().collect();
            let items = vs.iter().zip(&tys);
            Ok(Relocated::Tuple(extract_each(&mut store, items, cx)?))
        }
        (Val::Record(vs), Type::Record(rt)) => {
            let ftys: Vec<Type> = rt.fields().map(|f| f.ty).collect();
            let items = vs.iter().map(|(_, v)| v).zip(&ftys);
            let out = extract_each(&mut store, items, cx)?;
            let names = vs.iter().map(|(n, _)| n.clone());
            Ok(Relocated::Record(names.zip(out).collect()))
        }
        (Val::Option(Some(v)), Type::Option(ot)) => Ok(Relocated::Option(Box::new(extract(
            store.as_context_mut(),
            v,
            &ot.ty(),
            cx,
        )?))),
        (Val::Result(Ok(Some(v))), Type::Result(rt)) => {
            let it = rt.ok().ok_or_else(|| {
                wasmtime::format_err!("cross-store bridge: result `ok` is missing its type")
            })?;
            Ok(Relocated::Result(Ok(Box::new(extract(
                store.as_context_mut(),
                v,
                &it,
                cx,
            )?))))
        }
        (Val::Result(Err(Some(v))), Type::Result(rt)) => {
            let it = rt.err().ok_or_else(|| {
                wasmtime::format_err!("cross-store bridge: result `err` is missing its type")
            })?;
            Ok(Relocated::Result(Err(Box::new(extract(
                store.as_context_mut(),
                v,
                &it,
                cx,
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
                Box::new(extract(store.as_context_mut(), v, &case_ty, cx)?),
            ))
        }
        (Val::Map(es), Type::Map(mt)) => {
            let kt = mt.key();
            let vt = mt.value();
            let items = es.iter().flat_map(|(k, v)| [(k, &kt), (v, &vt)]);
            let mut flat = extract_each(&mut store, items, cx)?.into_iter();
            let mut out = Vec::with_capacity(es.len());
            while let (Some(k), Some(v)) = (flat.next(), flat.next()) {
                out.push((k, v));
            }
            Ok(Relocated::Map(out))
        }
        // A handle in a value/type mismatch (shouldn't happen for valid calls).
        _ => wasmtime::bail!("cross-store bridge: handle in an unexpected value/type position"),
    }
}

/// Undo taking `values` out of `store` for a call that will not be made, and
/// return what still owns a resource in another store.
fn give_back(mut store: StoreContextMut<SharedCtx>, mut values: Vec<Relocated>) -> Vec<Relocated> {
    let mut stranded = Vec::new();
    let mut lent = Vec::new();
    while let Some(value) = values.pop() {
        match value {
            Relocated::Resource {
                owner,
                proxy_id,
                owned: true,
            } => {
                let registry = store.data_mut().resource_registry.as_mut();
                match registry.filter(|registry| *registry.owner() == owner) {
                    // This store's own, registered only to leave: dropped, as
                    // the call it was bound for would have dropped it.
                    Some(registry) => {
                        if let Some(real) = registry.take(proxy_id) {
                            registry.stage_taken(real);
                        }
                    }
                    None => stranded.push(Relocated::Resource {
                        owner,
                        proxy_id,
                        owned: true,
                    }),
                }
            }
            Relocated::Host(carried) if carried.lent() => lent.push(carried),
            Relocated::List(inner)
            | Relocated::FixedLengthList(inner)
            | Relocated::Tuple(inner) => values.extend(inner),
            Relocated::Record(fields) => values.extend(fields.into_iter().map(|(_, v)| v)),
            Relocated::Variant(_, inner) | Relocated::Option(inner) => values.push(*inner),
            Relocated::Result(Ok(inner) | Err(inner)) => values.push(*inner),
            Relocated::Map(entries) => values.extend(entries.into_iter().flat_map(|(k, v)| [k, v])),
            // A pump stops when its far end is dropped, a host resource given
            // away is dropped as its new store would have dropped it, and a
            // `borrow` took nothing from the store.
            Relocated::Val(_)
            | Relocated::Stream(_)
            | Relocated::Future(_)
            | Relocated::Host(_)
            | Relocated::HostAgain(_)
            | Relocated::Resource { owned: false, .. } => {}
        }
    }
    let _ = host_move::restore(store, lent);
    stranded
}

/// Prepare every one of `vals` to leave `store`, as the arguments of a call
/// about to be made from it or the results of one that ran in it. `host` says
/// what becomes of a handle to a host resource, and the drain signal of each
/// stream/future pump set up is appended to `dones`.
///
/// Whether those signals are awaited or dropped is the caller's to decide: a
/// store about to be torn down has to wait for its result pumps to drain, one
/// that outlives the call does not.
///
/// On failure, at any depth, what had already left is put back or dropped, and
/// whatever it owned in another store is handed back as stranded.
pub(crate) fn extract_all(
    mut store: StoreContextMut<SharedCtx>,
    vals: &[Val],
    tys: &[Type],
    host: HostHandles,
    dones: &mut Vec<Done>,
) -> Result<Vec<Relocated>, RelocateError> {
    let mut cx = Extraction {
        host,
        dones,
        lent: Vec::new(),
        cut_short: Vec::new(),
    };
    match extract_each(&mut store, vals.iter().zip(tys), &mut cx) {
        Ok(out) => Ok(out),
        Err(error) => Err(RelocateError {
            error,
            stranded: give_back(store, cx.cut_short),
        }),
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
        Relocated::Host(carried) => inject_host(store, carried, done),
        Relocated::HostAgain(nth) => {
            let landed = done.lent.hosts.get(nth);
            let landed = landed.context("cross-store bridge: unknown lent host resource")?;
            Ok(Val::Resource(landed.handle()))
        }
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

/// Values that could not all be taken out of a store, or rebuilt in one.
pub(crate) struct RelocateError {
    pub(crate) error: wasmtime::Error,
    /// What still owns a resource somewhere else: the proxies already consumed
    /// or made, and the values never reached. Their owners have to be told to
    /// drop them.
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
) -> Result<(Vec<Val>, Lent), RelocateError> {
    let mut done = Injection::default();
    match inject_each(&mut store, values, &mut done) {
        Ok(vals) => Ok((vals, done.lent)),
        Err(error) => Err(RelocateError {
            error,
            stranded: done.undo(store),
        }),
    }
}

/// What a call that has run sends back to its caller's store.
pub(crate) struct Finished {
    pub(crate) results: Vec<Relocated>,
    /// The host resources the caller lent, to go back where they were.
    pub(crate) returned: Vec<Carried>,
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
    host: HostHandles,
) -> Result<Finished, RelocateError> {
    let returned = match lent.release(store.as_context_mut()) {
        Ok(returned) => returned,
        Err(error) => {
            return Err(RelocateError {
                error,
                stranded: Vec::new(),
            });
        }
    };
    let results = extract_all(store, results, result_tys, host, &mut Vec::new())?;
    Ok(Finished { results, returned })
}

#[cfg(test)]
mod tests {
    use super::{
        HostHandles, Relocated, contains_handle, extract_all, inject, inject_all, stream_pump,
    };
    use crate::engine::ctx::{Ctx, SharedCtx};
    use crate::engine::store::host_move;
    use crate::engine::store::resource_bridge::{Owner, ProxyResource, ResourceRegistry};
    use bytes::Bytes;
    use wasmtime::AsContextMut as _;
    use wasmtime::component::{Component, Resource, StreamReader, Type, Val, types::ComponentItem};
    use wasmtime_wasi::p2::pipe::MemoryInputPipe;
    use wasmtime_wasi_io::streams::DynInputStream;

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

        let relocated = extract_all(
            store.as_context_mut(),
            &[value],
            &[ty],
            HostHandles::Kept,
            &mut Vec::new(),
        )
        .map_err(|failed| failed.error)
        .unwrap();
        assert!(matches!(
            relocated.as_slice(),
            [Relocated::FixedLengthList(values)]
                if matches!(values.as_slice(), [Relocated::Stream(_)])
        ));
    }

    /// The parameter types of the one function `signature` imports, over a
    /// resource `$r`.
    fn param_types(signature: &str) -> Vec<Type> {
        let engine = wasmtime::Engine::default();
        let wasm = wat::parse_str(format!(
            r#"(component
                (import "r" (type $r (sub resource)))
                (import "f" (func {signature})))"#
        ))
        .unwrap();
        let component = Component::new(&engine, wasm).unwrap();
        let func_ty = component
            .component_type()
            .imports(&engine)
            .find_map(|(_, import)| match import.ty {
                ComponentItem::ComponentFunc(func_ty) => Some(func_ty),
                _ => None,
            })
            .unwrap();
        func_ty.params().map(|(_, ty)| ty).collect()
    }

    /// An `own` of a proxy for a resource another store owns.
    fn proxy(store: &mut wasmtime::Store<SharedCtx>, proxy_id: u64) -> Val {
        let proxy = ProxyResource {
            owner: Owner::Component(OTHER.into()),
            proxy_id,
        };
        let pushed = store.data_mut().table.push(proxy).unwrap();
        Val::Resource(pushed.try_into_resource_any(store).unwrap())
    }

    /// A handle to a resource of the store's own, which cannot leave a store
    /// without a registry, and cannot be lent from one that is a component's.
    fn local(store: &mut wasmtime::Store<SharedCtx>) -> Val {
        let local = Resource::<u32>::new_own(1);
        Val::Resource(local.try_into_resource_any(store).unwrap())
    }

    fn stranded_by_extraction(
        store: &mut wasmtime::Store<SharedCtx>,
        vals: &[Val],
        tys: &[Type],
        host: HostHandles,
    ) -> Vec<u64> {
        let failed = extract_all(store.as_context_mut(), vals, tys, host, &mut Vec::new())
            .err()
            .expect("the extraction fails");
        let mut ids = Vec::new();
        for value in failed.stranded {
            match value {
                Relocated::Resource {
                    proxy_id,
                    owned: true,
                    ..
                } => ids.push(proxy_id),
                _ => panic!("only owned resources are stranded"),
            }
        }
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_failed_extraction_gives_back_the_proxies_it_consumed_at_every_level() {
        let tys = param_types(
            r#"(param "a" (own $r))
               (param "b" (tuple (own $r) (list (own $r))))
               (param "c" (own $r))"#,
        );
        let mut store = make_store();
        let vals = [
            proxy(&mut store, 1),
            Val::Tuple(vec![
                proxy(&mut store, 2),
                Val::List(vec![proxy(&mut store, 3), local(&mut store)]),
            ]),
            proxy(&mut store, 4),
        ];
        let stranded = stranded_by_extraction(&mut store, &vals, &tys, HostHandles::Kept);
        // The one never reached is still in the store's table.
        assert_eq!(stranded, [1, 2, 3]);
    }

    #[test]
    fn a_failed_extraction_gives_back_what_a_lone_argument_nested() {
        let tys = param_types(r#"(param "a" (option (tuple (own $r) (own $r))))"#);
        let mut store = make_store();
        let pair = Val::Tuple(vec![proxy(&mut store, 1), local(&mut store)]);
        let vals = [Val::Option(Some(Box::new(pair)))];
        let stranded = stranded_by_extraction(&mut store, &vals, &tys, HostHandles::Kept);
        assert_eq!(stranded, [1]);
    }

    #[test]
    fn a_failed_extraction_drops_the_resource_its_store_registered() {
        let tys = param_types(r#"(param "a" (own $r)) (param "b" (borrow $r))"#);
        let mut store = make_store();
        store.data_mut().resource_registry =
            Some(ResourceRegistry::new(Owner::Component(ME.into())));
        let vals = [local(&mut store), local(&mut store)];

        let stranded = stranded_by_extraction(&mut store, &vals, &tys, HostHandles::Kept);
        assert_eq!(stranded, [0u64; 0], "nothing of another store's was held");
        let registry = store.data_mut().resource_registry.as_mut().unwrap();
        assert!(registry.has_pending_drops());
        assert!(
            registry.drain_all().len() == 1,
            "and nothing stays registered"
        );
    }

    fn stream_of(store: &mut wasmtime::Store<SharedCtx>, contents: &'static str) -> u32 {
        let stream: DynInputStream = Box::new(MemoryInputPipe::new(Bytes::from(contents)));
        store.data_mut().table.push(stream).unwrap().rep()
    }

    /// A handle to the stream in `slot`. Owned, as wasmtime mints a borrowed
    /// one only inside a call; lending reads no more from it than the slot.
    fn stream_handle(store: &mut wasmtime::Store<SharedCtx>, slot: u32) -> Val {
        let stream = Resource::<DynInputStream>::new_own(slot);
        Val::Resource(stream.try_into_resource_any(store).unwrap())
    }

    fn read(store: &mut wasmtime::Store<SharedCtx>, slot: u32) -> Option<Bytes> {
        let stream = Resource::<DynInputStream>::new_borrow(slot);
        let stream = store.data_mut().table.get_mut(&stream).unwrap();
        stream.read(16).ok()
    }

    #[test]
    fn a_failed_extraction_puts_back_the_host_resource_it_lent() {
        let tys = param_types(r#"(param "a" (borrow $r)) (param "b" (own $r))"#);
        let mut store = make_store();
        let slot = stream_of(&mut store, "hello");
        let vals = [stream_handle(&mut store, slot), local(&mut store)];

        let stranded = stranded_by_extraction(&mut store, &vals, &tys, HostHandles::MovedOrLent);
        assert_eq!(stranded, [0u64; 0]);
        assert_eq!(read(&mut store, slot).as_deref(), Some(&b"hello"[..]));
    }

    #[test]
    fn a_host_resource_borrowed_twice_by_one_call_is_lent_once() {
        let tys = param_types(r#"(param "a" (borrow $r)) (param "b" (list (borrow $r)))"#);
        let (mut lender, mut borrower) = (make_store(), make_store());
        let slot = stream_of(&mut lender, "hello");
        let vals = [
            stream_handle(&mut lender, slot),
            Val::List(vec![stream_handle(&mut lender, slot)]),
        ];

        let relocated = extract_all(
            lender.as_context_mut(),
            &vals,
            &tys,
            HostHandles::MovedOrLent,
            &mut Vec::new(),
        )
        .map_err(|failed| failed.error)
        .unwrap();
        assert!(
            read(&mut lender, slot).is_none(),
            "a stand-in keeps the slot"
        );

        let (landed, lent) = inject_all(borrower.as_context_mut(), relocated)
            .map_err(|failed| failed.error)
            .unwrap();
        let [Val::Resource(first), Val::List(again)] = landed.as_slice() else {
            panic!("a handle and a list of one");
        };
        assert_eq!(again.as_slice(), [Val::Resource(*first)]);

        let returned = lent.release(borrower.as_context_mut()).unwrap();
        assert_eq!(returned.len(), 1);
        host_move::restore(lender.as_context_mut(), returned).unwrap();
        assert_eq!(read(&mut lender, slot).as_deref(), Some(&b"hello"[..]));
    }
}
