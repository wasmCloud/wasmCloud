//! What enforces [`HostMemoryBudgets::max_guest_memory`].
//!
//! The budget names a total; wasmtime's own knobs bound a *single* linear
//! memory ([`HostMemoryBudgets::default_heap_memory`]) and a *count* of pool
//! slots ([`HostMemoryBudgets::core_instances`]). Neither adds up to the total:
//! `core_instances x default_heap_memory` bounds address space, which the
//! kernel does not back until it is written to. The case nothing covers is
//! aggregate creep — N guests each under its own ceiling, together over the
//! pod's — and the first notification is the OOM killer taking every workload
//! on the host with it.
//!
//! This module closes that. A [`GuestMemoryBudget`] is one counter of guest
//! bytes for the whole host. Every store carries a [`StoreMemoryLimiter`]
//! installed through [`wasmtime::Store::limiter`], which sees each
//! `memory.grow` before it happens, charges the growth to the shared counter,
//! and can refuse it.
//!
//! # Count before Enforce
//!
//! [`GuestMemoryMode::Count`] is the default and is a no-op by construction: it
//! charges and reports, and allows the growth regardless. This matters because
//! `max_guest_memory` is always *derived* when unset — never absent — so a host
//! that switched straight to enforcement would gain a ceiling it has never had,
//! on upgrade, with nobody having asked for one. Count preserves
//! [`crate::engine::host_memory`]'s property that nothing changes when the
//! flags are unset, while still producing the number no host has today: a
//! high-water mark for aggregate guest memory, which is what answers "one
//! runaway component, or aggregate creep?".
//!
//! Same shape as [`crate::sockets::policy::EgressMode`], for the same reason.
//!
//! # What a refusal looks like to a guest
//!
//! Returning `Ok(false)` from [`wasmtime::ResourceLimiter::memory_growing`]
//! makes `memory.grow` return `-1`. That is an ordinary WebAssembly outcome an
//! allocator is written to handle, not a trap, and it is already what a guest
//! sees on hitting `default_heap_memory`. The exception is a memory's *initial*
//! size, requested during instantiation: wasmtime has no `-1` to return there,
//! so a refusal surfaces as an instantiation error.
//!
//! # This is growth, not RSS
//!
//! The limiter sees requested growth. The pooling allocator commits lazily, so
//! a guest that grows its heap and touches none of it is charged for pages the
//! kernel has not backed. That overcounts against RSS, which is the safe
//! direction for a budget, but it means this number is an upper bound on
//! resident guest memory rather than a measurement of it.
//!
//! # A charge is taken before the growth is known to have succeeded
//!
//! [`wasmtime::ResourceLimiter`] offers no callback that says "the growth you
//! permitted went through". `memory_grow_failed` looks like one and is not: it
//! also fires for growth the limiter was never consulted about, so refunding
//! against it would let a guest alternate a real growth with an oversized one
//! and have its own charges written off. Instead every ceiling wasmtime is
//! going to apply is checked *here* first — see
//! [`GuestMemoryBudget::with_heap_ceiling`] for the one the limiter is not
//! told about — so the growth that gets charged is the growth that happens.
//!
//! What is left over is the host genuinely running out: an `mmap` that fails
//! under real memory pressure. That charge is held until the store is dropped,
//! which makes the budget refuse sooner than it strictly must — the safe
//! direction, and a host in that state has larger problems.
//!
//! # A plugin draws on the same budget as the workloads that call it
//!
//! A host component plugin's store is long-lived and shared by every workload
//! importing its capability, and it is charged here like any other guest. So
//! under [`GuestMemoryMode::Enforce`] workloads can fill the budget and leave
//! the plugin unable to grow — and a Rust guest that cannot allocate aborts,
//! which takes the capability away from every workload using it, not just the
//! one that filled the budget. Reserving a plugin's share is the same missing
//! piece as a per-workload sub-budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::host_memory::{HostMemoryBudgets, render_bytes};

/// Share of the budget in use past which the periodic report is worth an
/// operator's attention rather than a debug line.
const PRESSURE_NUMERATOR: u64 = 3;
const PRESSURE_DENOMINATOR: u64 = 4;

/// How strictly the guest memory budget is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GuestMemoryMode {
    /// Charge the growth, record that the budget would have refused it, and
    /// allow it anyway. The default, so upgrading a host does not hand every
    /// guest a ceiling nobody asked for.
    #[default]
    Count,
    /// Refuse growth past the budget.
    Enforce,
}

impl GuestMemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Enforce => "enforce",
        }
    }
}

/// One counter of guest linear-memory bytes, shared by every store on a host.
///
/// Held by the [`Engine`] and cloned into each store's
/// [`StoreMemoryLimiter`]. Charges accumulate as guests grow their heaps and
/// are returned when the store that made them is dropped.
///
/// [`Engine`]: crate::engine::Engine
#[derive(Debug)]
pub struct GuestMemoryBudget {
    cap: u64,
    mode: GuestMemoryMode,
    /// The pooling allocator's per-slot ceiling, when one is installed.
    ///
    /// Not a budget knob — a correction. wasmtime hands the limiter the
    /// *memory type's* maximum, which for a Rust component that declares none
    /// is 4 GiB; the pool's own per-slot ceiling is applied later, and a
    /// growth past it fails after the limiter already said yes. Charging that
    /// growth would hold bytes the guest never got for the life of the store,
    /// so the ceiling has to be known here to refuse it first.
    heap_ceiling: Option<u64>,
    in_use: AtomicU64,
    high_water: AtomicU64,
    refused: AtomicU64,
    would_refuse: AtomicU64,
    /// Whether the cap has ever been crossed, so the first crossing can be
    /// reported loudly and the rest quietly. A host that has crept past its
    /// budget is worth one warning, not one per `memory.grow`.
    crossed: AtomicBool,
    /// Refusals as of the last [`Self::report`], so a report can say whether
    /// anything has been refused *since* rather than ever.
    reported_refusals: AtomicU64,
}

impl Default for GuestMemoryBudget {
    /// An unmetered budget: accounted but never crossed. What a store built
    /// without an engine's budget gets, so a limiter is always installable.
    fn default() -> Self {
        Self::new(u64::MAX, GuestMemoryMode::Count)
    }
}

impl GuestMemoryBudget {
    pub fn new(cap: u64, mode: GuestMemoryMode) -> Self {
        Self {
            cap,
            mode,
            heap_ceiling: None,
            in_use: AtomicU64::new(0),
            high_water: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            would_refuse: AtomicU64::new(0),
            crossed: AtomicBool::new(false),
            reported_refusals: AtomicU64::new(0),
        }
    }

    /// Tell the budget the per-slot ceiling the pooling allocator was actually
    /// built with, so growth the pool will refuse is never charged.
    ///
    /// Read back from the installed pool rather than from the resolved knobs:
    /// an environment override or a caller-supplied pooling config both move
    /// it, and a ceiling that is too high here reintroduces the overcharge.
    #[must_use]
    pub fn with_heap_ceiling(mut self, heap_ceiling: u64) -> Self {
        self.heap_ceiling = Some(heap_ceiling);
        self
    }

    /// The budget a host's resolved knobs describe.
    pub fn from_budgets(budgets: &HostMemoryBudgets, mode: GuestMemoryMode) -> Self {
        Self::new(budgets.max_guest_memory, mode)
    }

    /// A limiter drawing on this budget, to be installed on one store.
    pub fn limiter(self: &Arc<Self>) -> StoreMemoryLimiter {
        StoreMemoryLimiter {
            budget: Some(Arc::clone(self)),
            charged: 0,
        }
    }

    pub fn cap(&self) -> u64 {
        self.cap
    }

    pub fn mode(&self) -> GuestMemoryMode {
        self.mode
    }

    /// Guest bytes currently charged.
    pub fn in_use(&self) -> u64 {
        self.in_use.load(Ordering::Relaxed)
    }

    /// The most that has ever been charged at once — the figure that says
    /// whether this host is near its budget, and the whole point of running in
    /// [`GuestMemoryMode::Count`].
    pub fn high_water(&self) -> u64 {
        self.high_water.load(Ordering::Relaxed)
    }

    /// Growths refused under [`GuestMemoryMode::Enforce`].
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Growths [`GuestMemoryMode::Enforce`] would have refused, counted while
    /// running in [`GuestMemoryMode::Count`].
    pub fn would_refuse(&self) -> u64 {
        self.would_refuse.load(Ordering::Relaxed)
    }

    /// Publish this budget as OpenTelemetry metrics.
    ///
    /// *Observable* instruments: the exporter calls back on its own schedule
    /// and reads the atomics this budget already keeps, so nothing is recorded
    /// on the `memory.grow` path and the cost of being watched is zero.
    ///
    /// Not gated behind `--enable-meters`, unlike
    /// [`crate::observability::FuelConsumptionMeter`]. That flag exists because
    /// fuel metering makes the guest measurably slower; this does not, and the
    /// high-water figure is the one an operator is told to watch before turning
    /// enforcement on — putting it behind an opt-in would hide the number the
    /// rollout depends on. With no OTel exporter configured the global meter is
    /// a no-op and none of these callbacks are ever invoked.
    fn register_metrics(self: &Arc<Self>, meter: &opentelemetry::metrics::Meter) {
        let mode = [opentelemetry::KeyValue::new("mode", self.mode.as_str())];

        // Weak, so a dropped engine's budget is not kept alive by the meter
        // provider — the same reason the epoch ticker holds a weak engine. A
        // callback that cannot upgrade reports nothing and the series ends.
        macro_rules! observe {
            ($build:ident, $name:literal, $unit:literal, $doc:literal, $read:expr) => {{
                let budget = Arc::downgrade(self);
                let mode = mode.clone();
                // The returned handle carries no state: the callback is
                // registered on the pipeline, so dropping it changes nothing.
                let _ = meter
                    .$build($name)
                    .with_description($doc)
                    .with_unit($unit)
                    .with_callback(move |observer| {
                        if let Some(budget) = budget.upgrade() {
                            #[allow(clippy::redundant_closure_call)]
                            observer.observe($read(&budget), &mode);
                        }
                    })
                    .build();
            }};
        }

        observe!(
            u64_observable_gauge,
            "guest_memory.in_use",
            "By",
            "Guest linear memory currently charged to this host's budget",
            |b: &Arc<Self>| b.in_use()
        );
        // Kept rather than left to `max_over_time(in_use)` in the query layer:
        // the exporter samples, and a burst that begins and ends between two
        // samples is invisible to the gauge but is exactly what precedes an
        // unexplained OOM.
        observe!(
            u64_observable_gauge,
            "guest_memory.high_water",
            "By",
            "Most guest linear memory charged at once since this host started",
            |b: &Arc<Self>| b.high_water()
        );
        // Exported so a dashboard can plot utilisation without the budget being
        // hardcoded in the query.
        observe!(
            u64_observable_gauge,
            "guest_memory.limit",
            "By",
            "This host's max-guest-memory budget",
            |b: &Arc<Self>| b.cap()
        );
        observe!(
            u64_observable_counter,
            "guest_memory.refused",
            "{growth}",
            "Guest memory growths refused for crossing this host's budget",
            |b: &Arc<Self>| b.refused()
        );
        observe!(
            u64_observable_counter,
            "guest_memory.would_refuse",
            "{growth}",
            "Guest memory growths that crossed this host's budget and were \
             allowed because it is only being counted",
            |b: &Arc<Self>| b.would_refuse()
        );
    }

    /// The budget, published as metrics on the process-wide meter.
    ///
    /// Registration needs an [`Arc`] — the callbacks hold a
    /// [`std::sync::Weak`] back to it — so this is the one place the two are
    /// wired together.
    pub fn into_metered(self) -> Arc<Self> {
        let budget = Arc::new(self);
        budget.register_metrics(&opentelemetry::global::meter("wash-runtime"));
        budget
    }

    /// Report where the host stands, for the periodic host status line.
    ///
    /// At `info` while the host is *currently* under memory pressure, or when
    /// growth has been refused since the last report. That is the figure an
    /// operator is told to watch before turning enforcement on, and a host
    /// does not run at `debug`.
    ///
    /// Keyed on live pressure rather than on the high-water mark, which only
    /// ever rises: a host that crossed its budget once during a burst at
    /// minute three would otherwise say so on every heartbeat for the rest of
    /// its life, and a line that never stops carries nothing.
    pub fn report(&self) {
        let in_use = self.in_use();
        let refused = self.refused();
        let would_refuse = self.would_refuse();
        let refusals = refused.saturating_add(would_refuse);
        let new_refusals = refusals > self.reported_refusals.swap(refusals, Ordering::Relaxed);
        // Saturating, so a `u64::MAX` cap (the unmetered default) cannot
        // overflow its way under the threshold.
        let pressured = in_use >= self.cap / PRESSURE_DENOMINATOR * PRESSURE_NUMERATOR;

        if !pressured && !new_refusals {
            tracing::debug!(
                mode = self.mode.as_str(),
                in_use = %render_bytes(in_use),
                high_water = %render_bytes(self.high_water()),
                max_guest_memory = %render_bytes(self.cap),
                "guest memory budget"
            );
            return;
        }
        tracing::info!(
            mode = self.mode.as_str(),
            in_use = %render_bytes(in_use),
            high_water = %render_bytes(self.high_water()),
            max_guest_memory = %render_bytes(self.cap),
            refused,
            would_refuse,
            "guest memory is close to this host's budget"
        );
    }

    /// Charge `bytes` and report whether the growth may proceed.
    ///
    /// Under [`GuestMemoryMode::Count`] the charge is taken whether or not it
    /// fits, so the high-water figure describes what the host actually did
    /// rather than what it would have permitted.
    fn charge(&self, bytes: u64) -> bool {
        match self.mode {
            GuestMemoryMode::Enforce => {
                let taken =
                    self.in_use
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                            let next = in_use.saturating_add(bytes);
                            (next <= self.cap).then_some(next)
                        });
                match taken {
                    Ok(previous) => {
                        self.high_water
                            .fetch_max(previous.saturating_add(bytes), Ordering::Relaxed);
                        true
                    }
                    Err(in_use) => {
                        self.refused.fetch_add(1, Ordering::Relaxed);
                        self.report_refusal(bytes, in_use);
                        false
                    }
                }
            }
            GuestMemoryMode::Count => {
                // Saturating rather than `fetch_add`: this branch takes the
                // charge whether or not it fits, so nothing else stops the
                // counter wrapping past `u64::MAX` and reading as near-empty.
                let in_use = self
                    .in_use
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                        Some(in_use.saturating_add(bytes))
                    })
                    // Both variants carry the previous value; the closure
                    // never declines, so `Err` is unreachable — but reading a
                    // zero out of it would report a host holding gigabytes as
                    // holding one growth, which is the one number this mode
                    // exists to produce.
                    .unwrap_or_else(|previous| previous)
                    .saturating_add(bytes);
                self.high_water.fetch_max(in_use, Ordering::Relaxed);
                if in_use > self.cap {
                    self.would_refuse.fetch_add(1, Ordering::Relaxed);
                    self.report_crossing(bytes, in_use);
                }
                true
            }
        }
    }

    /// Say that a growth was refused: loudly the first time, quietly after
    /// that, since a host doing this is doing it on every `memory.grow`.
    ///
    /// The first one is a `warn` because of what a refusal does to the guest
    /// that gets it. `-1` is a well-formed answer, but a Rust guest's
    /// allocator turns it into `handle_alloc_error` and aborts — so this is
    /// often the only record connecting a workload that died, or a host
    /// component plugin that took its capability down with it, to the budget
    /// that caused it.
    fn report_refusal(&self, bytes: u64, in_use: u64) {
        if self.crossed.swap(true, Ordering::Relaxed) {
            tracing::debug!(
                requested = %render_bytes(bytes),
                in_use = %render_bytes(in_use),
                max_guest_memory = %render_bytes(self.cap),
                "refusing guest memory growth past this host's budget"
            );
            return;
        }
        tracing::warn!(
            requested = %render_bytes(bytes),
            in_use = %render_bytes(in_use),
            max_guest_memory = %render_bytes(self.cap),
            "refusing guest memory growth past this host's budget. The guest sees memory.grow \
             return -1; a guest that cannot handle that will abort. Raise --max-guest-memory, \
             give this host fewer workloads, or run with --guest-memory-mode count while \
             sizing it"
        );
    }

    /// Say that the budget has been crossed: loudly the first time, quietly
    /// after that. A host doing this is doing it on every `memory.grow`.
    fn report_crossing(&self, bytes: u64, in_use: u64) {
        if self.crossed.swap(true, Ordering::Relaxed) {
            tracing::debug!(
                requested = %render_bytes(bytes),
                in_use = %render_bytes(in_use),
                max_guest_memory = %render_bytes(self.cap),
                "guest memory growth is past this host's budget; allowing it because the host \
                 is in count mode"
            );
            return;
        }
        tracing::warn!(
            requested = %render_bytes(bytes),
            in_use = %render_bytes(in_use),
            max_guest_memory = %render_bytes(self.cap),
            "guest memory has crossed this host's budget; allowing it because the host is in \
             count mode. Nothing bounds guest memory until this host runs with \
             --guest-memory-mode enforce, and the kernel's own correction is an OOM kill that \
             takes every workload on this host with it"
        );
    }

    /// Return bytes a store is no longer holding.
    fn release(&self, bytes: u64) {
        // Saturating rather than `fetch_sub`, which would wrap an unbalanced
        // release into an enormous `in_use` and refuse everything thereafter.
        let _ = self
            .in_use
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                Some(in_use.saturating_sub(bytes))
            });
    }
}

/// One store's share of a [`GuestMemoryBudget`].
///
/// Lives inside the store's own [`SharedCtx`], which is what makes the release
/// exact: a store that is dropped — cleanly, or with a call still in flight —
/// drops this too, and [`Drop`] hands back everything the store was charged.
/// Nothing outside has to notice the store is gone.
///
/// [`SharedCtx`]: crate::engine::ctx::SharedCtx
#[derive(Debug, Default)]
pub struct StoreMemoryLimiter {
    /// `None` on a store built without a budget — an embedder's, or a test's.
    /// Such a store is neither charged nor limited.
    budget: Option<Arc<GuestMemoryBudget>>,
    /// What this store has taken from the budget and owes back on drop.
    charged: u64,
}

impl StoreMemoryLimiter {
    /// What this store is currently charged.
    pub fn charged(&self) -> u64 {
        self.charged
    }
}

impl wasmtime::ResourceLimiter for StoreMemoryLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let Some(budget) = &self.budget else {
            return Ok(true);
        };
        // Growth wasmtime is going to refuse on its own account is refused here
        // first, uncharged. There is no callback saying a permitted growth then
        // failed that can be trusted — `memory_grow_failed` also fires for
        // growth the limiter was never asked about — so a charge taken for
        // bytes the guest never receives is held until the store is dropped.
        //
        // Two ceilings, and the limiter is told about only one of them:
        // `maximum` is the memory *type's* limit, while the pooling allocator's
        // per-slot ceiling is applied after this returns.
        let refused_by_wasmtime = maximum.is_some_and(|maximum| desired > maximum)
            || budget
                .heap_ceiling
                .is_some_and(|ceiling| desired as u64 > ceiling);
        if refused_by_wasmtime {
            return Ok(false);
        }
        // `desired` is the memory's new total, not a delta, and wasmtime
        // guarantees it is the larger of the two.
        let growth = (desired as u64).saturating_sub(current as u64);
        if !budget.charge(growth) {
            return Ok(false);
        }
        self.charged = self.charged.saturating_add(growth);
        Ok(true)
    }

    /// Tables are not charged against the guest memory budget. The budget is
    /// denominated in linear-memory bytes — the thing `default_heap_memory`
    /// bounds one of — and a table is counted in elements, whose byte cost is
    /// wasmtime's business. What still bounds a table is its own declared
    /// maximum and the pooling allocator's `table_elements`; there is no
    /// store-level element ceiling here, and `ResourceLimiter` does not
    /// provide one (its `tables()` default is a count of tables, not of
    /// elements).
    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

impl Drop for StoreMemoryLimiter {
    fn drop(&mut self) {
        if let Some(budget) = &self.budget
            && self.charged > 0
        {
            budget.release(self.charged);
        }
    }
}

/// Point `store` at its own [`StoreMemoryLimiter`].
///
/// Required at every store creation rather than optional: a store whose
/// limiter is never installed grows its linear memories without the budget
/// ever seeing them, and the aggregate stops being an aggregate.
pub fn install_memory_limiter(store: &mut wasmtime::Store<crate::engine::ctx::SharedCtx>) {
    store.limiter(|ctx| &mut ctx.memory_limiter);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wasmtime::ResourceLimiter as _;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn budget(cap: u64, mode: GuestMemoryMode) -> Arc<GuestMemoryBudget> {
        Arc::new(GuestMemoryBudget::new(cap, mode))
    }

    /// `memory_growing` takes totals, not deltas, so a limiter has to subtract.
    /// Charging `desired` would count a memory's whole size again on every
    /// grow and exhaust the budget in a handful of pages.
    #[test]
    fn a_grow_is_charged_for_its_growth_not_the_new_total() {
        let budget = budget(100 * MIB, GuestMemoryMode::Enforce);
        let mut limiter = budget.limiter();

        assert!(limiter.memory_growing(0, 10 * MIB as usize, None).unwrap());
        assert_eq!(budget.in_use(), 10 * MIB);
        assert!(
            limiter
                .memory_growing(10 * MIB as usize, 30 * MIB as usize, None)
                .unwrap()
        );
        assert_eq!(budget.in_use(), 30 * MIB, "charged the 20MiB of growth");
        assert_eq!(limiter.charged(), 30 * MIB);
    }

    #[test]
    fn enforce_refuses_the_growth_that_crosses_the_budget() {
        let budget = budget(100 * MIB, GuestMemoryMode::Enforce);
        let mut a = budget.limiter();
        let mut b = budget.limiter();

        assert!(a.memory_growing(0, 60 * MIB as usize, None).unwrap());
        assert!(
            !b.memory_growing(0, 60 * MIB as usize, None).unwrap(),
            "the second guest is refused: together they would be over"
        );
        assert_eq!(budget.in_use(), 60 * MIB, "a refused growth is not charged");
        assert_eq!(budget.refused(), 1);
        assert_eq!(b.charged(), 0);

        // The budget refuses what does not fit, not everything thereafter.
        assert!(b.memory_growing(0, 30 * MIB as usize, None).unwrap());
        assert_eq!(budget.in_use(), 90 * MIB);
    }

    /// The upgrade-safety guarantee: a host that gains a derived budget it
    /// never asked for must behave exactly as it did before.
    #[test]
    fn count_allows_everything_and_still_reports_it() {
        let budget = budget(100 * MIB, GuestMemoryMode::Count);
        let mut a = budget.limiter();
        let mut b = budget.limiter();

        assert!(a.memory_growing(0, 80 * MIB as usize, None).unwrap());
        assert!(
            b.memory_growing(0, 80 * MIB as usize, None).unwrap(),
            "count mode never refuses"
        );
        assert_eq!(
            budget.in_use(),
            160 * MIB,
            "and still charges, so the high-water figure is true"
        );
        assert_eq!(budget.high_water(), 160 * MIB);
        assert_eq!(budget.would_refuse(), 1);
        assert_eq!(budget.refused(), 0, "nothing was actually refused");
    }

    /// The failure the budget exists to catch: guests individually under the
    /// per-memory ceiling, collectively over the host's.
    #[test]
    fn aggregate_creep_is_caught_where_a_per_memory_ceiling_would_not_be() {
        let heap_ceiling = 1024 * MIB;
        let budget = budget(4608 * MIB, GuestMemoryMode::Enforce);
        let mut guests: Vec<_> = (0..5).map(|_| budget.limiter()).collect();

        let mut admitted = 0;
        for guest in &mut guests {
            // Every one of these is legal against `default_heap_memory`.
            if guest
                .memory_growing(0, heap_ceiling as usize, Some(heap_ceiling as usize))
                .unwrap()
            {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, 4,
            "the fifth is what would have OOM-killed the pod"
        );
        assert_eq!(budget.refused(), 1);
    }

    #[test]
    fn a_dropped_store_gives_its_bytes_back() {
        let budget = budget(100 * MIB, GuestMemoryMode::Enforce);
        let mut resident = budget.limiter();
        assert!(resident.memory_growing(0, 40 * MIB as usize, None).unwrap());

        {
            let mut transient = budget.limiter();
            assert!(
                transient
                    .memory_growing(0, 50 * MIB as usize, None)
                    .unwrap()
            );
            assert_eq!(budget.in_use(), 90 * MIB);
        }

        assert_eq!(
            budget.in_use(),
            40 * MIB,
            "the dropped store's charge is returned, the survivor's is not"
        );
        // The bytes are genuinely available again, not merely uncounted.
        let mut next = budget.limiter();
        assert!(next.memory_growing(0, 60 * MIB as usize, None).unwrap());
    }

    /// A store that dies mid-call is the case that would ratchet the budget
    /// down until the host refused everything, so it is pinned separately from
    /// the orderly drop above.
    #[test]
    fn a_store_dropped_mid_growth_still_gives_its_bytes_back() {
        let budget = budget(100 * MIB, GuestMemoryMode::Enforce);
        let mut limiter = budget.limiter();
        assert!(limiter.memory_growing(0, 90 * MIB as usize, None).unwrap());
        // Refused, so nothing more is owed — and then the store goes away
        // without anyone unwinding the charge it did make.
        assert!(!limiter.memory_growing(0, 90 * MIB as usize, None).unwrap());
        drop(limiter);

        assert_eq!(budget.in_use(), 0);
        assert_eq!(budget.high_water(), 90 * MIB, "the high-water mark stands");
    }

    /// The charge has to be exact under concurrency: a lost update leaks bytes
    /// the budget never gets back, and an over-release lets it admit more than
    /// its cap.
    #[test]
    fn concurrent_growth_across_threads_balances_exactly() {
        let budget = budget(u64::MAX, GuestMemoryMode::Enforce);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let budget = Arc::clone(&budget);
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        let mut limiter = budget.limiter();
                        assert!(limiter.memory_growing(0, MIB as usize, None).unwrap());
                        assert!(
                            limiter
                                .memory_growing(MIB as usize, 2 * MIB as usize, None)
                                .unwrap()
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("no thread should panic");
        }

        assert_eq!(
            budget.in_use(),
            0,
            "every store was dropped, so every byte came back"
        );
        assert!(budget.high_water() >= 2 * MIB);
    }

    /// The pooling allocator's per-slot ceiling is applied *after* the limiter
    /// and is not what `maximum` reports — that is the memory type's limit,
    /// 4GiB for a Rust component that declares none. Charging against the
    /// wrong one leaks: a guest looping `memory.grow` past
    /// `default-heap-memory` is charged every time, wasmtime refuses every
    /// time, and the budget walks to its cap and starts refusing every other
    /// workload on the host.
    #[test]
    fn growth_past_the_pools_slot_ceiling_is_not_charged() {
        let heap_ceiling = 512 * MIB;
        let budget = Arc::new(
            GuestMemoryBudget::new(8192 * MIB, GuestMemoryMode::Enforce)
                .with_heap_ceiling(heap_ceiling),
        );
        let mut limiter = budget.limiter();

        // Up to the ceiling is fine.
        assert!(
            limiter
                .memory_growing(0, heap_ceiling as usize, Some(4096 * MIB as usize))
                .unwrap()
        );
        assert_eq!(budget.in_use(), heap_ceiling);

        // Past it, wasmtime refuses regardless — the type max says 4GiB, so
        // nothing else here would have caught this.
        for _ in 0..100 {
            assert!(
                !limiter
                    .memory_growing(
                        heap_ceiling as usize,
                        (heap_ceiling + MIB) as usize,
                        Some(4096 * MIB as usize),
                    )
                    .unwrap()
            );
        }
        assert_eq!(
            budget.in_use(),
            heap_ceiling,
            "a hundred refused growths must not have moved the budget"
        );
        assert_eq!(limiter.charged(), heap_ceiling);
    }

    /// Charging for growth wasmtime is about to refuse anyway would leave the
    /// budget holding bytes no guest ever received, for the life of the store.
    #[test]
    fn growth_past_the_memorys_own_maximum_is_not_charged() {
        let budget = budget(u64::MAX, GuestMemoryMode::Enforce);
        let mut limiter = budget.limiter();

        assert!(
            !limiter
                .memory_growing(0, 20 * MIB as usize, Some(10 * MIB as usize))
                .unwrap()
        );
        assert_eq!(budget.in_use(), 0);
        assert_eq!(limiter.charged(), 0);
        assert_eq!(
            budget.refused(),
            0,
            "wasmtime's own ceiling refused this, not the budget"
        );
    }

    #[test]
    fn a_store_without_a_budget_is_neither_charged_nor_limited() {
        let mut limiter = StoreMemoryLimiter::default();
        assert!(limiter.memory_growing(0, usize::MAX / 2, None).unwrap());
        assert_eq!(limiter.charged(), 0);
    }

    #[test]
    fn tables_are_not_charged_against_the_memory_budget() {
        let budget = budget(MIB, GuestMemoryMode::Enforce);
        let mut limiter = budget.limiter();
        assert!(limiter.table_growing(0, 1_000_000, None).unwrap());
        assert_eq!(budget.in_use(), 0);
    }

    #[test]
    fn the_budget_takes_its_cap_from_the_resolved_knobs() {
        let budgets = HostMemoryBudgets::resolve(Some(8 * 1024 * MIB), None, None).unwrap();
        let budget = GuestMemoryBudget::from_budgets(&budgets, GuestMemoryMode::Enforce);
        assert_eq!(budget.cap(), 8 * 1024 * MIB);
        assert_eq!(budget.mode(), GuestMemoryMode::Enforce);
    }

    /// Collect every `u64` metric the budget publishes, through a real SDK
    /// pipeline. A reader of its own rather than the process-wide meter, so
    /// the assertions do not depend on what another test installed globally.
    fn collect_metrics(budget: &Arc<GuestMemoryBudget>) -> BTreeMap<String, u64> {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
            data::{AggregatedMetrics, MetricData},
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        budget.register_metrics(&provider.meter("test"));
        provider.force_flush().expect("flush");

        let mut seen = BTreeMap::new();
        for resource in exporter.get_finished_metrics().expect("finished metrics") {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    let AggregatedMetrics::U64(data) = metric.data() else {
                        continue;
                    };
                    let value = match data {
                        MetricData::Gauge(gauge) => gauge.data_points().map(|p| p.value()).next(),
                        MetricData::Sum(sum) => sum.data_points().map(|p| p.value()).next(),
                        _ => None,
                    };
                    if let Some(value) = value {
                        seen.insert(metric.name().to_string(), value);
                    }
                }
            }
        }
        seen
    }

    /// The figures reach a metrics pipeline, which is how an operator watches
    /// the high-water mark before turning enforcement on. Collected rather than
    /// read off the getters, so this covers the part that can silently not
    /// work: the callbacks being registered, and reporting what the budget
    /// actually holds.
    #[test]
    fn the_budget_is_exported_as_metrics() {
        let budget = Arc::new(GuestMemoryBudget::new(100 * MIB, GuestMemoryMode::Enforce));
        let mut limiter = budget.limiter();
        assert!(limiter.memory_growing(0, 40 * MIB as usize, None).unwrap());
        assert!(
            !limiter
                .memory_growing(40 * MIB as usize, 200 * MIB as usize, None)
                .unwrap()
        );

        let seen = collect_metrics(&budget);
        assert_eq!(seen.get("guest_memory.in_use"), Some(&(40 * MIB)));
        assert_eq!(seen.get("guest_memory.high_water"), Some(&(40 * MIB)));
        assert_eq!(
            seen.get("guest_memory.limit"),
            Some(&(100 * MIB)),
            "the cap is exported so a dashboard need not hardcode it"
        );
        assert_eq!(
            seen.get("guest_memory.refused"),
            Some(&1),
            "the refusal must reach the pipeline: {seen:?}"
        );
        assert_eq!(seen.get("guest_memory.would_refuse"), Some(&0));
    }

    /// The high-water mark is exported rather than left to
    /// `max_over_time(in_use)` in the query layer, because the exporter samples:
    /// a burst that begins and ends between two collections is invisible to the
    /// gauge but is exactly what precedes an unexplained OOM.
    #[test]
    fn a_burst_between_collections_survives_in_the_high_water_mark() {
        let budget = Arc::new(GuestMemoryBudget::new(100 * MIB, GuestMemoryMode::Enforce));
        {
            let mut spike = budget.limiter();
            assert!(spike.memory_growing(0, 80 * MIB as usize, None).unwrap());
        }
        // The store is gone before anything collected, so `in_use` never
        // witnessed it.
        let seen = collect_metrics(&budget);
        assert_eq!(seen.get("guest_memory.in_use"), Some(&0));
        assert_eq!(seen.get("guest_memory.high_water"), Some(&(80 * MIB)));
    }

    /// The callbacks must not keep a dropped engine's budget alive: a process
    /// building many engines would otherwise accumulate one live budget per
    /// engine for as long as the meter provider lives.
    #[test]
    fn a_dropped_budget_is_not_held_alive_by_the_meter() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter).build())
            .build();

        let weak = {
            let budget = Arc::new(GuestMemoryBudget::new(MIB, GuestMemoryMode::Count));
            budget.register_metrics(&provider.meter("test"));
            Arc::downgrade(&budget)
        };
        assert!(
            weak.upgrade().is_none(),
            "the meter provider must not hold the budget alive"
        );
        // Collecting against a dead budget observes nothing rather than panicking.
        provider.force_flush().expect("flush");
    }

    #[test]
    fn the_default_budget_is_accounted_but_never_crossed() {
        let budget = Arc::new(GuestMemoryBudget::default());
        let mut limiter = budget.limiter();
        assert!(
            limiter
                .memory_growing(0, 4096 * MIB as usize, None)
                .unwrap()
        );
        assert_eq!(budget.in_use(), 4096 * MIB);
        assert_eq!(budget.would_refuse(), 0);
    }
}
