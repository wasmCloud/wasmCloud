//! Cross-store `resource` proxying for host component plugins.
//!
//! A `resource` handle is a capability into one store's table and cannot be
//! moved to another store by value. When a host component plugin (running in its
//! own store) returns `own<bucket>` to a workload, the host keeps the *real*
//! resource in the plugin store and hands the workload an opaque **proxy**; when
//! the workload calls a method on the proxy or drops it, the call routes back to
//! the real resource across the bridge.
//!
//! Two pieces make this work, wired through [`crate::engine::store::relocate`]:
//! - Each plugin store owns a [`ResourceRegistry`] keeping its handed-out real
//!   resources alive, keyed by a `proxy_id`.
//! - Each caller store holds a [`ProxyResource`] (host resource type) per proxy,
//!   carrying only the `proxy_id`.
//!
//! Relocation distinguishes the two sides by whether the store has a registry: a
//! plugin store (`Some`) registers/looks up reals; a caller store (`None`)
//! creates/reads proxies. Within a store, a proxy is told apart from a real
//! resource by its host [`wasmtime::component::ResourceType`].
//!
//! # Known limitations
//! - **Caller crash leaks reals.** A real is freed only when the caller's guest
//!   drops its proxy (which fires the proxy destructor and routes a drop) or when
//!   the plugin store is torn down (restart/stop drains the whole registry). A
//!   workload that stops or crashes still holding proxies does not fire those
//!   destructors, so its reals stay registered until the plugin next restarts.
//!   Reclaiming a specific caller's outstanding proxies on workload unbind needs
//!   per-caller tracking, which is tied to the deferred per-caller-identity work.
//! - **Shared proxy type.** All proxied resource kinds share one host type
//!   ([`proxy_resource_type`]). A well-typed guest can only return a handle of
//!   the kind it received, so kinds never mix; a hand-built adversarial guest
//!   that smuggles one kind's proxy into another kind's method is caught by the
//!   plugin-side `call_concurrent` type check (a trap, not memory corruption).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use wasmtime::component::{ResourceAny, ResourceType};

/// Process-global source of `proxy_id`s. Ids are never reused — not even across a
/// plugin restart, which builds a fresh registry — so a proxy a workload still
/// holds after its plugin restarted references an id that is simply absent from
/// the new registry (its method calls error and its drop is a no-op) rather than
/// aliasing an unrelated real resource in the new incarnation.
static NEXT_PROXY_ID: AtomicU64 = AtomicU64::new(0);

/// The host object a *caller* store keeps in its resource table for each proxied
/// resource. Opaque to the guest; `proxy_id` references the real resource in the
/// plugin store's [`ResourceRegistry`].
pub struct ProxyResource {
    pub proxy_id: u64,
}

/// The [`ResourceType`] every cross-store resource proxy uses. A single host
/// type backs all proxied resources — wasmtime allows several distinct resource
/// imports to share one host type, and a well-typed guest can only hand back a
/// handle of the type it received, so distinct resource kinds never mix.
pub fn proxy_resource_type() -> ResourceType {
    ResourceType::host::<ProxyResource>()
}

/// A plugin store's registry of the real resources it has handed out across the
/// bridge, keyed by `proxy_id`. Keeps each real [`ResourceAny`] alive until the
/// caller drops its proxy (or ownership is transferred back).
///
/// Dropping a *guest* resource runs its destructor and so requires top-level
/// async store access (`resource_drop_async`), which is unavailable from inside
/// the TriggerService's `run_concurrent` loop. Drops are therefore **staged** here when
/// a proxy is dropped and **flushed** by the TriggerService driver the moment it steps
/// out of `run_concurrent` (see [`crate::host::trigger_service`]).
#[derive(Default)]
pub struct ResourceRegistry {
    reals: BTreeMap<u64, ResourceAny>,
    pending_drops: Vec<ResourceAny>,
}

impl ResourceRegistry {
    /// Register a real resource and return its `proxy_id` (globally unique — see
    /// [`NEXT_PROXY_ID`]).
    pub fn register(&mut self, real: ResourceAny) -> u64 {
        let id = NEXT_PROXY_ID.fetch_add(1, Ordering::Relaxed);
        self.reals.insert(id, real);
        id
    }

    /// The real resource for `proxy_id`, if still registered (for a borrowing
    /// method call — leaves ownership in the registry).
    pub fn get(&self, proxy_id: u64) -> Option<ResourceAny> {
        self.reals.get(&proxy_id).copied()
    }

    /// Remove and return the real resource for `proxy_id` (for an ownership
    /// transfer back to the plugin guest).
    pub fn take(&mut self, proxy_id: u64) -> Option<ResourceAny> {
        self.reals.remove(&proxy_id)
    }

    /// Move `proxy_id`'s real resource to the pending-drop list (the caller
    /// dropped its proxy). Returns whether a resource was staged. A no-op for an
    /// already-gone id (idempotent).
    pub fn stage_drop(&mut self, proxy_id: u64) -> bool {
        if let Some(real) = self.reals.remove(&proxy_id) {
            self.pending_drops.push(real);
            true
        } else {
            false
        }
    }

    /// Whether any drops are staged and waiting to be flushed.
    pub fn has_pending_drops(&self) -> bool {
        !self.pending_drops.is_empty()
    }

    /// Take the staged drops to flush them (the TriggerService calls this with
    /// top-level store access, then `resource_drop_async`s each).
    pub fn take_pending_drops(&mut self) -> Vec<ResourceAny> {
        std::mem::take(&mut self.pending_drops)
    }

    /// Every real resource still owned by the plugin (registered or staged),
    /// draining the registry — used on store teardown to drop them all.
    pub fn drain_all(&mut self) -> Vec<ResourceAny> {
        let mut all: Vec<ResourceAny> = std::mem::take(&mut self.reals).into_values().collect();
        all.append(&mut self.pending_drops);
        all
    }
}
