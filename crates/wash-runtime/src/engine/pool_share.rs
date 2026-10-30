//! One kind of work's share of the instance pool's totals: how many of the
//! pool's memories, tables, core instances, component instances, stacks and GC heaps it
//! may hold at once.
//!
//! The pooling allocator refuses an instantiation once any of its totals is
//! handed out, and says neither which work holds the slots nor how many the
//! next instantiation will take. Work admitted in bursts — messaging is the
//! one trigger that does so today — has to stop admitting before the pool
//! refuses, and has to leave room for the work it does not admit:
//! HTTP-triggered requests, warm pools and long-lived services, which
//! otherwise fail to *start* while a burst holds the pool.
//!
//! [`PoolShare`] is that bound: a [`Share`] (a fraction) of every installed
//! pool total, handed out as [`PoolReservation`]s costed from the component
//! about to be instantiated ([`PoolCost::of`]). It is a count of pool slots
//! that work may hold, not the pool itself. Work that does not reserve through
//! it — the warm instances a pool keeps, the stores HTTP builds — is invisible
//! to it, which is why it claims a fraction of each total rather than the
//! whole and why [`crate::engine::pool_usage`] exists to read the pool itself.
//! A second trigger that admits bursts builds a `PoolShare` of its own, with
//! its own fraction of each total, rather than a second mechanism; the two
//! then bound each other through the fractions they claim.
//!
//! One share serves every component its holder admits, and hands slots out
//! in arrival order: a reservation parked for a composed component's several
//! memories holds the single-memory reservations behind it until it is
//! served, though each of those would fit. That is the cost of not starving
//! the composed component behind a steady stream of smaller ones, and it is
//! confined to the holder's share, since nothing else reserves through it.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use wasmtime::component::Component;

use crate::engine::pool_usage::PoolTotals;

/// What one component instance takes from the pool while it runs: one slot
/// of each total per thing the component has.
///
/// Non-exhaustive, so a total added later is not a breaking change: build
/// one with [`Default::default`] or [`Self::USUAL`] and set the fields.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PoolCost {
    pub memories: u32,
    pub tables: u32,
    pub core_instances: u32,
    pub component_instances: u32,
    pub stacks: u32,
    /// One GC heap per store, sharing the linear-memory pool for its backing.
    pub gc_heaps: u32,
}

impl PoolCost {
    /// The usual shape: a component built by one toolchain.
    ///
    /// One linear memory, which the wit-component adapter shares with the main
    /// module; the main module's function table and the shim's, so two tables;
    /// and the largest measured core-module count, Componentize-Go's five
    /// (`$main`, the `wasi_snapshot_preview1` adapter and three glue modules;
    /// Rust p2 is four). A composition holds one memory per member, which is
    /// what [`Self::of`] reads off the component; what it cannot read is the
    /// core-instance count, which is estimated from this shape.
    ///
    /// [`crate::engine`] sizes the pool's core-instance and table totals as
    /// multiples of the memory count with this shape in mind.
    pub const USUAL: Self = Self {
        memories: 1,
        tables: 2,
        core_instances: 5,
        component_instances: 1,
        stacks: 1,
        gc_heaps: 0,
    };

    /// What `component` takes, from what wasmtime can tell of it. GC heap
    /// requirements are added separately per store, by
    /// [`PoolShare::admit_store`] from what the runtime read off the
    /// component's source.
    ///
    /// wasmtime reports the memories and tables a component instantiates, so
    /// a composition is charged every member's. It does not report core
    /// instances, which are estimated as [`Self::USUAL`]'s per memory (or per
    /// pair of tables, for a component with more tables than memories). A
    /// component that imports a core module hides its requirements altogether
    /// (`resources_required` is `None`); it is charged the usual shape, a
    /// bounded estimate that lets the pool refuse the odd oversized one at
    /// instantiation, rather than every pool slot its holder may have, which
    /// would serialize every delivery on the host behind each of its own.
    pub fn of(component: &Component) -> Self {
        let Some(resources) = component.resources_required() else {
            return Self::USUAL;
        };
        let units = resources
            .num_memories
            .max(resources.num_tables.div_ceil(Self::USUAL.tables))
            .max(1);
        Self {
            memories: resources.num_memories,
            tables: resources.num_tables,
            core_instances: units.saturating_mul(Self::USUAL.core_instances),
            component_instances: 1,
            stacks: 1,
            gc_heaps: 0,
        }
    }

    /// How many instances of this cost `totals` can run at once, tables
    /// aside: the smallest of each total divided by what one instance takes.
    ///
    /// Tables are left to the reservation. The usual shape's two is an upper
    /// estimate that a one-table component never reaches, and a count that
    /// halved for it would under-admit every such component, while a
    /// reservation charges each component the tables it actually has. A
    /// resource this cost does not use does not bound it.
    pub(crate) fn fits_in(self, totals: &PoolTotals) -> u32 {
        let per = |installed: u32, each: u32| installed.checked_div(each).unwrap_or(u32::MAX);
        per(totals.memories, self.memories)
            .min(per(totals.core_instances, self.core_instances))
            .min(per(totals.component_instances, self.component_instances))
            .min(per(totals.stacks, self.stacks))
            .min(per(totals.gc_heaps, self.gc_heaps))
    }

    /// `self` and `other` instantiated into one store: memories, tables, GC
    /// heaps and instances add, and the store's one fiber stack is shared.
    fn in_one_store_with(self, other: Self) -> Self {
        Self {
            memories: self.memories.saturating_add(other.memories),
            tables: self.tables.saturating_add(other.tables),
            core_instances: self.core_instances.saturating_add(other.core_instances),
            component_instances: self
                .component_instances
                .saturating_add(other.component_instances),
            stacks: self.stacks.max(other.stacks),
            gc_heaps: self.gc_heaps.saturating_add(other.gc_heaps),
        }
    }
}

/// The fraction of each installed pool total a [`PoolShare`] may hold,
/// floored and capped in instances of the usual shape ([`PoolCost::USUAL`]):
/// a total an instance takes five of is floored and capped at five times the
/// number, so every total binds at the same number of instances. The floor
/// never raises a count past the installed total. One `Share` sizes every
/// total of one [`PoolShare`] and the ceiling on work derived beside it, so
/// the two cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Share {
    pub numerator: u32,
    pub denominator: u32,
    pub floor: u32,
    pub cap: u32,
}

impl Share {
    /// How many instances this fraction of `installed` instances comes to.
    pub fn of(self, installed: u32) -> u32 {
        self.of_slots(installed, 1)
    }

    /// How many of a pool total of `installed` slots this fraction comes to,
    /// for a total an instance of the usual shape takes `per_instance` of.
    pub fn of_slots(self, installed: u32, per_instance: u32) -> u32 {
        let fraction =
            u64::from(installed) * u64::from(self.numerator) / u64::from(self.denominator.max(1));
        u32::try_from(fraction)
            .unwrap_or(u32::MAX)
            .max(self.floor.saturating_mul(per_instance).min(installed))
            .min(self.cap.saturating_mul(per_instance))
            .min(installed)
    }
}

/// The holder's share of one pool total: what is installed, how many of
/// those slots the holder may have at once (its budget of them), and the
/// permits that hand them out.
#[derive(Debug)]
struct Gate {
    /// The name wasmtime's refusal and the `WASMTIME_POOLING_TOTAL_*`
    /// variable use for the total.
    kind: &'static str,
    knob: &'static str,
    installed: u32,
    /// The most of `installed` the holder may have at once.
    budget: u32,
    slots: Arc<Semaphore>,
}

impl Gate {
    fn new(
        kind: &'static str,
        knob: &'static str,
        installed: u32,
        share: Share,
        per_instance: u32,
    ) -> Self {
        let budget = share
            .of_slots(installed, per_instance)
            .min(u32::try_from(Semaphore::MAX_PERMITS).unwrap_or(u32::MAX));
        Self {
            kind,
            knob,
            installed,
            budget,
            slots: Arc::new(Semaphore::new(budget as usize)),
        }
    }

    /// Refuse a cost of this total that the holder's budget of it can never
    /// hold. Admitting it capped at that budget would hand out every permit
    /// for the total and still fail at instantiation, since the pool wants the
    /// whole cost, and would hold every other reservation off while it did.
    fn admit(&self, holder: &str, required: u32) -> anyhow::Result<()> {
        if required > 0 && self.installed == 0 {
            anyhow::bail!(
                "a component of this shape needs {required} {kind} per instance, but the pool \
                 has none: {knob} is 0. Unset it, so the pool derives it from the memory \
                 count, or set it to at least {required}",
                kind = self.kind,
                knob = self.knob,
            );
        }
        if required > self.budget {
            anyhow::bail!(
                "a component of this shape needs {required} {kind} per instance, but {holder} \
                 may hold at most {budget} of the pool's {installed} at once; raise \
                 --max-memories (or {knob}) until {holder}'s share of the pool's {kind} \
                 holds one",
                kind = self.kind,
                budget = self.budget,
                installed = self.installed,
                knob = self.knob,
            );
        }
        Ok(())
    }

    async fn take(&self, cost: u32) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.slots).acquire_many_owned(cost).await.ok()
    }

    fn available(&self) -> u32 {
        u32::try_from(self.slots.available_permits()).unwrap_or(u32::MAX)
    }
}

/// The six totals a store spends, each behind its own gate.
#[derive(Debug)]
struct Gates {
    memories: Gate,
    tables: Gate,
    core_instances: Gate,
    component_instances: Gate,
    stacks: Gate,
    gc_heaps: Gate,
}

/// One kind of work's share of every installed pool total, cloned to
/// wherever that work is admitted. Clones hand out the same permits.
#[derive(Clone, Debug)]
pub struct PoolShare {
    /// Named in refusals: whose share of the pool totals this is.
    holder: &'static str,
    gates: Arc<Gates>,
}

impl PoolShare {
    /// The fraction `share` of every total in `totals`, for `holder` (named
    /// in the error a component too large for its share gets).
    pub(crate) fn new(totals: PoolTotals, share: Share, holder: &'static str) -> Self {
        let usual = PoolCost::USUAL;
        let gate = |kind, knob, installed, per_instance| {
            Gate::new(kind, knob, installed, share, per_instance)
        };
        Self {
            holder,
            gates: Arc::new(Gates {
                memories: gate(
                    "memories",
                    "WASMTIME_POOLING_TOTAL_MEMORIES",
                    totals.memories,
                    usual.memories,
                ),
                tables: gate(
                    "tables",
                    "WASMTIME_POOLING_TOTAL_TABLES",
                    totals.tables,
                    usual.tables,
                ),
                core_instances: gate(
                    "core instances",
                    "WASMTIME_POOLING_TOTAL_CORE_INSTANCES",
                    totals.core_instances,
                    usual.core_instances,
                ),
                component_instances: gate(
                    "component instances",
                    "WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES",
                    totals.component_instances,
                    usual.component_instances,
                ),
                stacks: gate(
                    "stacks",
                    "WASMTIME_POOLING_TOTAL_STACKS",
                    totals.stacks,
                    usual.stacks,
                ),
                // Floored and capped at one per instance, though the usual
                // shape takes none: a store that may allocate GC objects
                // takes exactly one, and a per-instance figure of zero would
                // give this gate no budget at all. Such a store also takes a
                // second memory for the heap's backing, so a share of GC
                // stores is bound by the memories gate at half the instances
                // the other gates allow: that is the pool's real cost, not
                // an accounting slip, and the reason the GC charge is made
                // only for components whose source may need it.
                gc_heaps: gate(
                    "GC heaps",
                    "WASMTIME_POOLING_TOTAL_GC_HEAPS",
                    totals.gc_heaps,
                    1,
                ),
            }),
        }
    }

    /// The holder's budget of each pool total: the most of it this share
    /// hands out at once.
    pub fn budget(&self) -> PoolCost {
        let g = &*self.gates;
        PoolCost {
            memories: g.memories.budget,
            tables: g.tables.budget,
            core_instances: g.core_instances.budget,
            component_instances: g.component_instances.budget,
            stacks: g.stacks.budget,
            gc_heaps: g.gc_heaps.budget,
        }
    }

    /// What is not reserved right now.
    pub fn available(&self) -> PoolCost {
        let g = &*self.gates;
        PoolCost {
            memories: g.memories.available(),
            tables: g.tables.available(),
            core_instances: g.core_instances.available(),
            component_instances: g.component_instances.available(),
            stacks: g.stacks.available(),
            gc_heaps: g.gc_heaps.available(),
        }
    }

    /// [`Self::admit_store`] for one component alone, charging no GC heap.
    /// It under-reserves a component that allocates GC objects, so a caller
    /// that cannot rule that out uses [`Self::admit_store`].
    pub fn admit(&self, component: &Component) -> anyhow::Result<PoolCost> {
        self.admit_store(std::iter::once(component), false)
    }

    /// Check that the cost of one store ([`PoolCost::of`] each component
    /// instantiated into it, linked dependencies included) fits the holder's
    /// budget of every pool total at all, once per binding rather than per
    /// reservation, and return what each reservation should take. Memories,
    /// tables and instances add; the fiber stack is shared by the entire
    /// store. Estimates are clamped only after adding them, so several
    /// individually admissible components cannot overdraw a gate.
    ///
    /// `may_use_gc` charges the store one GC heap and a memory to back it.
    /// Pass it only for a store some component of which may allocate GC
    /// objects; the runtime reads that off each component's source, since a
    /// compiled component does not say.
    ///
    /// What is read off the components — their memories, and their tables
    /// when wasmtime reports them — is refused when the budget can never hold
    /// it; see [`Gate::admit`]. What is estimated — core instances always,
    /// tables for a component that hides its requirements — is clamped to the
    /// budget instead: a refusal there would turn away a component the pool
    /// may well hold, over a number nothing measured, and the pool itself
    /// still refuses one that truly does not fit.
    pub fn admit_store<'a>(
        &self,
        components: impl IntoIterator<Item = &'a Component>,
        may_use_gc: bool,
    ) -> anyhow::Result<PoolCost> {
        let g = &*self.gates;
        let mut cost = PoolCost::default();
        let mut measured_tables = 0_u32;
        for component in components {
            let next = PoolCost::of(component);
            cost = cost.in_one_store_with(next);
            if component.resources_required().is_some() {
                measured_tables = measured_tables.saturating_add(next.tables);
            }
        }
        // All linked instances share one GC heap in this store. Its backing
        // memory comes from the same pool as the declared linear memories.
        cost.gc_heaps = u32::from(may_use_gc);
        cost.memories = cost.memories.saturating_add(cost.gc_heaps);
        g.gc_heaps.admit(self.holder, cost.gc_heaps)?;
        g.memories.admit(self.holder, cost.memories)?;
        g.component_instances
            .admit(self.holder, cost.component_instances)?;
        g.stacks.admit(self.holder, cost.stacks)?;
        g.tables.admit(self.holder, measured_tables)?;
        Ok(PoolCost {
            tables: cost.tables.min(g.tables.budget),
            core_instances: cost.core_instances.min(g.core_instances.budget),
            ..cost
        })
    }

    /// Hold `cost` out of the holder's share of each pool total until the
    /// reservation is dropped, parking while any of those totals' slots are
    /// all held.
    ///
    /// The totals are taken in one order for every caller — memories, tables,
    /// core instances, component instances, stacks, GC heaps — so two reservations
    /// cannot wait on each other; each is served in arrival order (see the
    /// module docs). Cancel-safe: dropping the future before it resolves
    /// releases whatever it had already taken. `None` only if a gate was
    /// closed, which nothing does.
    pub async fn reserve(&self, cost: PoolCost) -> Option<PoolReservation> {
        let g = &*self.gates;
        Some(PoolReservation {
            _memories: g.memories.take(cost.memories).await?,
            _tables: g.tables.take(cost.tables).await?,
            _core_instances: g.core_instances.take(cost.core_instances).await?,
            _component_instances: g.component_instances.take(cost.component_instances).await?,
            _stacks: g.stacks.take(cost.stacks).await?,
            _gc_heaps: g.gc_heaps.take(cost.gc_heaps).await?,
        })
    }
}

/// One instance's cost in pool slots, held out of a [`PoolShare`] until
/// dropped.
#[derive(Debug)]
pub struct PoolReservation {
    _memories: OwnedSemaphorePermit,
    _tables: OwnedSemaphorePermit,
    _core_instances: OwnedSemaphorePermit,
    _component_instances: OwnedSemaphorePermit,
    _stacks: OwnedSemaphorePermit,
    _gc_heaps: OwnedSemaphorePermit,
}

/// The pool, engine and components the share's tests and messaging's share
/// tests build, once.
#[cfg(test)]
pub(crate) mod test_support {
    use wasmtime::PoolingAllocationConfig;
    use wasmtime::component::Component;

    use crate::engine::Engine;

    /// A small pool with every total explicit, so no environment override
    /// moves it: 32 memories, and the engine's multiples of that for core
    /// instances and tables.
    pub(crate) fn config() -> PoolingAllocationConfig {
        let mut config = PoolingAllocationConfig::default();
        config
            .total_memories(32)
            .total_core_instances(512)
            .total_tables(128)
            .total_component_instances(32)
            .total_stacks(32)
            .max_memory_size(1 << 16);
        config
    }

    pub(crate) fn engine(config: PoolingAllocationConfig) -> Engine {
        Engine::builder()
            .with_pooling_config(config)
            .without_max_process_memory()
            .build()
            .unwrap()
    }

    /// A component of `cores` core modules, the first `memories` of them
    /// with a linear memory and the first `tables` with a table.
    pub(crate) fn component(
        engine: &Engine,
        memories: usize,
        tables: usize,
        cores: usize,
    ) -> Component {
        let mut source = String::from("(component");
        for i in 0..cores.max(memories).max(tables) {
            source.push_str(&format!("(core module $m{i}"));
            if i < memories {
                source.push_str("(memory 1)");
            }
            if i < tables {
                source.push_str("(table 1 funcref)");
            }
            source.push_str(&format!(") (core instance (instantiate $m{i}))"));
        }
        source.push(')');
        Component::new(engine.inner(), wat::parse_str(source).unwrap()).unwrap()
    }

    /// A component that imports its core module, which hides its resource
    /// requirements from wasmtime (`resources_required` is `None`).
    pub(crate) fn hidden_component(engine: &Engine) -> Component {
        let bytes = wat::parse_str(
            r#"(component
            (core type $m (module))
            (import "module" (core module $m (type $m)))
            (core instance (instantiate $m))
        )"#,
        )
        .unwrap();
        Component::new(engine.inner(), bytes).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt as _;
    use tokio::task::unconstrained;
    use wasmtime::Store;
    use wasmtime::component::Linker;

    use super::test_support::{component, config, engine, hidden_component};
    use super::*;
    use crate::engine::{Engine, derived_pool_totals};
    // Two thirds of each total, floored at 8 and capped at 4096: the numbers
    // below are a stock host's.
    use crate::plugin::wasmcloud_messaging::MESSAGING_SHARE;

    fn pool_share(engine: &Engine) -> PoolShare {
        PoolShare::new(engine.pool_totals().unwrap(), MESSAGING_SHARE, "the test")
    }

    /// Reserve and instantiate until a reservation queues, holding every
    /// store: how many instances the share of the pool totals admits, checked
    /// against the real pool, which must take each one. Dropping one must let
    /// the queued reservation through and its instantiation succeed.
    async fn fill(engine: &Engine, component: &Component) -> usize {
        let pool_share = pool_share(engine);
        let cost = pool_share.admit(component).unwrap();
        let linker = Linker::<()>::new(engine.inner());
        let mut held = Vec::new();
        for _ in 0..=u32::MAX {
            // Resource availability, not whether Tokio's cooperative budget
            // happened to expire in this loop.
            let Some(reservation) = unconstrained(pool_share.reserve(cost))
                .now_or_never()
                .flatten()
            else {
                assert!(
                    !held.is_empty(),
                    "the share of the pool totals should admit at least one"
                );
                let admitted = held.len();
                drop(held.pop());
                let _reservation = unconstrained(pool_share.reserve(cost))
                    .now_or_never()
                    .flatten()
                    .expect("a dropped reservation frees its cost");
                let mut store = Store::new(engine.inner(), ());
                linker
                    .instantiate_async(&mut store, component)
                    .await
                    .unwrap();
                return admitted;
            };
            let mut store = Store::new(engine.inner(), ());
            linker
                .instantiate_async(&mut store, component)
                .await
                .expect("a reservation must queue before the pool can refuse");
            held.push((store, reservation));
        }
        unreachable!("the share of the pool totals never queued")
    }

    #[test]
    fn a_share_of_a_pool_total_is_the_fraction_floored_and_capped_within_it() {
        let share = MESSAGING_SHARE;
        assert_eq!(share.of(1000), 666);
        assert_eq!(share.of(32), 21);
        // The floor binds on a small total but never exceeds it.
        assert_eq!(share.of(9), 8);
        assert_eq!(share.of(4), 4);
        assert_eq!(share.of(1), 1);
        assert_eq!(share.of(0), 0);
        // The cap binds on an enormous one.
        assert_eq!(share.of(64_000), 4096);
        assert_eq!(share.of(u32::MAX), 4096);
    }

    #[test]
    fn the_cap_binds_every_total_at_the_same_number_of_instances() {
        // Core instances cost five per instance and tables two, so a cap on
        // slots would bind them at a fifth and a half of the instances the
        // memories allow. In instances, no total runs out before the memories
        // on any pool size.
        let usual = PoolCost::USUAL;
        for core_instances in [1000u32, 2000, 10_000, 100_000] {
            let budget = PoolShare::new(
                derived_pool_totals(core_instances),
                MESSAGING_SHARE,
                "the test",
            )
            .budget();
            let instances = budget.memories;
            assert!(
                budget.core_instances / usual.core_instances >= instances,
                "--max-memories {core_instances}: {} core instances for {instances} memories",
                budget.core_instances
            );
            assert!(
                budget.tables / usual.tables >= instances,
                "--max-memories {core_instances}: {} tables for {instances} memories",
                budget.tables
            );
            assert!(budget.stacks >= instances && budget.component_instances >= instances);
        }
        // And the floor scales the same way: a tiny pool's eight instances
        // are forty core instances.
        assert_eq!(MESSAGING_SHARE.of_slots(100, usual.core_instances), 66);
        assert_eq!(MESSAGING_SHARE.of_slots(50, usual.core_instances), 40);
    }

    #[test]
    fn the_usual_shape_fits_a_stock_pool_once_per_memory() {
        let stock = derived_pool_totals(1000);
        assert_eq!(PoolCost::USUAL.fits_in(&stock), 1000);
        // Tables are left to the reservation.
        let mut few_tables = stock;
        few_tables.tables = 1;
        assert_eq!(PoolCost::USUAL.fits_in(&few_tables), 1000);
        // Any of the other totals, overridden lower, binds.
        let mut few_stacks = stock;
        few_stacks.stacks = 100;
        assert_eq!(PoolCost::USUAL.fits_in(&few_stacks), 100);
        let mut few_cores = stock;
        few_cores.core_instances = 50;
        assert_eq!(PoolCost::USUAL.fits_in(&few_cores), 10);
    }

    #[test]
    fn a_cost_is_read_off_the_component_and_cores_estimated_from_the_shape() {
        let engine = engine(config());
        let two_memories = PoolCost::of(&component(&engine, 2, 0, 2));
        assert_eq!(two_memories.memories, 2);
        assert_eq!(two_memories.tables, 0);
        assert_eq!(
            two_memories.core_instances,
            2 * PoolCost::USUAL.core_instances
        );
        assert_eq!(two_memories.component_instances, 1);
        assert_eq!(two_memories.stacks, 1);
        // More tables than memories: a pair of tables counts as a unit.
        let tables = PoolCost::of(&component(&engine, 1, 5, 5));
        assert_eq!(tables.core_instances, 3 * PoolCost::USUAL.core_instances);
        // No memory at all still runs one core instance's worth.
        let empty = PoolCost::of(&component(&engine, 0, 0, 1));
        assert_eq!(empty.core_instances, PoolCost::USUAL.core_instances);
    }

    #[test]
    fn a_component_that_hides_its_requirements_is_charged_the_usual_shape() {
        // Imported core modules hide the requirements from wasmtime. Charging
        // every pool slot the holder may have would serialize every delivery
        // on the host behind this one component's; the usual shape bounds it.
        let engine = engine(config());
        let component = hidden_component(&engine);
        assert!(component.resources_required().is_none());
        assert_eq!(PoolCost::of(&component), PoolCost::USUAL);
    }

    /// A store not known to use GC is charged no heap;
    /// a store known to need one is charged a heap and its backing memory,
    /// and a pool with no GC heaps at all says which knob emptied it.
    #[test]
    fn only_a_store_that_may_use_gc_is_charged_a_heap() {
        let mut config = config();
        config.total_gc_heaps(0);
        let engine = engine(config);
        let pool_share = pool_share(&engine);
        let component = component(&engine, 1, 1, 2);

        let plain = pool_share.admit(&component).unwrap();
        assert_eq!(plain.gc_heaps, 0);
        assert_eq!(plain.memories, 1);

        let error = pool_share
            .admit_store([&component], true)
            .expect_err("a pool with no GC heaps cannot hold a GC store");
        let message = error.to_string();
        assert!(
            message.contains("WASMTIME_POOLING_TOTAL_GC_HEAPS is 0"),
            "{message}"
        );
    }

    #[test]
    fn estimated_costs_are_clamped_to_the_budget_rather_than_refused() {
        // A total nothing measured must not refuse a binding: core instances
        // are always estimated, and tables are for a component that hides
        // its requirements. A share of one table and four core instances
        // admits the usual shape's two and five, clamped, and the pool
        // refuses the component at instantiation if it truly does not fit.
        let mut config = config();
        config.total_tables(1).total_core_instances(4);
        let engine = engine(config);
        let pool_share = pool_share(&engine);
        let cost = pool_share.admit(&hidden_component(&engine)).unwrap();
        assert_eq!(cost.memories, 1);
        assert_eq!(cost.tables, 1);
        assert_eq!(cost.core_instances, 4);
        // Tables wasmtime did measure are refused where the share can never
        // hold them, since a reservation capped at the share would hold
        // every table permit and still fail at instantiation.
        let err = pool_share
            .admit(&component(&engine, 1, 2, 2))
            .expect_err("two measured tables do not fit a share of one");
        let text = format!("{err:#}");
        assert!(text.contains("2 tables"), "{text}");
        assert!(text.contains("WASMTIME_POOLING_TOTAL_TABLES"), "{text}");
        // And a measured table within the share is charged as measured.
        let cost = pool_share.admit(&component(&engine, 1, 1, 2)).unwrap();
        assert_eq!(cost.tables, 1);
    }

    #[tokio::test]
    async fn composed_components_pay_for_every_memory() {
        let engine = engine(config());
        // The share of the pool's 32 memories is 21, so ten two-memory
        // components fit.
        assert_eq!(fill(&engine, &component(&engine, 2, 0, 2)).await, 10);
    }

    #[tokio::test]
    async fn the_usual_shape_fills_the_share_of_memories() {
        let engine = engine(config());
        assert_eq!(fill(&engine, &component(&engine, 1, 2, 4)).await, 21);
    }

    #[tokio::test]
    async fn independently_overridden_tables_bound_compositions() {
        let mut config = config();
        config.total_tables(32);
        let engine = engine(config);
        assert_eq!(fill(&engine, &component(&engine, 1, 2, 4)).await, 10);
    }

    #[tokio::test]
    async fn a_tableless_component_needs_no_table_capacity() {
        let mut config = config();
        config.total_tables(0);
        let engine = engine(config);
        assert_eq!(fill(&engine, &component(&engine, 1, 0, 1)).await, 21);
    }

    #[tokio::test]
    async fn a_lowered_core_total_queues_before_instantiation_fails() {
        let mut config = config();
        config.total_core_instances(32);
        let engine = engine(config);
        // Two thirds of the pool's 32 core instances is 21, but the floor of
        // eight instances' worth is 40, so the share is all 32: six components
        // at an estimated 5 each, the same six the pool holds of the usual
        // shape, and the pool queues the seventh rather than refusing it.
        assert_eq!(fill(&engine, &component(&engine, 1, 0, 4)).await, 6);
    }

    #[test]
    fn a_component_needing_more_memories_than_the_share_is_refused_up_front() {
        // Admitted capped at the holder's 21 memories, it would take every
        // memory permit, hold all other work off, and still fail at
        // instantiation when anything else held a memory: the pool wants all
        // 25. Refusing names the knob.
        let engine = engine(config());
        let pool_share = pool_share(&engine);
        let err = pool_share
            .admit(&component(&engine, 25, 0, 25))
            .expect_err("25 memories do not fit a share of 21 memories");
        let text = format!("{err:#}");
        assert!(text.contains("25 memories"), "{text}");
        assert!(text.contains("the test may hold at most 21"), "{text}");
        assert!(text.contains("--max-memories"), "{text}");
        assert!(text.contains("WASMTIME_POOLING_TOTAL_MEMORIES"), "{text}");
        // Exactly the share of memories fits.
        pool_share.admit(&component(&engine, 21, 0, 21)).unwrap();
    }

    #[tokio::test]
    async fn cancelling_a_reservation_returns_every_earlier_permit() {
        let engine = engine(config());
        let pool_share = pool_share(&engine);
        let cost = pool_share.admit(&component(&engine, 1, 1, 4)).unwrap();
        let g = &*pool_share.gates;
        for blocked in [
            &g.memories,
            &g.tables,
            &g.core_instances,
            &g.component_instances,
            &g.stacks,
        ] {
            let _blocker = Arc::clone(&blocked.slots)
                .acquire_many_owned(blocked.budget)
                .await
                .unwrap();
            let before = pool_share.available();
            assert!(
                unconstrained(pool_share.reserve(cost))
                    .now_or_never()
                    .is_none()
            );
            assert_eq!(
                before,
                pool_share.available(),
                "cancelled while waiting for {}",
                blocked.kind
            );
        }
    }

    #[tokio::test]
    async fn clones_hand_out_the_same_permits() {
        let engine = engine(config());
        let pool_share = pool_share(&engine);
        let other = pool_share.clone();
        let small = pool_share.admit(&component(&engine, 1, 0, 1)).unwrap();
        let composed = other.admit(&component(&engine, 2, 0, 2)).unwrap();
        let mut held = Vec::new();
        for i in 0..14 {
            let (pool_share, cost) = if i < 7 {
                (&pool_share, small)
            } else {
                (&other, composed)
            };
            held.push(
                unconstrained(pool_share.reserve(cost))
                    .now_or_never()
                    .flatten()
                    .unwrap(),
            );
        }
        // Seven single-memory and seven two-memory reservations spend all 21
        // memories of the share.
        assert_eq!(pool_share.available().memories, 0);
        assert!(
            unconstrained(pool_share.reserve(small))
                .now_or_never()
                .is_none()
        );
        drop(held.pop());
        assert!(
            unconstrained(other.reserve(small))
                .now_or_never()
                .flatten()
                .is_some()
        );
    }
}
