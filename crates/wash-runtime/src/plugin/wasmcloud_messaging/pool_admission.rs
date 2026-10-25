//! Resource capacity shared by messaging backends, separate from message counts.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use wasmtime::component::Component;

use crate::engine::pool_usage::PoolTotals;

// Retain the previous allowance for the largest measured core-module shape.
// Wasmtime exposes memory and table requirements, but not core-instance counts.
// Compositions pay it again for each additional memory or pair of tables.
const CORE_INSTANCES_PER_UNIT: u32 = 5;
const TABLES_PER_UNIT: u32 = 2;

/// Capacity for the usual single-memory component. Other shapes also pass
/// through the resource gates below before any store is built.
pub(super) fn capacity(totals: PoolTotals) -> u32 {
    totals
        .memories
        .min(totals.core_instances / CORE_INSTANCES_PER_UNIT)
        .min(totals.component_instances)
        .min(totals.stacks)
}

#[derive(Debug)]
struct ResourceGate {
    name: &'static str,
    installed: u32,
    budget: u32,
    slots: Arc<Semaphore>,
}

impl ResourceGate {
    fn new(name: &'static str, installed: u32, requested: Option<u64>) -> Self {
        let share = (u64::from(installed) * u64::from(super::MESSAGING_POOL_SHARE_NUM)
            / u64::from(super::MESSAGING_POOL_SHARE_DEN))
        .max(u64::from(
            installed.min(super::MIN_DERIVED_IN_FLIGHT as u32),
        ));
        let budget = requested
            .unwrap_or(share)
            .max(share)
            .min(u64::from(installed))
            .min(Semaphore::MAX_PERMITS as u64) as u32;
        Self {
            name,
            installed,
            budget,
            slots: Arc::new(Semaphore::new(budget as usize)),
        }
    }

    fn cost(&self, required: u32) -> anyhow::Result<u32> {
        if required > self.installed {
            anyhow::bail!(
                "messaging component needs {required} {} but the installed pool holds {}",
                self.name,
                self.installed,
            );
        }
        // A single large component may use more than messaging's usual share,
        // provided it fits the physical pool. Taking the whole budget excludes
        // other messaging instances needing this resource until it finishes.
        Ok(required.min(self.budget))
    }

    async fn acquire(&self, cost: u32) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.slots).acquire_many_owned(cost).await.ok()
    }
}

#[derive(Clone, Debug)]
pub(super) struct PoolAdmission {
    // One consistent acquisition order across every component and backend.
    gates: Arc<[ResourceGate; 5]>,
    costs: [u32; 5],
}

impl PoolAdmission {
    pub(super) fn new(totals: PoolTotals, explicit_host_total: Option<usize>) -> Self {
        let requested = explicit_host_total.map(|n| n as u64);
        Self {
            gates: Arc::new([
                ResourceGate::new(
                    "core instances",
                    totals.core_instances,
                    requested.map(|n| n.saturating_mul(u64::from(CORE_INSTANCES_PER_UNIT))),
                ),
                ResourceGate::new("memories", totals.memories, requested),
                ResourceGate::new(
                    "tables",
                    totals.tables,
                    requested.map(|n| n.saturating_mul(u64::from(TABLES_PER_UNIT))),
                ),
                ResourceGate::new("component instances", totals.component_instances, requested),
                ResourceGate::new("stacks", totals.stacks, requested),
            ]),
            // The final linked component supplies these before delivery starts.
            costs: [0; 5],
        }
    }

    /// Resolve the cost from the final linked component, once per subscription.
    pub(super) fn with_component(mut self, component: &Component) -> anyhow::Result<Self> {
        let Some(resources) = component.resources_required() else {
            // Imported core modules can hide the requirements. Serialize those
            // deliveries against all messaging resources instead of assuming
            // they have the single-memory shape.
            self.costs = self.gates.each_ref().map(|gate| gate.budget);
            return Ok(self);
        };
        let [core, memories, tables, components, stacks] = self.gates.as_ref();
        let core_instances = resources
            .num_memories
            .max(resources.num_tables.div_ceil(TABLES_PER_UNIT))
            .max(1)
            .saturating_mul(CORE_INSTANCES_PER_UNIT)
            .min(core.installed);
        self.costs = [
            core.cost(core_instances)?,
            memories.cost(resources.num_memories)?,
            tables.cost(resources.num_tables)?,
            components.cost(1)?,
            stacks.cost(1)?,
        ];
        Ok(self)
    }

    pub(super) async fn acquire(&self) -> Option<[OwnedSemaphorePermit; 5]> {
        let [core, memories, tables, components, stacks] = self.gates.as_ref();
        let [
            core_cost,
            memory_cost,
            table_cost,
            component_cost,
            stack_cost,
        ] = self.costs;
        // A fixed array keeps the delivery path allocation-free. If any wait
        // is cancelled or closed, already-acquired permits are dropped too.
        Some([
            core.acquire(core_cost).await?,
            memories.acquire(memory_cost).await?,
            tables.acquire(table_cost).await?,
            components.acquire(component_cost).await?,
            stacks.acquire(stack_cost).await?,
        ])
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt as _;
    use tokio::task::unconstrained;
    use wasmtime::component::{Component, Linker};
    use wasmtime::{PoolingAllocationConfig, Store};

    use super::*;
    use crate::engine::Engine;
    use crate::plugin::wasmcloud_messaging::{AdmissionIdentity, MessagingLimits};

    fn config() -> PoolingAllocationConfig {
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

    fn engine(config: PoolingAllocationConfig) -> Engine {
        Engine::builder()
            .with_pooling_config(config)
            .without_max_process_memory()
            .build()
            .unwrap()
    }

    fn component(engine: &Engine, memories: usize, tables: usize, cores: usize) -> Component {
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

    /// Hold real stores and admission permits until the next delivery queues.
    /// Dropping a completed delivery must let that queued delivery instantiate.
    async fn fill(engine: &Engine, component: &Component, explicit_host: Option<usize>) -> usize {
        let limits = MessagingLimits::resolve(explicit_host, None, engine);
        let linker = Linker::<()>::new(engine.inner());
        let mut held = Vec::new();
        for i in 0..=limits.host_total() {
            let identity = AdmissionIdentity::new("review", &format!("workload-{i}"), "component");
            let admission = limits
                .admission(&identity, None)
                .with_component(component)
                .unwrap();
            // Check resource availability, not whether Tokio's cooperative
            // scheduling budget happened to expire in this polling loop.
            let Some(permit) = unconstrained(admission.acquire()).now_or_never().flatten() else {
                assert!(
                    !held.is_empty(),
                    "the pool should admit at least one component"
                );
                let admitted = held.len();
                drop(held.pop());
                let _permit = unconstrained(admission.acquire())
                    .now_or_never()
                    .flatten()
                    .expect("a completed delivery must release its resources");
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
                .expect("admission must queue before instantiation can exhaust the pool");
            held.push((store, permit));
        }
        panic!("admitted more messages than the host ceiling");
    }

    #[tokio::test]
    async fn a_custom_core_total_queues_before_instantiation_fails() {
        let mut config = config();
        config.total_core_instances(32);
        let engine = engine(config);
        let component = component(&engine, 1, 0, 4);
        let limits = MessagingLimits::resolve(None, None, &engine);
        assert_eq!(limits.host_total(), 6);
        // Previously the ninth of 21 admitted messages failed; the core gate
        // now queues even when an explicit message ceiling is larger.
        assert_eq!(fill(&engine, &component, None).await, 4);
        assert_eq!(fill(&engine, &component, Some(64)).await, 6);
    }

    #[tokio::test]
    async fn composed_components_pay_for_every_memory() {
        let engine = engine(config());
        let component = component(&engine, 2, 0, 2);
        // Previously the seventeenth of 21 admitted messages failed. The
        // resource share is 21 memories, so ten two-memory components fit.
        assert_eq!(fill(&engine, &component, None).await, 10);
    }

    #[tokio::test]
    async fn ordinary_components_keep_the_larger_ceiling() {
        let engine = engine(config());
        let component = component(&engine, 1, 2, 4);
        assert_eq!(fill(&engine, &component, None).await, 21);
    }

    #[tokio::test]
    async fn independently_overridden_tables_bound_compositions() {
        let mut config = config();
        config.total_tables(32);
        let engine = engine(config);
        let component = component(&engine, 1, 2, 4);
        assert_eq!(fill(&engine, &component, None).await, 10);
    }

    #[tokio::test]
    async fn component_and_stack_overrides_bound_explicit_message_limits() {
        for stacks in [false, true] {
            let mut config = config();
            if stacks {
                config.total_stacks(4);
            } else {
                config.total_component_instances(4);
            }
            let engine = engine(config);
            let component = component(&engine, 1, 0, 1);
            assert_eq!(fill(&engine, &component, Some(64)).await, 4);
        }
    }

    #[tokio::test]
    async fn an_explicit_single_message_can_hold_multiple_memories() {
        let engine = engine(config());
        let component = component(&engine, 2, 0, 2);
        assert_eq!(fill(&engine, &component, Some(1)).await, 1);
    }

    #[tokio::test]
    async fn one_component_larger_than_the_share_runs_exclusively() {
        let engine = engine(config());
        let component = component(&engine, 25, 0, 25);
        assert_eq!(fill(&engine, &component, None).await, 1);
    }

    #[tokio::test]
    async fn a_tableless_component_needs_no_table_capacity() {
        let mut config = config();
        config.total_tables(0);
        let engine = engine(config);
        let component = component(&engine, 1, 0, 1);
        assert_eq!(fill(&engine, &component, None).await, 21);
    }

    #[tokio::test]
    async fn cancelling_a_resource_wait_returns_every_earlier_permit() {
        let engine = engine(config());
        let component = component(&engine, 1, 1, 4);
        let admission = PoolAdmission::new(engine.pool_totals().unwrap(), None)
            .with_component(&component)
            .unwrap();
        for blocked in &*admission.gates {
            let _blocker = Arc::clone(&blocked.slots)
                .acquire_many_owned(blocked.budget)
                .await
                .unwrap();
            let before: Vec<_> = admission
                .gates
                .iter()
                .map(|g| g.slots.available_permits())
                .collect();
            assert!(unconstrained(admission.acquire()).now_or_never().is_none());
            let after: Vec<_> = admission
                .gates
                .iter()
                .map(|g| g.slots.available_permits())
                .collect();
            assert_eq!(
                before, after,
                "cancelled while waiting for {}",
                blocked.name
            );
        }
    }

    #[tokio::test]
    async fn clones_share_resource_capacity_across_backends() {
        let engine = engine(config());
        let small = component(&engine, 1, 0, 1);
        let composed = component(&engine, 2, 0, 2);
        let limits = MessagingLimits::resolve(None, None, &engine);
        let other_backend = limits.clone();
        let mut held = Vec::new();
        for i in 0..14 {
            let (backend, component) = if i < 7 {
                (&limits, &small)
            } else {
                (&other_backend, &composed)
            };
            let identity = AdmissionIdentity::new("review", &format!("workload-{i}"), "component");
            let admission = backend
                .admission(&identity, None)
                .with_component(component)
                .unwrap();
            held.push(
                unconstrained(admission.acquire())
                    .now_or_never()
                    .flatten()
                    .unwrap(),
            );
        }
        // Seven single-memory and seven two-memory deliveries spend all 21.
        let identity = AdmissionIdentity::new("review", "extra", "component");
        let extra = limits
            .admission(&identity, None)
            .with_component(&small)
            .unwrap();
        assert!(unconstrained(extra.acquire()).now_or_never().is_none());
        drop(held.pop());
        assert!(
            unconstrained(extra.acquire())
                .now_or_never()
                .flatten()
                .is_some()
        );
    }
    #[tokio::test]
    async fn resource_waits_use_the_same_deadline_and_release_permits() {
        use crate::plugin::wasmcloud_messaging::Admitted;
        use std::time::Duration;

        let engine = engine(config());
        let component = component(&engine, 2, 1, 4);
        let limits =
            MessagingLimits::resolve(None, None, &engine).with_admission_wait(Duration::ZERO);
        let identity = AdmissionIdentity::new("review", "timeout", "component");
        let admission = limits
            .admission(&identity, None)
            .with_component(&component)
            .unwrap();
        let memories = &admission.pool.as_ref().unwrap().gates[1];
        let blocker = Arc::clone(&memories.slots)
            .acquire_many_owned(memories.budget)
            .await
            .unwrap();
        assert!(matches!(
            admission.acquire_before_deadline("component", "test").await,
            Admitted::Shed
        ));
        drop(blocker);
        assert!(
            unconstrained(admission.acquire())
                .now_or_never()
                .flatten()
                .is_some()
        );
    }

    #[test]
    fn imported_core_modules_take_the_whole_resource_budget() {
        let engine = engine(config());
        let bytes = wat::parse_str(
            r#"(component
            (core type $m (module))
            (import "module" (core module $m (type $m)))
            (core instance (instantiate $m))
        )"#,
        )
        .unwrap();
        let component = Component::new(engine.inner(), bytes).unwrap();
        assert!(component.resources_required().is_none());
        let admission = PoolAdmission::new(engine.pool_totals().unwrap(), None)
            .with_component(&component)
            .unwrap();
        for (gate, cost) in admission.gates.iter().zip(admission.costs) {
            assert_eq!(cost, gate.budget);
        }
    }
}
