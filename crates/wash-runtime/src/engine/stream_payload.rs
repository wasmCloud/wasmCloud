//! Typed payload adapters for cross-store streams and futures.
//!
//! Wasmtime requires `Lift`/`Lower` implementations to read a stream's items.
//! Embedders can register their generated WIT types here without making the
//! runtime depend on an application's WIT package. Scalar and string payloads
//! are supported without registration.

use std::sync::RwLock;

use wasmtime::{
    StoreContextMut,
    component::{FutureAny, Lift, Lower, StreamAny, Type},
};

use super::{
    ctx::SharedCtx,
    store::{
        relocate::{self, ValFactory},
        stream_pump::Done,
    },
};

type StreamFactory =
    fn(StoreContextMut<SharedCtx>, StreamAny) -> wasmtime::Result<(ValFactory, Done)>;
type FutureFactory =
    fn(StoreContextMut<SharedCtx>, FutureAny) -> wasmtime::Result<(ValFactory, Done)>;

struct Payload {
    ty: Type,
    stream: StreamFactory,
    future: FutureFactory,
}

/// Engine-local adapters shared by its workload and component-plugin stores.
#[derive(Default)]
pub struct StreamPayloads(RwLock<Vec<Payload>>);

impl StreamPayloads {
    /// Register a generated Rust type for a WIT payload taken from a compiled
    /// component's `StreamType::ty()` or `FutureType::ty()`.
    ///
    /// Register before resolving workloads or loading component plugins. The
    /// payload may contain records, variants, lists, options and results, but
    /// no store-bound handles. Wasmtime checks that `T` matches the source
    /// payload before starting each transfer; a mismatched registration errors
    /// instead of copying a value under the wrong type.
    pub fn register<T>(&self, ty: Type) -> anyhow::Result<()>
    where
        T: Lift + Lower + Send + Sync + 'static,
    {
        anyhow::ensure!(
            !super::value::carries_cross_store_handle(&ty),
            "stream/future payload adapters cannot carry store-bound handles"
        );
        let mut payloads = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        anyhow::ensure!(
            !payloads.iter().any(|entry| entry.ty == ty),
            "payload type already registered"
        );
        payloads.push(Payload {
            ty,
            stream: relocate::bridge_stream::<T>,
            future: relocate::bridge_future::<T>,
        });
        Ok(())
    }

    pub(crate) fn supports(&self, ty: &Type) -> bool {
        relocate::bridgeable_element_type(ty)
            || self
                .0
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|entry| entry.ty == *ty)
    }

    pub(crate) fn stream(&self, ty: &Type) -> Option<StreamFactory> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| entry.ty == *ty)
            .map(|entry| entry.stream)
    }

    pub(crate) fn future(&self, ty: &Type) -> Option<FutureFactory> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| entry.ty == *ty)
            .map(|entry| entry.future)
    }
}
