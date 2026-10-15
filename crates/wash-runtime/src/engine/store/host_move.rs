//! Moving host resources between stores.
//!
//! A host resource is an entry in one store's resource table, and a handle to
//! it means nothing in another store. A linked call served from a store of its
//! own (see [`crate::engine::companion`]) can still carry one: the object
//! behind the handle is taken out of the sending store's table and put into
//! the receiving one's.
//!
//! An `own` goes for good. A `borrow` goes for the length of a sync call, while
//! its lender's store is stopped, and a stand-in keeps its slot, and so its
//! handle and its children, until it is back.
//!
//! Only the kinds in [`KINDS`] move. Each is whole in itself: one that refers
//! to another table entry by index, as a `pollable` does, would point at
//! nothing once moved.

use std::any::Any;
use std::sync::LazyLock;

use wasmtime::component::{Resource, ResourceAny, ResourceType};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut as _, StoreContextMut};
use wasmtime_wasi::p2::pipe::{ClosedInputStream, ClosedOutputStream};
use wasmtime_wasi_http::p2::bindings::http::types::Method;
use wasmtime_wasi_http::p2::body::{HostFutureTrailers, HostIncomingBody, HostOutgoingBody};
use wasmtime_wasi_http::p2::types::{
    HostFutureIncomingResponse, HostIncomingRequest, HostIncomingResponse, HostOutgoingRequest,
    HostOutgoingResponse, HostResponseOutparam,
};
use wasmtime_wasi_http::{FieldMap, RequestOptions};
use wasmtime_wasi_io::streams::{DynInputStream, DynOutputStream};

use crate::engine::ctx::SharedCtx;

type Object = Box<dyn Any + Send>;

/// How far a kind of host resource can go to another store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Reach {
    /// Handed over as an `own`.
    Own,
    /// Handed over as an `own`, or lent as a `borrow` to a sync call.
    OwnOrBorrow,
}

/// Takes one type of host resource out of a store's table and puts it into
/// another's.
trait Mover: Sync {
    fn resource_type(&self) -> ResourceType;

    fn reach(&self) -> Reach;

    fn take(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<Object>;

    /// The slot in `store`'s table of the resource `handle` borrows.
    fn slot(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<u32>;

    /// Take the object out of `slot` and leave a stand-in there.
    fn lend(&self, store: StoreContextMut<'_, SharedCtx>, slot: u32) -> wasmtime::Result<Object>;

    fn put(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        object: Object,
    ) -> wasmtime::Result<ResourceAny>;

    /// Put a lent object back in the slot its stand-in has been keeping.
    fn restore(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        slot: u32,
        object: Object,
    ) -> wasmtime::Result<()>;
}

struct Movable<T> {
    /// Makes what keeps a lent object's slot. A type without one is never lent.
    stand_in: Option<fn() -> T>,
}

fn unbox<T: 'static>(object: Object) -> wasmtime::Result<T> {
    match object.downcast::<T>() {
        Ok(object) => Ok(*object),
        Err(_) => wasmtime::bail!("a moved host resource is not the type it was moved as"),
    }
}

impl<T: Send + 'static> Mover for Movable<T> {
    fn resource_type(&self) -> ResourceType {
        ResourceType::host::<T>()
    }

    fn reach(&self) -> Reach {
        match self.stand_in {
            Some(_) => Reach::OwnOrBorrow,
            None => Reach::Own,
        }
    }

    fn take(
        &self,
        mut store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<Object> {
        let resource = handle.try_into_resource::<T>(store.as_context_mut())?;
        let object = store.data_mut().table.delete(resource).context(
            "a host resource cannot go to a linked component while a resource made from it, \
             such as a `pollable`, is still held",
        )?;
        Ok(Box::new(object))
    }

    fn slot(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<u32> {
        Ok(handle.try_into_resource::<T>(store)?.rep())
    }

    fn lend(
        &self,
        mut store: StoreContextMut<'_, SharedCtx>,
        slot: u32,
    ) -> wasmtime::Result<Object> {
        let stand_in = self
            .stand_in
            .context("this kind of host resource cannot be lent to a linked component")?;
        let kept = Resource::<T>::new_borrow(slot);
        let object = std::mem::replace(store.data_mut().table.get_mut(&kept)?, stand_in());
        Ok(Box::new(object))
    }

    fn put(
        &self,
        mut store: StoreContextMut<'_, SharedCtx>,
        object: Object,
    ) -> wasmtime::Result<ResourceAny> {
        let resource = store.data_mut().table.push(unbox::<T>(object)?)?;
        resource.try_into_resource_any(store)
    }

    fn restore(
        &self,
        mut store: StoreContextMut<'_, SharedCtx>,
        slot: u32,
        object: Object,
    ) -> wasmtime::Result<()> {
        let kept = Resource::<T>::new_borrow(slot);
        *store.data_mut().table.get_mut(&kept)? = unbox::<T>(object)?;
        Ok(())
    }
}

/// One kind of host resource that moves.
pub(crate) struct Kind {
    /// The interface that defines it, without its version.
    interface: &'static str,
    resource: &'static str,
    mover: &'static dyn Mover,
}

const fn kind<T: Send + 'static>(
    interface: &'static str,
    resource: &'static str,
    mover: &'static Movable<T>,
) -> Kind {
    Kind {
        interface,
        resource,
        mover,
    }
}

const IO_STREAMS: &str = "wasi:io/streams";
const HTTP_TYPES: &str = "wasi:http/types";

static KINDS: [Kind; 14] = [
    kind::<DynInputStream>(
        IO_STREAMS,
        "input-stream",
        &Movable {
            stand_in: Some(|| Box::new(ClosedInputStream)),
        },
    ),
    kind::<DynOutputStream>(
        IO_STREAMS,
        "output-stream",
        &Movable {
            stand_in: Some(|| Box::new(ClosedOutputStream)),
        },
    ),
    kind::<wasmtime_wasi_io::streams::Error>(
        "wasi:io/error",
        "error",
        &Movable {
            stand_in: Some(|| wasmtime::format_err!("lent to a linked component")),
        },
    ),
    kind::<FieldMap>(
        HTTP_TYPES,
        "fields",
        &Movable {
            stand_in: Some(FieldMap::default),
        },
    ),
    kind::<RequestOptions>(
        HTTP_TYPES,
        "request-options",
        &Movable {
            stand_in: Some(RequestOptions::default),
        },
    ),
    kind::<HostIncomingRequest>(HTTP_TYPES, "incoming-request", &Movable { stand_in: None }),
    kind::<HostResponseOutparam>(
        HTTP_TYPES,
        "response-outparam",
        &Movable {
            stand_in: Some(|| HostResponseOutparam {
                send: Box::new(|_| {}),
            }),
        },
    ),
    kind::<HostOutgoingResponse>(
        HTTP_TYPES,
        "outgoing-response",
        &Movable {
            stand_in: Some(|| HostOutgoingResponse {
                status: http::StatusCode::OK,
                headers: FieldMap::default(),
                body: None,
            }),
        },
    ),
    kind::<HostOutgoingRequest>(
        HTTP_TYPES,
        "outgoing-request",
        &Movable {
            stand_in: Some(|| HostOutgoingRequest {
                method: Method::Get,
                scheme: None,
                authority: None,
                path_with_query: None,
                headers: FieldMap::default(),
                body: None,
            }),
        },
    ),
    kind::<HostIncomingResponse>(
        HTTP_TYPES,
        "incoming-response",
        &Movable {
            stand_in: Some(|| HostIncomingResponse {
                status: 0,
                headers: FieldMap::default(),
                body: None,
            }),
        },
    ),
    kind::<HostFutureIncomingResponse>(
        HTTP_TYPES,
        "future-incoming-response",
        &Movable {
            stand_in: Some(|| HostFutureIncomingResponse::Consumed),
        },
    ),
    kind::<HostIncomingBody>(HTTP_TYPES, "incoming-body", &Movable { stand_in: None }),
    kind::<HostOutgoingBody>(HTTP_TYPES, "outgoing-body", &Movable { stand_in: None }),
    kind::<HostFutureTrailers>(HTTP_TYPES, "future-trailers", &Movable { stand_in: None }),
];

/// How far the resource `interface` defines as `resource` can go, when the host
/// serves that interface. `None` for one that stays in its store.
pub(crate) fn reach(interface: &str, resource: &str) -> Option<Reach> {
    let (name, version) = interface.split_once('@')?;
    KINDS
        .iter()
        .find(|kind| kind.interface == name && kind.resource == resource)
        .filter(|_| version.starts_with("0.2."))
        .map(|kind| kind.mover.reach())
}

/// How far `resource` of `wasi:io/streams` can go, for an interface that
/// re-exports it under a name of its own.
pub(crate) fn stream_reach(resource: &str) -> Option<Reach> {
    KINDS
        .iter()
        .find(|kind| kind.interface == IO_STREAMS && kind.resource == resource)
        .map(|kind| kind.mover.reach())
}

/// The type of handle each kind moves, in the order of [`KINDS`]. Resolved
/// once, as every handle leaving a store is looked up here.
static HANDLE_TYPES: LazyLock<[ResourceType; KINDS.len()]> =
    LazyLock::new(|| KINDS.each_ref().map(|kind| kind.mover.resource_type()));

/// The kind of host resource a handle of type `ty` is to, when it is one that
/// moves.
pub(crate) fn kind_of(ty: &ResourceType) -> Option<&'static Kind> {
    let at = HANDLE_TYPES.iter().position(|known| known == ty)?;
    KINDS.get(at)
}

impl Kind {
    /// Take the resource `handle` owns out of `store` for good.
    pub(crate) fn take(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<Carried> {
        Ok(Carried {
            mover: self.mover,
            object: self.mover.take(store, handle)?,
            slot: None,
        })
    }

    /// The slot in `store`'s table of the resource `handle` borrows.
    pub(crate) fn slot(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<u32> {
        self.mover.slot(store, handle)
    }

    /// Take the resource out of `slot` in `store` until [`restore`] puts it
    /// back. Nothing in `store` may use the resource in between, and a slot
    /// lent twice gives up only its stand-in the second time.
    pub(crate) fn lend(
        &self,
        store: StoreContextMut<'_, SharedCtx>,
        slot: u32,
    ) -> wasmtime::Result<Carried> {
        Ok(Carried {
            mover: self.mover,
            object: self.mover.lend(store, slot)?,
            slot: Some(slot),
        })
    }
}

/// A host resource out of its table, on its way to another store's.
pub struct Carried {
    mover: &'static dyn Mover,
    object: Object,
    /// For a `borrow`, the slot in its lender's table it goes back to.
    slot: Option<u32>,
}

/// Put back what a call that has returned was lent from `store`.
pub(crate) fn restore(
    mut store: StoreContextMut<'_, SharedCtx>,
    returned: Vec<Carried>,
) -> wasmtime::Result<()> {
    for carried in returned {
        let slot = carried
            .slot
            .context("a host resource that was not lent came back as one that was")?;
        carried
            .mover
            .restore(store.as_context_mut(), slot, carried.object)?;
    }
    Ok(())
}

impl Carried {
    /// Whether this goes back to its lender when the call it is on returns.
    pub(crate) fn lent(&self) -> bool {
        self.slot.is_some()
    }

    /// Put the resource into `store`'s table.
    pub(crate) fn land(self, store: StoreContextMut<'_, SharedCtx>) -> wasmtime::Result<Landed> {
        Ok(Landed {
            mover: self.mover,
            handle: self.mover.put(store, self.object)?,
            slot: self.slot,
        })
    }
}

/// A moved host resource in the table of the store it was moved to.
pub(crate) struct Landed {
    mover: &'static dyn Mover,
    handle: ResourceAny,
    slot: Option<u32>,
}

impl Landed {
    pub(crate) fn handle(&self) -> ResourceAny {
        self.handle
    }

    /// Take the resource out again, once the guest no longer holds its handle.
    pub(crate) fn take_back(
        self,
        store: StoreContextMut<'_, SharedCtx>,
    ) -> wasmtime::Result<Carried> {
        Ok(Carried {
            mover: self.mover,
            object: self.mover.take(store, self.handle)?,
            slot: self.slot,
        })
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use wasmtime::component::Resource;
    use wasmtime_wasi::p2::pipe::MemoryInputPipe;
    use wasmtime_wasi_io::poll::subscribe;
    use wasmtime_wasi_io::streams::{DynInputStream, StreamError};

    use super::*;
    use crate::engine::ctx::Ctx;

    fn make_store() -> wasmtime::Store<SharedCtx> {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).expect("engine");
        let ctx = Ctx::builder("workload", "component").build();
        wasmtime::Store::new(&engine, SharedCtx::new(ctx))
    }

    fn stream_of(
        store: &mut wasmtime::Store<SharedCtx>,
        contents: &'static str,
    ) -> Resource<DynInputStream> {
        let stream: DynInputStream = Box::new(MemoryInputPipe::new(Bytes::from(contents)));
        store.data_mut().table.push(stream).expect("pushed")
    }

    fn take(
        store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<Carried> {
        let kind = kind_of(&handle.ty()).expect("a kind that moves");
        kind.take(store, handle)
    }

    fn lend(
        mut store: StoreContextMut<'_, SharedCtx>,
        handle: ResourceAny,
    ) -> wasmtime::Result<Carried> {
        let kind = kind_of(&handle.ty()).expect("a kind that moves");
        let slot = kind.slot(store.as_context_mut(), handle)?;
        kind.lend(store, slot)
    }

    fn read(store: &mut wasmtime::Store<SharedCtx>, slot: u32) -> Result<Bytes, StreamError> {
        let stream = Resource::<DynInputStream>::new_borrow(slot);
        store
            .data_mut()
            .table
            .get_mut(&stream)
            .expect("a stream is in the slot")
            .read(16)
    }

    #[test]
    fn only_what_the_host_serves_at_0_2_moves() {
        assert_eq!(
            reach("wasi:io/streams@0.2.6", "input-stream"),
            Some(Reach::OwnOrBorrow)
        );
        assert_eq!(
            reach("wasi:http/types@0.2.2", "incoming-request"),
            Some(Reach::Own)
        );
        assert_eq!(reach("wasi:io/poll@0.2.6", "pollable"), None);
        assert_eq!(reach("wasi:io/streams@0.3.0", "input-stream"), None);
        assert_eq!(reach("wasi:io/streams", "input-stream"), None);
        assert_eq!(reach("example:io/streams@0.2.0", "input-stream"), None);
    }

    #[test]
    fn an_owned_resource_leaves_one_table_for_another() {
        let (mut from, mut to) = (make_store(), make_store());
        let handle = stream_of(&mut from, "hello")
            .try_into_resource_any(&mut from)
            .expect("a handle");
        let carried = take(from.as_context_mut(), handle).expect("taken");
        assert!(!carried.lent());
        assert!(from.data().table.is_empty());

        let landed = carried.land(to.as_context_mut()).expect("landed");
        let moved = landed
            .handle()
            .try_into_resource::<DynInputStream>(&mut to)
            .expect("the same type");
        assert_eq!(read(&mut to, moved.rep()).expect("read"), "hello");
    }

    #[test]
    fn a_lent_resource_comes_back_to_the_slot_and_children_it_had() {
        let (mut lender, mut borrower) = (make_store(), make_store());
        let stream = stream_of(&mut lender, "hello");
        let slot = stream.rep();
        // A child keeps the slot from being emptied, so the stream cannot
        // simply be removed and put back.
        let child = subscribe(
            &mut lender.data_mut().table,
            Resource::<DynInputStream>::new_borrow(slot),
        )
        .expect("subscribed");
        // An owned handle to the slot: wasmtime mints a borrowed one only
        // inside a call, and lending reads no more from it than the slot.
        let handle = Resource::<DynInputStream>::new_own(slot)
            .try_into_resource_any(&mut lender)
            .expect("a handle");

        let carried = lend(lender.as_context_mut(), handle).expect("lent");
        assert!(carried.lent());
        assert!(
            matches!(read(&mut lender, slot), Err(StreamError::Closed)),
            "a stand-in keeps the slot"
        );

        let landed = carried.land(borrower.as_context_mut()).expect("landed");
        let returned = landed.take_back(borrower.as_context_mut()).expect("back");
        assert!(borrower.data().table.is_empty());

        restore(lender.as_context_mut(), vec![returned]).expect("restored");
        assert_eq!(read(&mut lender, slot).expect("read"), "hello");
        lender.data_mut().table.delete(child).expect("the child");
        lender.data_mut().table.delete(stream).expect("the stream");
    }

    #[test]
    fn a_resource_with_a_child_is_not_handed_over() {
        let mut store = make_store();
        let stream = stream_of(&mut store, "hello");
        let slot = stream.rep();
        subscribe(
            &mut store.data_mut().table,
            Resource::<DynInputStream>::new_borrow(slot),
        )
        .expect("subscribed");
        let handle = stream.try_into_resource_any(&mut store).expect("a handle");

        assert!(take(store.as_context_mut(), handle).is_err());
        assert_eq!(read(&mut store, slot).expect("still there"), "hello");
    }

    #[test]
    fn a_kind_without_a_stand_in_is_not_lent() {
        let mut store = make_store();
        let slot = store.data_mut().table.push(7u32).expect("pushed").rep();
        let mover = Movable::<u32> { stand_in: None };

        assert_eq!(mover.reach(), Reach::Own);
        assert!(mover.lend(store.as_context_mut(), slot).is_err());
    }

    #[test]
    fn a_handle_is_matched_to_its_kind_by_type() {
        for (kind, ty) in KINDS.iter().zip(HANDLE_TYPES.iter()) {
            let found = kind_of(ty).expect("a kind that moves");
            assert_eq!(
                (found.interface, found.resource),
                (kind.interface, kind.resource)
            );
        }
        assert!(kind_of(&ResourceType::host::<u32>()).is_none());
    }
}
