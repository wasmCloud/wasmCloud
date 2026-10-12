//! Component execution context for wasmtime stores.
//!
//! This module provides the [`Ctx`] type which serves as the store context
//! for wasmtime when executing WebAssembly components. It integrates WASI
//! interfaces, HTTP capabilities, and plugin access into a unified context.

use std::{
    any::Any,
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use wasmtime::component::{Instance, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView};
#[cfg(feature = "wasi-tls")]
use wasmtime_wasi_tls::{WasiTlsCtx, WasiTlsCtxBuilder, WasiTlsCtxView, WasiTlsView};

use crate::host::allowed_hosts::AllowedHost;
use crate::plugin::HostPlugin;

/// A shareable, cloneable `wasi:tls` provider. Wraps an `Arc<dyn TlsProvider>`
/// so the same provider can back many per-component contexts without
/// re-creating it, and implements `TlsProvider` directly so it can be boxed
/// and passed to `WasiTlsCtxBuilder::provider`.
#[cfg(feature = "wasi-tls")]
#[derive(Clone)]
pub struct SharedTlsProvider(Arc<dyn wasmtime_wasi_tls::TlsProvider>);

#[cfg(feature = "wasi-tls")]
impl SharedTlsProvider {
    pub fn new(provider: impl wasmtime_wasi_tls::TlsProvider + 'static) -> Self {
        Self(Arc::new(provider))
    }
}

/// Concrete return type of [`wasmtime_wasi_tls::TlsProvider::connect`]. The
/// upstream alias (`BoxFutureTlsStream`) is `pub(crate)`, so we re-declare an
/// equivalent here to keep the impl signature readable.
#[cfg(feature = "wasi-tls")]
type TlsConnectFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<Box<dyn wasmtime_wasi_tls::TlsStream>, wasmtime_wasi_tls::Error>,
            > + Send,
    >,
>;

#[cfg(feature = "wasi-tls")]
impl wasmtime_wasi_tls::TlsProvider for SharedTlsProvider {
    fn connect(
        &self,
        server_name: String,
        transport: Box<dyn wasmtime_wasi_tls::TlsTransport>,
    ) -> TlsConnectFuture {
        self.0.connect(server_name, transport)
    }
}

/// Shared context for linked components
pub struct SharedCtx {
    /// The context of the component this store was built for (its root).
    ///
    /// It is never swapped. A host import is served with the context of the
    /// component whose instance made the import, and that is decided when the
    /// import is linked, not by store state: every component's linker carries
    /// getters bound to its own slot ([`CtxGetters`]). This field is what
    /// host-initiated paths read (the store id, the host link, an incoming
    /// request delivered to the root's export) and is that root's slot.
    pub root_ctx: Ctx,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// Contexts of the linked components instantiated into this store, by
    /// component slot ([`Ctx::slot`]). The root's slot is always `None` here;
    /// its context is [`Self::root_ctx`].
    linked: Vec<Option<Ctx>>,
    /// Which context the deprecated store-wide getters serve
    /// ([`extract_active_ctx`], [`extract_sockets`]). Nothing else reads it,
    /// and it goes away with them. See [`LegacySlot`].
    #[cfg(feature = "insecure-store-wide-ctx")]
    legacy: LegacySlot,
    /// Every plugin any context in this store carries, by id. For host-wide
    /// plugins reached through a store-level view rather than a slot getter
    /// (`wasi:webgpu`), which must be found whichever component bound them.
    pub(crate) store_plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
    /// Store-owned linked-component instances, keyed by component id.
    /// Dropping the store reclaims the instances without external cleanup.
    pub exporter_instances: HashMap<Arc<str>, Instance>,
    /// Present only on a store whose component hands its resources to other
    /// stores — a host component plugin, or a linked component's companion: the
    /// registry of the real resources behind the proxies those stores hold. See
    /// [`crate::engine::store::resource_bridge`].
    pub resource_registry: Option<crate::engine::store::resource_bridge::ResourceRegistry>,
    /// This store's linked companions and the resources shared with them. See
    /// [`crate::engine::companion`].
    pub(crate) links: crate::engine::companion::LinkState,
    /// The in-flight calls on this store whose dispatchers may abandon them;
    /// read by the store's epoch callback. See [`crate::engine::abandon`].
    pub abandoned: Arc<crate::engine::abandon::AbandonedCalls>,
    /// Guest execution on this store, sampled by the same epoch callback and
    /// reported by [`crate::observability::ExecutionTimeMeter`]. Read its docs
    /// before reading the number: it is a floor, not a total.
    pub executed: Arc<crate::engine::abandon::GuestExecution>,
    /// This store's share of the host's guest memory budget, installed on the
    /// store by [`crate::engine::guest_memory::install_memory_limiter`]. Living
    /// here is what makes the release exact: dropping the store drops this and
    /// hands the bytes back.
    pub memory_limiter: crate::engine::guest_memory::StoreMemoryLimiter,
    /// Notifies waiters after all other store data is dropped.
    ///
    /// Wasmtime disposes guest fibers and host tasks before dropping store data.
    pub dropped: StoreDropped,
}

/// Lazily creates a token and cancels it when dropped.
#[derive(Default)]
pub struct StoreDropped(std::sync::OnceLock<tokio_util::sync::CancellationToken>);

impl StoreDropped {
    /// Returns a token canceled when this value drops.
    pub fn token(&self) -> tokio_util::sync::CancellationToken {
        self.0
            .get_or_init(tokio_util::sync::CancellationToken::new)
            .clone()
    }
}

impl Drop for StoreDropped {
    fn drop(&mut self) {
        if let Some(token) = self.0.get() {
            token.cancel();
        }
    }
}

/// The identity of whoever is invoking a host component plugin, used to
/// partition state per caller.
///
/// `component_id` is `None` when there is no component behind the call: a
/// `wasmcloud:host/workload-lifecycle` hook is delivered by the host about a
/// whole workload, not on behalf of any one of its items. Encoding that as a
/// real id — an empty string, say — would make the host invent a component that
/// does not exist, and would collide with a genuinely unresolvable caller.
#[derive(Clone, Debug)]
pub struct CallerIdentity {
    pub workload_id: Arc<str>,
    pub component_id: Option<Arc<str>>,
    /// The binding the call arrived on: the `(implements ..)` label the caller
    /// imported the plugin's interface under, which is also the name the
    /// operator declared it by. `None` for a plain import, and for a lifecycle
    /// hook, which is about a whole workload rather than one binding.
    ///
    /// Read by `wasmcloud:host/identity#get-binding-name`, so a plugin serving
    /// two bindings of one workload can pair a call with the configuration that
    /// binding was resolved with.
    pub binding: Option<Arc<str>>,
}

impl SharedCtx {
    pub fn new(context: Ctx) -> Self {
        Self {
            store_plugins: context.plugins.clone(),
            #[cfg(feature = "insecure-store-wide-ctx")]
            legacy: LegacySlot::new(context.slot),
            root_ctx: context,
            table: ResourceTable::new(),
            linked: Vec::new(),
            exporter_instances: Default::default(),
            resource_registry: None,
            links: Default::default(),
            abandoned: Arc::default(),
            executed: Arc::default(),
            memory_limiter: Default::default(),
            dropped: StoreDropped::default(),
        }
    }

    /// Marks this store as a host-component-plugin store, enabling it to keep
    /// real resources alive as it hands proxies across the bridge.
    pub fn with_resource_registry(mut self) -> Self {
        self.resource_registry = Some(
            crate::engine::store::resource_bridge::ResourceRegistry::new(
                crate::engine::store::resource_bridge::Owner::Plugin,
            ),
        );
        self
    }

    /// Draws this store's linear memory from `budget`. Without this the store
    /// is neither charged nor limited.
    pub fn with_guest_memory(
        mut self,
        budget: &Arc<crate::engine::guest_memory::GuestMemoryBudget>,
    ) -> Self {
        self.memory_limiter = budget.limiter();
        self
    }

    /// Adds the context of a linked component instantiated into this store,
    /// at its slot. A slot can hold one context: two components of one store
    /// sharing a slot would make their imports indistinguishable.
    pub fn insert_linked_ctx(&mut self, ctx: Ctx) -> anyhow::Result<()> {
        let slot = ctx.slot;
        anyhow::ensure!(
            slot < MAX_COMPONENT_SLOTS,
            "component slot {slot} is out of range (a workload holds at most \
             {MAX_WORKLOAD_COMPONENTS} components)"
        );
        anyhow::ensure!(
            slot != self.root_ctx.slot,
            "linked component '{}' has the root component's slot {slot}",
            ctx.component_id
        );
        if self.linked.len() <= slot {
            self.linked.resize_with(slot + 1, || None);
        }
        match self.linked.get_mut(slot) {
            Some(entry @ None) => {
                for (id, plugin) in &ctx.plugins {
                    self.store_plugins
                        .entry(id)
                        .or_insert_with(|| Arc::clone(plugin));
                }
                *entry = Some(ctx);
                Ok(())
            }
            _ => anyhow::bail!("component slot {slot} is already taken in this store"),
        }
    }

    /// The slot of the component `component_id` in this store, if it is here.
    #[cfg(feature = "insecure-store-wide-ctx")]
    fn slot_of(&self, component_id: &str) -> Option<usize> {
        if self.root_ctx.component_id.as_ref() == component_id {
            return Some(self.root_ctx.slot);
        }
        self.linked
            .iter()
            .flatten()
            .find(|ctx| ctx.component_id.as_ref() == component_id)
            .map(|ctx| ctx.slot)
    }

    /// Points the deprecated store-wide getters at `component_id` for the
    /// duration of one linked call and returns a token for
    /// [`Self::restore_legacy_slot`]. A component not in this store is an
    /// error, as it was for the old `set_active_ctx`.
    #[cfg(feature = "insecure-store-wide-ctx")]
    pub(crate) fn swap_legacy_slot(&mut self, component_id: &str) -> wasmtime::Result<u64> {
        let slot = self.slot_of(component_id).ok_or_else(|| {
            wasmtime::format_err!("Context for component {component_id} not found")
        })?;
        Ok(self.legacy.push(slot))
    }

    /// Ends the linked call `token` started.
    #[cfg(feature = "insecure-store-wide-ctx")]
    pub(crate) fn restore_legacy_slot(&mut self, token: u64) {
        self.legacy.remove(token);
    }

    /// Points the deprecated store-wide getters at `component_id` until it is
    /// called again, as the old `set_active_ctx` did. Linked calls in flight
    /// still take precedence while they last.
    ///
    /// Affects only plugins wired with [`extract_active_ctx`] or
    /// [`extract_sockets`]; the engine and migrated plugins never read it.
    #[cfg(feature = "insecure-store-wide-ctx")]
    #[deprecated(
        since = "2.11.0",
        note = "only the deprecated store-wide getters read this; plugins wired with \
                `item.ctx_getter()` are always served with their own component's context"
    )]
    pub fn set_active_ctx(&mut self, component_id: &Arc<str>) -> wasmtime::Result<()> {
        let slot = self.slot_of(component_id).ok_or_else(|| {
            wasmtime::format_err!("Context for component {component_id} not found")
        })?;
        self.legacy.base = slot;
        Ok(())
    }

    /// The table and the context the deprecated store-wide getters serve.
    #[cfg(feature = "insecure-store-wide-ctx")]
    fn split_legacy(&mut self) -> (&mut ResourceTable, &mut Ctx) {
        let slot = self.legacy.current();
        self.split_at(slot)
    }

    /// The context of the component at `slot` in this store, if one is here.
    #[cfg(test)]
    pub(crate) fn ctx_at(&mut self, slot: usize) -> Option<&mut Ctx> {
        if slot == self.root_ctx.slot {
            Some(&mut self.root_ctx)
        } else {
            self.linked.get_mut(slot).and_then(Option::as_mut)
        }
    }

    /// The resource table (shared by every component in the store, so handles
    /// keep their identity across a linked call) and the context of the
    /// component at `slot`, borrowed together.
    ///
    /// # Panics
    ///
    /// If no component at `slot` was instantiated into this store. That cannot
    /// happen through the engine: a getter for a slot is only ever wired into
    /// that component's own linker, and a store only instantiates components
    /// whose contexts it holds. An embedder that builds its own store
    /// ([`SharedCtx::new`]) and instantiates a component the engine linked
    /// must give the store that component's context: a component linked for
    /// slot N, instantiated into a store whose only context is slot 0, panics
    /// on its first host call. The getter is a plain `fn` with no way to
    /// return an error, and serving the call with some other component's
    /// context instead would be the very misattribution slots exist to rule
    /// out.
    #[allow(clippy::panic)]
    fn split_at(&mut self, slot: usize) -> (&mut ResourceTable, &mut Ctx) {
        let ctx = if slot == self.root_ctx.slot {
            &mut self.root_ctx
        } else {
            match self.linked.get_mut(slot).and_then(Option::as_mut) {
                Some(ctx) => ctx,
                None => panic!("no component context for slot {slot} in this store"),
            }
        };
        (&mut self.table, ctx)
    }
}

/// Which context the deprecated store-wide getters serve: the callee of the
/// newest linked call still in flight, or `base` (the root, unless the
/// deprecated `set_active_ctx` moved it) when none is.
///
/// This reproduces the old swap for sequential calls. Under overlapping calls
/// it is still wrong mid-flight, which is why those getters are deprecated,
/// but it cannot be left stuck: each call removes its own entry, so once no
/// linked call is in flight the getters are back on `base`, where the old
/// last-in, first-out restore could leave a pooled store on a callee for good.
#[cfg(feature = "insecure-store-wide-ctx")]
struct LegacySlot {
    base: usize,
    in_flight: Vec<(u64, usize)>,
    next_token: u64,
}

#[cfg(feature = "insecure-store-wide-ctx")]
impl LegacySlot {
    fn new(base: usize) -> Self {
        Self {
            base,
            in_flight: Vec::new(),
            next_token: 0,
        }
    }

    fn push(&mut self, slot: usize) -> u64 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        self.in_flight.push((token, slot));
        token
    }

    fn remove(&mut self, token: u64) {
        if let Some(i) = self.in_flight.iter().rposition(|(t, _)| *t == token) {
            self.in_flight.remove(i);
        }
    }

    fn current(&self) -> usize {
        self.in_flight.last().map_or(self.base, |(_, slot)| *slot)
    }
}

/// How many distinct context slots a store can address: slot 0 for a
/// workload's service and one for each component after it, so a workload
/// holds at most `MAX_COMPONENT_SLOTS - 1` (255) components. Each slot has its
/// own monomorphized getters ([`CtxGetters`]), because Wasmtime's
/// `add_to_linker` takes a plain `fn` pointer, which cannot capture which
/// component it serves.
pub const MAX_COMPONENT_SLOTS: usize = 256;

/// The most components one workload may hold: every slot but the service's.
pub const MAX_WORKLOAD_COMPONENTS: usize = MAX_COMPONENT_SLOTS - 1;

/// The getter a host plugin passes to a generated `add_to_linker`, bound to the
/// component whose linker it is wired into. Obtain it from
/// [`crate::engine::workload::WorkloadItem::ctx_getter`].
pub type CtxGetter = for<'a> fn(&'a mut SharedCtx) -> ActiveCtx<'a>;

fn active_at<const N: usize>(ctx: &mut SharedCtx) -> ActiveCtx<'_> {
    let (table, ctx) = ctx.split_at(N);
    ActiveCtx { table, ctx }
}

fn sockets_at<const N: usize>(ctx: &mut SharedCtx) -> crate::sockets::WasiSocketsCtxView<'_> {
    let (table, ctx) = ctx.split_at(N);
    crate::sockets::WasiSocketsCtxView {
        ctx: &mut ctx.sockets,
        table,
    }
}

fn wasi_at<const N: usize>(ctx: &mut SharedCtx) -> WasiCtxView<'_> {
    let (table, ctx) = ctx.split_at(N);
    WasiCtxView {
        ctx: &mut ctx.ctx,
        table,
    }
}

fn cli_at<const N: usize>(ctx: &mut SharedCtx) -> wasmtime_wasi::cli::WasiCliCtxView<'_> {
    let WasiCtxView { ctx, table } = wasi_at::<N>(ctx);
    wasmtime_wasi::cli::WasiCliCtxView {
        ctx: ctx.cli(),
        table,
    }
}

fn clocks_at<const N: usize>(ctx: &mut SharedCtx) -> wasmtime_wasi::clocks::WasiClocksCtxView<'_> {
    let WasiCtxView { ctx, table } = wasi_at::<N>(ctx);
    wasmtime_wasi::clocks::WasiClocksCtxView {
        ctx: ctx.clocks(),
        table,
    }
}

fn filesystem_at<const N: usize>(
    ctx: &mut SharedCtx,
) -> wasmtime_wasi::filesystem::WasiFilesystemCtxView<'_> {
    let WasiCtxView { ctx, table } = wasi_at::<N>(ctx);
    wasmtime_wasi::filesystem::WasiFilesystemCtxView {
        ctx: ctx.filesystem(),
        table,
    }
}

fn random_at<const N: usize>(ctx: &mut SharedCtx) -> &mut wasmtime_wasi::random::WasiRandomCtx {
    ctx.split_at(N).1.ctx.random()
}

fn http_at<const N: usize>(ctx: &mut SharedCtx) -> WasiHttpCtxView<'_> {
    let (table, ctx) = ctx.split_at(N);
    WasiHttpCtxView {
        ctx: &mut ctx.http,
        table,
        hooks: &mut ctx.http_hooks,
    }
}

/// Every context getter the engine wires into one component's linker, all
/// bound to that component's slot. Copy, so a linker-building function can
/// take it by value.
#[derive(Clone, Copy)]
pub struct CtxGetters {
    pub active: CtxGetter,
    pub sockets: for<'a> fn(&'a mut SharedCtx) -> crate::sockets::WasiSocketsCtxView<'a>,
    pub cli: for<'a> fn(&'a mut SharedCtx) -> wasmtime_wasi::cli::WasiCliCtxView<'a>,
    pub clocks: for<'a> fn(&'a mut SharedCtx) -> wasmtime_wasi::clocks::WasiClocksCtxView<'a>,
    pub filesystem:
        for<'a> fn(&'a mut SharedCtx) -> wasmtime_wasi::filesystem::WasiFilesystemCtxView<'a>,
    pub random: for<'a> fn(&'a mut SharedCtx) -> &'a mut wasmtime_wasi::random::WasiRandomCtx,
    pub http: for<'a> fn(&'a mut SharedCtx) -> WasiHttpCtxView<'a>,
}

macro_rules! slot_getters {
    ($($n:literal)*) => {
        const ALL_SLOTS: [CtxGetters; MAX_COMPONENT_SLOTS] = [$(CtxGetters {
            active: active_at::<$n>,
            sockets: sockets_at::<$n>,
            cli: cli_at::<$n>,
            clocks: clocks_at::<$n>,
            filesystem: filesystem_at::<$n>,
            random: random_at::<$n>,
            http: http_at::<$n>,
        }),*];
    };
}

slot_getters!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
    32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
    64 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95
    96 97 98 99 100 101 102 103 104 105 106 107 108 109 110 111 112 113 114 115 116 117 118 119 120 121 122 123 124 125 126 127
    128 129 130 131 132 133 134 135 136 137 138 139 140 141 142 143 144 145 146 147 148 149 150 151 152 153 154 155 156 157 158 159
    160 161 162 163 164 165 166 167 168 169 170 171 172 173 174 175 176 177 178 179 180 181 182 183 184 185 186 187 188 189 190 191
    192 193 194 195 196 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218 219 220 221 222 223
    224 225 226 227 228 229 230 231 232 233 234 235 236 237 238 239 240 241 242 243 244 245 246 247 248 249 250 251 252 253 254 255
);

impl CtxGetters {
    /// The getters for the component at `slot`.
    pub fn for_slot(slot: usize) -> anyhow::Result<Self> {
        ALL_SLOTS.get(slot).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "component slot {slot} is out of range (a workload holds at most \
                 {MAX_WORKLOAD_COMPONENTS} components)"
            )
        })
    }
}

/// The store-wide context getter from before per-component contexts.
///
/// It serves the root's context, or a linked callee's for the duration of a
/// linked call, through a single slot every call in the store shares. Under
/// concurrent linked calls that slot can name the wrong component, so a
/// plugin wired with this can serve one component's imports with another's
/// identity, config and grants. Insecure: available only with the
/// `insecure-store-wide-ctx` feature, for plugins that have not moved to the
/// item's getters yet.
#[deprecated(
    since = "2.11.0",
    note = "serves imports from a store-wide slot that concurrent linked calls can \
            point at the wrong component; pass the item's getter instead: \
            `let (linker, getter) = item.linker_and_getter();`"
)]
#[cfg(feature = "insecure-store-wide-ctx")]
pub fn extract_active_ctx(ctx: &mut SharedCtx) -> ActiveCtx<'_> {
    let (table, ctx) = ctx.split_legacy();
    ActiveCtx { table, ctx }
}

/// The store-wide sockets view from before per-component contexts. See
/// [`extract_active_ctx`] for why it is deprecated.
#[deprecated(
    since = "2.11.0",
    note = "reads a store-wide slot that concurrent linked calls can point at the \
            wrong component; use the item's own sockets getter instead: \
            `item.ctx_getters().sockets`"
)]
#[cfg(feature = "insecure-store-wide-ctx")]
pub fn extract_sockets(ctx: &mut SharedCtx) -> crate::sockets::WasiSocketsCtxView<'_> {
    let (table, ctx) = ctx.split_legacy();
    crate::sockets::WasiSocketsCtxView {
        ctx: &mut ctx.sockets,
        table,
    }
}

impl wasmtime::component::HasData for SharedCtx {
    type Data<'a> = ActiveCtx<'a>;
}

pub struct ActiveCtx<'a> {
    pub table: &'a mut wasmtime::component::ResourceTable,
    pub ctx: &'a mut Ctx,
}

impl<'a> Deref for ActiveCtx<'a> {
    type Target = Ctx;

    fn deref(&self) -> &Self::Target {
        self.ctx
    }
}

impl<'a> DerefMut for ActiveCtx<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx
    }
}

/// The context for a component store and linker, providing access to implementations of:
/// - wasi@0.2 interfaces
/// - wasi:http@0.2 interfaces
pub struct Ctx {
    /// Unique identifier for this component context. This is a [uuid::Uuid::now_v7] string.
    pub id: Arc<str>,
    /// Unique identifier shared by all component contexts in the same store.
    pub store_id: Arc<str>,
    /// The unique identifier for the workload component this instance belongs to
    pub component_id: Arc<str>,
    /// The unique identifier for the workload this component belongs to
    pub workload_id: Arc<str>,
    /// This component's slot in its workload: which [`CtxGetters`] its linker
    /// was wired with, and so which context in a store serves its imports.
    pub(crate) slot: usize,
    /// The WASI context used to provide WASI functionality to the components using this context.
    pub ctx: WasiCtx,
    /// The HTTP context used to provide HTTP functionality to the component.
    pub http: WasiHttpCtx,
    /// The sockets context used to provide socket functionality (with loopback support).
    pub sockets: crate::sockets::WasiSocketsCtx,
    /// The TLS Context used to provide TLS over the HTTP functionality to the component.
    #[cfg(feature = "wasi-tls")]
    pub tls: WasiTlsCtx,
    /// Plugin instances stored by string ID for access during component execution.
    /// These all implement the [`HostPlugin`] trait, but they are cast as `Arc<dyn Any + Send + Sync>`
    /// to support downcasting to the specific plugin type in [`Ctx::get_plugin`]
    plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
    /// The HTTP hooks for outgoing HTTP requests.
    http_hooks: CtxHttpHooks,
}

impl Ctx {
    /// Get a plugin by its string ID and downcast to the expected type
    ///
    /// **Panics** if the plugin is not found or does not match the expected type.
    #[allow(clippy::expect_used)] // Infallible accessor by contract; callers needing fallibility use `try_get_plugin`
    pub fn get_plugin<T: HostPlugin + 'static>(&self, plugin_id: &str) -> Arc<T> {
        self.try_get_plugin::<T>(plugin_id)
            .expect("plugin not found")
    }

    /// Get a reference to a plugin by its string ID and downcast to the expected type
    ///
    /// Unlike [`Ctx::get_plugin`], this borrows the plugin from `self` instead of
    /// cloning the `Arc`, allowing callers to return references tied to `&self`.
    ///
    /// **Panics** if the plugin is not found or does not match the expected type.
    #[allow(clippy::expect_used)] // Infallible accessor by contract, mirrors `get_plugin`
    pub fn get_plugin_ref<T: HostPlugin + 'static>(&self, plugin_id: &str) -> &T {
        self.plugins
            .get(plugin_id)
            .expect("plugin not found")
            .downcast_ref::<T>()
            .expect("failed to downcast plugin to expected type")
    }

    /// Get a plugin by its string ID and downcast to the expected type, if it exists
    pub fn try_get_plugin<T: HostPlugin + 'static>(
        &self,
        plugin_id: &str,
    ) -> wasmtime::Result<Arc<T>> {
        self.plugins
            .get(plugin_id)
            .ok_or_else(|| wasmtime::format_err!("plugin {plugin_id} not found"))?
            .clone()
            .downcast::<T>()
            .map_err(|_| {
                wasmtime::format_err!(
                    "failed to downcast plugin to type {}",
                    std::any::type_name::<T>()
                )
            })
    }

    /// Create a new [`CtxBuilder`] to construct a [`Ctx`]
    pub fn builder(
        workload_id: impl Into<Arc<str>>,
        component_id: impl Into<Arc<str>>,
    ) -> CtxBuilder {
        CtxBuilder::new(workload_id, component_id)
    }

    /// This store's link back to the host that built it, for readers that need
    /// to know whether that host is still there; see [`HostLink`].
    pub(crate) fn host_link(&self) -> HostLink {
        self.http_hooks.host.clone()
    }
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("id", &self.id)
            .field("workload_id", &self.workload_id.as_ref())
            .finish()
    }
}

impl wasmtime_wasi_io::IoView for SharedCtx {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

/// The ROOT component's `wasi:http` view. Used only by host-initiated paths
/// that serve the store's own root (delivering an incoming request to its
/// handler export). Never wire it into a linker: a guest's own `wasi:http`
/// imports use its slot getter ([`CtxGetters::http`]), or a linked callee's
/// outbound requests would be judged against the root's `allowedHosts`.
///
/// Changed meaning: this used to follow the store-wide swap. A plugin that
/// sends outbound requests on a component's behalf through `SharedCtx::http()`
/// now always gets the root's policy, even when a linked callee called it.
/// Such a plugin must take `item.ctx_getters().http` instead. It stays a trait
/// impl because wasmtime-wasi-http's incoming path requires one.
impl WasiHttpView for SharedCtx {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.root_ctx.http,
            table: &mut self.table,
            hooks: &mut self.root_ctx.http_hooks,
        }
    }
}

/// The ROOT component's `wasi:tls` view, the one interface still linked
/// through a store-wide view (upstream keeps its `HasData` type private). It
/// reaches only the TLS provider, which the engine installs identically on
/// every context; where a connection may go is the per-slot socket check's
/// decision.
#[cfg(feature = "wasi-tls")]
impl WasiTlsView for SharedCtx {
    fn tls(&mut self) -> WasiTlsCtxView<'_> {
        WasiTlsCtxView {
            ctx: &mut self.root_ctx.tls,
            table: &mut self.table,
        }
    }
}

/// A store's link back to the host that built it, held weak; see
/// [`CtxBuilder::with_host`].
#[derive(Clone, Default)]
pub(crate) struct HostLink(Option<crate::host::HostRef>);

impl HostLink {
    /// The handler to serve this store's egress with, or why there is none. A
    /// store with no host at all and a store whose host has gone away under it
    /// are different failures, and the guest is told which.
    fn handler(&self) -> Result<Arc<dyn crate::host::http::HostHandler>, String> {
        match &self.0 {
            None => Err("http client not available".to_string()),
            Some(host) if !host.has_handler() => Err("http client not available".to_string()),
            Some(host) => crate::host::http::live_handler(host).map_err(|e| format!("{e:#}")),
        }
    }

    /// Whether the host that built this store is gone, asked of the host's own
    /// lifetime rather than of its handler: an embedder may hold a clone of the
    /// handler long after the host is dropped, and a store that outlived its
    /// host has to be ended either way. A store built without a host answers
    /// `false` — never pointed at one, it can say nothing about one.
    pub(crate) fn host_is_gone(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(crate::host::HostRef::host_is_gone)
    }
}

/// HTTP hooks that delegate outgoing requests to the configured
/// [`HostHandler`](crate::host::http::HostHandler), so custom egress
/// (allowed-hosts policy, alternate transports, etc.) applies to `wasi:http`
/// 0.2 and 0.3 components alike.
struct CtxHttpHooks {
    host: HostLink,
    workload_id: Arc<str>,
    allowed_hosts: Arc<[AllowedHost]>,
    /// Set once this store has reported that its egress cannot be served, so a
    /// guest that keeps calling out does not repeat the warning per call.
    warned_unserved: bool,
}

impl WasiHttpHooks for CtxHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::WasiBody>,
        options: Option<wasmtime_wasi_http::RequestOptions>,
        fut: crate::host::http::RequestIoFuture,
    ) -> crate::host::http::SendFuture {
        match self.host.handler() {
            Ok(handler) => handler.outgoing_request(
                &self.workload_id,
                request,
                options,
                fut,
                &self.allowed_hosts,
            ),
            Err(message) => {
                // The hook cannot trap, so the guest gets a handleable error
                // and the host gets told once. A guest left calling out to a
                // host that is gone is ended by the store's epoch deadline
                // instead; see [`crate::engine::abandon::arm_epoch_deadline`].
                if !self.warned_unserved {
                    self.warned_unserved = true;
                    tracing::warn!(
                        workload_id = %self.workload_id,
                        %message,
                        "a component called out over HTTP with no handler to serve it"
                    );
                }
                Box::new(
                    async move { Err(wasmtime_wasi_http::Error::InternalError(Some(message))) },
                )
            }
        }
    }
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    id: Arc<str>,
    store_id: Arc<str>,
    workload_id: Arc<str>,
    component_id: Arc<str>,
    slot: usize,
    ctx: Option<WasiCtx>,
    sockets: Option<crate::sockets::WasiSocketsCtx>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    http_handler: Option<crate::host::HostRef>,
    allowed_hosts: Arc<[AllowedHost]>,
    /// TLS provider override for `wasi:tls` client connections.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl CtxBuilder {
    pub fn new(workload_id: impl Into<Arc<str>>, component_id: impl Into<Arc<str>>) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string().into(),
            store_id: uuid::Uuid::now_v7().to_string().into(),
            component_id: component_id.into(),
            workload_id: workload_id.into(),
            slot: 0,
            ctx: None,
            sockets: None,
            http_handler: None,
            plugins: HashMap::new(),
            allowed_hosts: Default::default(),
            #[cfg(feature = "wasi-tls")]
            tls_provider: None,
        }
    }

    /// Override the TLS provider used for `wasi:tls` client connections.
    ///
    /// Use this to plug in an alternative TLS backend, install a custom root
    /// certificate store (corporate CAs, certificate pinning), or integrate
    /// with HSM-backed key material.
    #[cfg(feature = "wasi-tls")]
    pub fn with_tls_provider(mut self, provider: SharedTlsProvider) -> Self {
        self.tls_provider = Some(provider);
        self
    }

    /// The component's slot in its workload ([`Ctx::slot`]). Defaults to 0,
    /// which is right for a store holding a single component.
    pub fn with_slot(mut self, slot: usize) -> Self {
        self.slot = slot;
        self
    }

    /// Set a custom [WasiCtx]
    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    pub fn with_sockets(mut self, sockets: crate::sockets::WasiSocketsCtx) -> Self {
        self.sockets = Some(sockets);
        self
    }

    /// Point this store back at the host that built it: its outgoing HTTP goes
    /// through that host's handler, and it is ended if that host is torn down
    /// while it still runs (see
    /// [`crate::engine::abandon::arm_epoch_deadline`]).
    ///
    /// Weak, and taken weak: building a store must not depend on the host
    /// still being there, because a store is built on paths — a message
    /// delivery, a service restart — that have nothing to do with egress. Only
    /// an actual outbound call needs a live handler, and the hooks report it
    /// per call. See [`crate::host::http::live_handler`].
    pub fn with_host(mut self, host: &crate::host::HostRef) -> Self {
        self.http_handler = Some(host.clone());
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) -> Self {
        self.plugins.extend(plugins);
        self
    }

    pub fn with_allowed_hosts(mut self, allowed_hosts: Arc<[AllowedHost]>) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub fn build(self) -> Ctx {
        let plugins = self
            .plugins
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn Any + Send + Sync>))
            .collect();

        let http_hooks = CtxHttpHooks {
            host: HostLink(self.http_handler),
            workload_id: self.workload_id.clone(),
            allowed_hosts: self.allowed_hosts,
            warned_unserved: false,
        };

        Ctx {
            id: self.id,
            store_id: self.store_id,
            slot: self.slot,
            ctx: self.ctx.unwrap_or_else(|| {
                WasiCtxBuilder::new()
                    .args(&["main.wasm"])
                    .inherit_stderr()
                    .build()
            }),
            workload_id: self.workload_id,
            component_id: self.component_id,
            http: WasiHttpCtx::new(),
            sockets: self.sockets.unwrap_or_default(),
            #[cfg(feature = "wasi-tls")]
            tls: {
                // EngineBuilder::build already warns once if no provider was
                // set; just fall through to the wasmtime-wasi-tls default
                // here when none is provided.
                let mut builder = WasiTlsCtxBuilder::new();
                if let Some(provider) = self.tls_provider {
                    builder = builder.provider(Box::new(provider));
                }
                builder.build()
            },
            plugins,
            http_hooks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();
    }

    #[test]
    fn ctx_builder_sets_ids() {
        setup();
        let ctx = Ctx::builder("wk-1", "comp-1").build();
        assert_eq!(ctx.workload_id.as_ref(), "wk-1");
        assert_eq!(ctx.component_id.as_ref(), "comp-1");
    }

    #[test]
    fn ctx_builder_generates_uuid_id() {
        setup();
        let ctx = Ctx::builder("wk", "comp").build();
        // id and store_id should be valid UUID v7 strings
        let id = uuid::Uuid::parse_str(&ctx.id).expect("id is a UUID");
        let store_id = uuid::Uuid::parse_str(&ctx.store_id).expect("store_id is a UUID");
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(store_id.get_version_num(), 7);
    }

    #[test]
    fn ctx_builder_uses_default_wasi_ctx_when_none_provided() {
        setup();
        // Should not panic — proves default WasiCtx is created
        let _ctx = Ctx::builder("wk", "comp").build();
    }

    #[test]
    fn shared_ctx_new_sets_active_ctx() {
        setup();
        let ctx = Ctx::builder("wk", "comp-a").build();
        let shared = SharedCtx::new(ctx);
        assert_eq!(shared.root_ctx.component_id.as_ref(), "comp-a");
        assert!(shared.linked.is_empty());
    }

    fn two_slot_store() -> SharedCtx {
        let ctx_a = Ctx::builder("wk", "comp-a").with_slot(1).build();
        let ctx_b = Ctx::builder("wk", "comp-b").with_slot(3).build();
        let mut shared = SharedCtx::new(ctx_a);
        shared.insert_linked_ctx(ctx_b).unwrap();
        shared
    }

    /// Each slot's getter reaches that slot's context, however often and in
    /// whatever order the others are used: there is no shared "current" slot
    /// to leave behind.
    #[test]
    fn slot_getters_resolve_by_slot_not_by_history() {
        setup();
        let mut shared = two_slot_store();
        let a = CtxGetters::for_slot(1).unwrap();
        let b = CtxGetters::for_slot(3).unwrap();
        for _ in 0..3 {
            assert_eq!((b.active)(&mut shared).component_id.as_ref(), "comp-b");
            assert_eq!((a.active)(&mut shared).component_id.as_ref(), "comp-a");
            assert_eq!((b.active)(&mut shared).component_id.as_ref(), "comp-b");
        }
        assert_eq!(shared.root_ctx.component_id.as_ref(), "comp-a");
    }

    /// The deprecated store-wide getters keep their old behavior for
    /// sequential calls: the root's context, a linked callee's while a call
    /// is in flight, the root's again after. Unknown ids are an error.
    #[test]
    #[allow(deprecated)]
    #[cfg(feature = "insecure-store-wide-ctx")]
    fn deprecated_getters_follow_the_legacy_swap() {
        setup();
        let mut shared = two_slot_store();
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-a"
        );

        let call = shared.swap_legacy_slot("comp-b").unwrap();
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-b"
        );
        let _ = extract_sockets(&mut shared);
        // Per-slot getters ignore the swap.
        let a = CtxGetters::for_slot(1).unwrap();
        assert_eq!((a.active)(&mut shared).component_id.as_ref(), "comp-a");

        assert!(shared.swap_legacy_slot("not-here").is_err());
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-b"
        );

        shared.restore_legacy_slot(call);
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-a"
        );
        assert_eq!(shared.root_ctx.component_id.as_ref(), "comp-a");

        // `set_active_ctx` moves the base; in-flight calls still win.
        shared.set_active_ctx(&Arc::from("comp-b")).unwrap();
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-b"
        );
        let call = shared.swap_legacy_slot("comp-a").unwrap();
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-a"
        );
        shared.restore_legacy_slot(call);
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-b"
        );
        assert!(shared.set_active_ctx(&Arc::from("not-here")).is_err());

        // Still fits wherever a plugin passes a getter, so existing
        // `add_to_linker(linker, extract_active_ctx)` calls build unchanged.
        let _: CtxGetter = extract_active_ctx;
    }

    /// Two overlapping calls finished first-in, first-out left the old
    /// last-in, first-out restore on the callee for good. Each call now
    /// removes its own entry, so the getters return to the root.
    #[test]
    #[allow(deprecated)]
    #[cfg(feature = "insecure-store-wide-ctx")]
    fn overlapping_calls_cannot_leave_the_legacy_slot_stuck() {
        setup();
        let mut shared = two_slot_store();
        let first = shared.swap_legacy_slot("comp-b").unwrap();
        let second = shared.swap_legacy_slot("comp-b").unwrap();
        shared.restore_legacy_slot(first);
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-b"
        );
        shared.restore_legacy_slot(second);
        assert_eq!(
            extract_active_ctx(&mut shared).component_id.as_ref(),
            "comp-a"
        );
    }

    #[test]
    fn insert_linked_ctx_refuses_a_taken_or_root_slot() {
        setup();
        let mut shared = two_slot_store();
        let dup = Ctx::builder("wk", "comp-c").with_slot(3).build();
        assert!(shared.insert_linked_ctx(dup).is_err());
        let root = Ctx::builder("wk", "comp-d").with_slot(1).build();
        assert!(shared.insert_linked_ctx(root).is_err());
        let far = Ctx::builder("wk", "comp-e")
            .with_slot(MAX_COMPONENT_SLOTS)
            .build();
        assert!(shared.insert_linked_ctx(far).is_err());
    }

    #[test]
    fn ctx_at_and_for_slot_bounds() {
        setup();
        let mut shared = two_slot_store();
        assert!(shared.ctx_at(2).is_none());
        assert_eq!(shared.ctx_at(3).unwrap().component_id.as_ref(), "comp-b");
        assert!(CtxGetters::for_slot(MAX_COMPONENT_SLOTS).is_err());
        assert!(CtxGetters::for_slot(MAX_COMPONENT_SLOTS - 1).is_ok());
    }

    /// A store an embedder built for itself must never be ended by
    /// [`crate::engine::abandon::arm_epoch_deadline`]'s host check: it was
    /// never pointed at a host, so it can say nothing about one. The host side
    /// of this contract is
    /// `a_store_sees_its_host_go_away_though_a_handler_clone_remains`.
    #[test]
    fn a_store_built_without_a_host_reports_none_gone() {
        setup();
        assert!(
            !Ctx::builder("wk", "comp")
                .build()
                .host_link()
                .host_is_gone(),
            "a store with no host must not report one gone"
        );
    }
}
