//! Cross-store `resource` proxying.
//!
//! A `resource` handle is a capability into one store's table and cannot be
//! moved to another store by value. When the component that defines a resource
//! runs in a store of its own — a host component plugin, or a linked
//! component's companion (see [`crate::engine::companion`]) — the host keeps
//! the *real* resource in that store and hands every other store an opaque
//! **proxy**. A method call on the proxy, or its drop, is routed back to the
//! real resource across the bridge.
//!
//! Two pieces make this work, wired through [`crate::engine::store::relocate`]:
//! - Each owning store has a [`ResourceRegistry`] keeping its handed-out real
//!   resources alive, keyed by a `proxy_id`.
//! - Every other store holds a [`ProxyResource`] (host resource type) per
//!   proxy, carrying the `proxy_id` and whose it is.
//!
//! Relocation tells the sides apart by the store's registry: a store whose
//! registry belongs to a proxy's [`Owner`] registers and looks up reals; any
//! other creates and reads proxies. Within a store, a proxy is told apart from
//! a real resource by its host [`wasmtime::component::ResourceType`].
//!
//! # Known limitations
//! - **Caller crash leaks a plugin's reals.** A real is freed only when the
//!   caller's guest drops its proxy (which fires the proxy destructor and routes
//!   a drop) or when the owning store is torn down. A workload that stops or
//!   crashes still holding proxies for a plugin's resources does not fire those
//!   destructors, so the reals stay registered until the plugin next restarts.
//!   Reclaiming a specific caller's outstanding proxies on workload unbind needs
//!   per-caller tracking, which is tied to the deferred per-caller-identity work.
//! - **Shared proxy type.** All proxied resource kinds share one host type
//!   ([`proxy_resource_type`]). A well-typed guest can only return a handle of
//!   the kind it received, so kinds never mix; a hand-built adversarial guest
//!   that smuggles one kind's proxy into another kind's method is caught by the
//!   owner-side `call_concurrent` type check (a trap, not memory corruption).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Notify;

use wasmtime::component::{ResourceAny, ResourceType};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, Store, StoreContextMut};

use crate::engine::ctx::SharedCtx;

/// Process-global source of `proxy_id`s. Ids are never reused — not even across a
/// plugin restart, which builds a fresh registry — so a proxy a workload still
/// holds after its plugin restarted references an id that is simply absent from
/// the new registry (its method calls error and its drop is a no-op) rather than
/// aliasing an unrelated real resource in the new incarnation.
static NEXT_PROXY_ID: AtomicU64 = AtomicU64::new(0);

/// Whose store holds the real resource a proxy stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A host component plugin. Which one is fixed by the import a proxy is
    /// held under, so it is not recorded here.
    Plugin,
    /// The companion store of this linked component.
    Component(Arc<str>),
}

/// The host object a store keeps in its resource table for each resource that
/// lives in another. Opaque to the guest; `proxy_id` references the real
/// resource in its owner's [`ResourceRegistry`].
#[derive(Clone)]
pub struct ProxyResource {
    pub owner: Owner,
    pub proxy_id: u64,
}

/// The [`ResourceType`] every cross-store resource proxy uses. A single host
/// type backs all proxied resources — wasmtime allows several distinct resource
/// imports to share one host type, and a well-typed guest can only hand back a
/// handle of the type it received, so distinct resource kinds never mix.
pub fn proxy_resource_type() -> ResourceType {
    ResourceType::host::<ProxyResource>()
}

/// An owning store's registry of the real resources it has handed out across
/// the bridge, keyed by `proxy_id`. Keeps each real [`ResourceAny`] alive until
/// the last proxy for it is dropped (or ownership is transferred back).
///
/// Dropping a *guest* resource runs its destructor and so requires top-level
/// async store access (`resource_drop_async`), which is unavailable from inside
/// the store's `run_concurrent` loop. Drops are therefore **staged** here when
/// a proxy is dropped and **flushed** by the store's driver the moment it steps
/// out of `run_concurrent` (see [`flush_drops`]).
pub struct ResourceRegistry {
    owner: Owner,
    reals: BTreeMap<u64, ResourceAny>,
    /// How many calls in flight each resource is lent to as a `borrow`.
    lent: BTreeMap<u64, usize>,
    /// Resources whose last proxy was dropped while they were lent. They are
    /// staged once the last call borrowing them returns.
    drop_when_returned: BTreeSet<u64>,
    pending_drops: Vec<ResourceAny>,
    /// Signalled when a drop is staged by a call ending rather than by
    /// [`Self::stage_drop`], whose caller is the one that flushes.
    late_drops: Arc<Notify>,
}

impl ResourceRegistry {
    pub fn new(owner: Owner) -> Self {
        Self {
            owner,
            reals: BTreeMap::new(),
            lent: BTreeMap::new(),
            drop_when_returned: BTreeSet::new(),
            pending_drops: Vec::new(),
            late_drops: Arc::default(),
        }
    }

    /// Whose resources this registry holds.
    pub fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Register a real resource and return its `proxy_id` (globally unique — see
    /// [`NEXT_PROXY_ID`]).
    pub fn register(&mut self, real: ResourceAny) -> u64 {
        let id = NEXT_PROXY_ID.fetch_add(1, Ordering::Relaxed);
        self.reals.insert(id, real);
        id
    }

    /// The real resource for `proxy_id`, if still registered, lent to a call as
    /// a `borrow`: ownership stays here, and the resource cannot be dropped
    /// until [`Self::returned`] says the call is over.
    pub fn lend(&mut self, proxy_id: u64) -> Option<ResourceAny> {
        let real = self.reals.get(&proxy_id).copied()?;
        *self.lent.entry(proxy_id).or_default() += 1;
        Some(real)
    }

    /// A call that borrowed `proxy_id` has returned, which stages the drop
    /// its borrow had been holding back, if there was one.
    pub fn returned(&mut self, proxy_id: u64) {
        match self.lent.get_mut(&proxy_id) {
            Some(calls) if *calls > 1 => *calls -= 1,
            _ => {
                self.lent.remove(&proxy_id);
                if self.drop_when_returned.remove(&proxy_id) && self.stage_drop(proxy_id) {
                    self.late_drops.notify_one();
                }
            }
        }
    }

    /// Remove and return the real resource for `proxy_id` (for an ownership
    /// transfer back to the owning guest).
    pub fn take(&mut self, proxy_id: u64) -> Option<ResourceAny> {
        self.reals.remove(&proxy_id)
    }

    /// Move `proxy_id`'s real resource to the pending-drop list (its last proxy
    /// was dropped). Returns whether a resource was staged: not for an
    /// already-gone id, and not yet for one a call still borrows, whose
    /// destructor cannot run until that call returns.
    pub fn stage_drop(&mut self, proxy_id: u64) -> bool {
        if self.lent.contains_key(&proxy_id) {
            self.drop_when_returned.insert(proxy_id);
            return false;
        }
        if let Some(real) = self.reals.remove(&proxy_id) {
            self.pending_drops.push(real);
            true
        } else {
            false
        }
    }

    /// Stage a resource this registry no longer tracks: one taken out for a
    /// call that then never ran.
    pub(crate) fn stage_taken(&mut self, real: ResourceAny) {
        self.pending_drops.push(real);
        self.late_drops.notify_one();
    }

    /// Notified when a call ending has staged a drop, for the store's driver
    /// to flush it.
    pub(crate) fn late_drops(&self) -> Arc<Notify> {
        Arc::clone(&self.late_drops)
    }

    /// Whether any drops are staged and waiting to be flushed.
    pub fn has_pending_drops(&self) -> bool {
        !self.pending_drops.is_empty()
    }

    /// Take the staged drops to flush them (see [`flush_drops`]).
    pub fn take_pending_drops(&mut self) -> Vec<ResourceAny> {
        std::mem::take(&mut self.pending_drops)
    }

    /// Every real resource still owned here (registered or staged), draining
    /// the registry — used on store teardown to drop them all.
    pub fn drain_all(&mut self) -> Vec<ResourceAny> {
        let mut all: Vec<ResourceAny> = std::mem::take(&mut self.reals).into_values().collect();
        all.append(&mut self.pending_drops);
        all
    }
}

/// Free every resource whose last proxy was dropped since the previous flush,
/// running each guest destructor. Needs the store itself, which is unavailable
/// inside `run_concurrent`.
///
/// A destructor that fails is logged and the rest still run: a resource the
/// guest is still lending to a call cannot be dropped, and that is no reason to
/// stop serving everything else the store holds.
pub(crate) async fn flush_drops(store: &mut Store<SharedCtx>) {
    let pending = store
        .data_mut()
        .resource_registry
        .as_mut()
        .map(ResourceRegistry::take_pending_drops)
        .unwrap_or_default();
    for real in pending {
        if let Err(e) = real.resource_drop_async(&mut *store).await {
            tracing::warn!(err = %e, "failed to drop a proxied resource");
        }
    }
}

/// What rebuilding a call's arguments in a store lent to the guest for that
/// call, to be taken back when it returns.
#[derive(Default)]
pub(crate) struct Lent {
    /// Proxies made for `borrow` arguments.
    pub(crate) proxies: Vec<ResourceAny>,
    /// This store's own resources passed as `borrow` arguments.
    pub(crate) reals: Vec<u64>,
}

impl Lent {
    /// Take back what a call that has returned was lent.
    pub(crate) fn release(self, mut store: StoreContextMut<'_, SharedCtx>) -> wasmtime::Result<()> {
        if let Some(registry) = store.data_mut().resource_registry.as_mut() {
            for proxy_id in self.reals {
                registry.returned(proxy_id);
            }
        }
        for any in self.proxies {
            let proxy = any
                .try_into_resource::<ProxyResource>(store.as_context_mut())
                .context("a lent proxied resource was not returned")?;
            store.data_mut().table.delete(proxy)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use wasmtime::component::Resource;

    use super::*;

    fn registry_with_one() -> (ResourceRegistry, u64) {
        let mut store = Store::new(&wasmtime::Engine::default(), ());
        let real = Resource::<u32>::new_own(1)
            .try_into_resource_any(&mut store)
            .expect("a host resource converts");
        let mut registry = ResourceRegistry::new(Owner::Plugin);
        let id = registry.register(real);
        (registry, id)
    }

    #[test]
    fn a_drop_is_staged_at_once_when_nothing_borrows_the_resource() {
        let (mut registry, id) = registry_with_one();
        assert!(registry.stage_drop(id));
        assert!(registry.has_pending_drops());
        assert!(!registry.stage_drop(id), "a second drop finds nothing");
    }

    #[tokio::test]
    async fn a_drop_waits_for_the_last_call_borrowing_the_resource() {
        let (mut registry, id) = registry_with_one();
        let late_drops = registry.late_drops();
        assert!(registry.lend(id).is_some());
        assert!(registry.lend(id).is_some());

        assert!(!registry.stage_drop(id));
        assert!(
            registry.lend(id).is_some(),
            "still there for a call in flight"
        );
        registry.returned(id);
        registry.returned(id);
        assert!(!registry.has_pending_drops());

        registry.returned(id);
        assert!(registry.has_pending_drops());
        assert!(registry.lend(id).is_none());
        // The driver is told, as nothing else would have it flush.
        late_drops.notified().await;
    }

    #[test]
    fn a_borrow_returned_without_a_drop_leaves_the_resource() {
        let (mut registry, id) = registry_with_one();
        assert!(registry.lend(id).is_some());
        registry.returned(id);
        assert!(!registry.has_pending_drops());
        assert!(registry.lend(id).is_some());
    }
}
