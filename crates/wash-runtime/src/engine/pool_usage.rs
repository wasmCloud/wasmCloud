//! How full the pooling allocator is, and what to say when it is full.
//!
//! Reaching a pool total does not fail a workload. It fails every
//! instantiation that needs one more slot, while the workload goes on
//! reporting that it is running — so a host whose pool is exhausted serves an
//! error on every request and looks healthy. wasmtime's own message names the
//! total that was hit but not the knob that raises it, nor the workload that
//! asked.
//!
//! This module gives an embedder two readings to report that with:
//! [`PoolUsage`], a live snapshot of every total against what is in use, and
//! [`pool_refusals`], how many instantiations the pool has refused. It also
//! annotates the refusal itself ([`explain_instantiate_failure`]) so the
//! error a caller sees says what to change.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wasmtime::{PoolConcurrencyLimitError, PoolingAllocationConfig, PoolingAllocatorMetrics};

use crate::engine::ctx::SharedCtx;
use crate::observability::rate_limit::RateLimitedWarn;

/// One pool total and how much of it is handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSlots {
    pub in_use: u64,
    pub total: u64,
}

impl PoolSlots {
    /// Whether the next request for this kind of slot will be refused.
    pub fn is_exhausted(&self) -> bool {
        self.in_use >= self.total
    }
}

/// The totals a pool was built with, read back from the installed
/// [`PoolingAllocationConfig`] so environment overrides are reflected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolTotals {
    core_instances: u32,
    component_instances: u32,
    pub(super) memories: u32,
    pub(super) tables: u32,
    pub(super) stacks: u32,
    gc_heaps: u32,
}

impl PoolTotals {
    pub(crate) fn of(config: &PoolingAllocationConfig) -> Self {
        Self {
            core_instances: config.get_total_core_instances(),
            component_instances: config.get_total_component_instances(),
            memories: config.get_total_memories(),
            tables: config.get_total_tables(),
            stacks: config.get_total_stacks(),
            gc_heaps: config.get_total_gc_heaps(),
        }
    }
}

/// Every pool total against what is in use right now.
///
/// A snapshot: a burst that fills the pool and drains between two readings
/// does not show here, which is what [`pool_refusals`] is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolUsage {
    pub core_instances: PoolSlots,
    pub component_instances: PoolSlots,
    pub memories: PoolSlots,
    pub tables: PoolSlots,
    pub stacks: PoolSlots,
    pub gc_heaps: PoolSlots,
}

impl PoolUsage {
    pub(crate) fn read(metrics: &PoolingAllocatorMetrics, totals: &PoolTotals) -> Self {
        let slots = |in_use: u64, total: u32| PoolSlots {
            in_use,
            total: u64::from(total),
        };
        let wide = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        Self {
            core_instances: slots(metrics.core_instances(), totals.core_instances),
            component_instances: slots(metrics.component_instances(), totals.component_instances),
            memories: slots(wide(metrics.memories()), totals.memories),
            tables: slots(wide(metrics.tables()), totals.tables),
            stacks: slots(wide(metrics.stacks()), totals.stacks),
            gc_heaps: slots(wide(metrics.gc_heaps()), totals.gc_heaps),
        }
    }

    /// Each total by the name wasmtime's refusal and the
    /// `WASMTIME_POOLING_TOTAL_*` variables use for it.
    pub fn totals(&self) -> [(&'static str, PoolSlots); 6] {
        [
            ("core instances", self.core_instances),
            ("component instances", self.component_instances),
            ("memories", self.memories),
            ("tables", self.tables),
            ("stacks", self.stacks),
            ("GC heaps", self.gc_heaps),
        ]
    }

    /// Whether any total is fully handed out, so the next instantiation that
    /// needs it will be refused.
    pub fn is_exhausted(&self) -> bool {
        self.totals().iter().any(|(_, slots)| slots.is_exhausted())
    }
}

/// Instantiations refused because a pool total was reached, since this process
/// started.
///
/// Process-wide rather than per engine: the refusal surfaces on a store, which
/// does not know which [`crate::engine::Engine`] built it, and a host runs one.
pub fn pool_refusals() -> u64 {
    REFUSALS.load(Ordering::Relaxed)
}

static REFUSALS: AtomicU64 = AtomicU64::new(0);

/// Lets a refusal through at `warn` a few times a minute, so a pool refusing
/// every request on every core neither logs nor contends on each one.
static REFUSAL_WARN: RateLimitedWarn = RateLimitedWarn::new(Duration::from_secs(10));

/// The context a refusal is annotated with, ahead of its remedy.
macro_rules! exhausted {
    ($remedy:literal) => {
        concat!("host instance pool exhausted. ", $remedy)
    };
}
const EXHAUSTED: &str = exhausted!("");

/// The context naming the knob that raises the total wasmtime names in a
/// refusal. Whole static strings, so annotating a refusal allocates nothing.
fn remedy(kind: &str) -> &'static str {
    match kind {
        "core instances" => exhausted!(
            "Raise --core-instances (or WASMTIME_POOLING_TOTAL_CORE_INSTANCES), or give this \
             host fewer warm instances"
        ),
        "component instances" => exhausted!("Raise WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES"),
        "memories" => exhausted!("Raise WASMTIME_POOLING_TOTAL_MEMORIES"),
        "tables" => exhausted!("Raise WASMTIME_POOLING_TOTAL_TABLES"),
        "fibers" | "stacks" => exhausted!("Raise WASMTIME_POOLING_TOTAL_STACKS"),
        "GC heaps" => exhausted!("Raise WASMTIME_POOLING_TOTAL_GC_HEAPS"),
        _ => exhausted!("Raise the matching WASMTIME_POOLING_TOTAL_* setting"),
    }
}

/// Annotate `err` if it is the pooling allocator refusing an instantiation,
/// count it, and log it (rate-limited); return any other error unchanged.
///
/// Call it at the instantiation itself, with the store the instance was for,
/// so the workload in the message is the one that asked.
pub(crate) fn explain_instantiate_failure(
    data: &SharedCtx,
    err: wasmtime::Error,
) -> wasmtime::Error {
    let Some(refusal) = err.downcast_ref::<PoolConcurrencyLimitError>() else {
        return err;
    };
    // wasmtime keeps the kind private; its message is "maximum concurrent
    // limit of N for KIND reached".
    let message = refusal.to_string();
    let kind = message
        .split_once(" for ")
        .and_then(|(_, rest)| rest.strip_suffix(" reached"))
        .unwrap_or("");
    let remedy = remedy(kind);
    let refusals = REFUSALS.fetch_add(1, Ordering::Relaxed).saturating_add(1);

    let ctx = &data.active_ctx;
    match REFUSAL_WARN.admit() {
        Some(suppressed) => tracing::warn!(
            workload_id = %ctx.workload_id,
            component_id = %ctx.component_id,
            refusals,
            suppressed,
            "host instance pool exhausted ({message}): the instantiation was refused, and the \
             workload keeps running but cannot serve until a slot frees. {}",
            remedy.strip_prefix(EXHAUSTED).unwrap_or(remedy)
        ),
        None => tracing::debug!(
            workload_id = %ctx.workload_id,
            component_id = %ctx.component_id,
            refusals,
            "host instance pool exhausted ({message})"
        ),
    }
    err.context(remedy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(in_use: u64, total: u64) -> PoolSlots {
        PoolSlots { in_use, total }
    }

    #[test]
    fn any_full_total_exhausts_the_pool() {
        let roomy = PoolUsage {
            core_instances: usage(3, 1000),
            component_instances: usage(1, 1000),
            memories: usage(2, 1000),
            tables: usage(2, 1000),
            stacks: usage(1, 1000),
            gc_heaps: usage(0, 1000),
        };
        assert!(!roomy.is_exhausted());
        let full = PoolUsage {
            core_instances: usage(1000, 1000),
            ..roomy
        };
        assert!(full.is_exhausted());
    }

    #[test]
    fn every_kind_wasmtime_names_has_its_own_remedy() {
        for kind in [
            "core instances",
            "component instances",
            "memories",
            "tables",
            "fibers",
            "GC heaps",
        ] {
            assert_ne!(
                remedy(kind),
                remedy("unknown"),
                "{kind} should name its own knob"
            );
        }
    }

    /// The real refusal, from a real pool of one core instance: what a host
    /// past its total actually returns.
    #[tokio::test]
    async fn a_real_refusal_is_annotated_and_counted() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut pooling = PoolingAllocationConfig::default();
        pooling.total_core_instances(1);
        pooling.total_memories(1);
        pooling.max_memory_size(1 << 16);
        let mut config = wasmtime::Config::new();
        config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(
            pooling.clone(),
        ));
        let engine = wasmtime::Engine::new(&config).unwrap();
        let wasm = wat::parse_str("(component (core module $m) (core instance (instantiate $m)))")
            .unwrap();
        let component = wasmtime::component::Component::new(&engine, wasm).unwrap();
        let linker = wasmtime::component::Linker::<SharedCtx>::new(&engine);
        let ctx = |n| crate::engine::ctx::Ctx::builder("pool-workload", n).build();

        let mut first = wasmtime::Store::new(&engine, SharedCtx::new(ctx("first")));
        let _held = linker
            .instantiate_async(&mut first, &component)
            .await
            .unwrap();
        let usage = PoolUsage::read(
            &engine.pooling_allocator_metrics().unwrap(),
            &PoolTotals::of(&pooling),
        );
        assert_eq!(
            usage.core_instances,
            PoolSlots {
                in_use: 1,
                total: 1
            }
        );
        assert!(usage.is_exhausted());

        let mut second = wasmtime::Store::new(&engine, SharedCtx::new(ctx("second")));
        let before = pool_refusals();
        let err = linker
            .instantiate_async(&mut second, &component)
            .await
            .expect_err("a pool of one core instance is full");
        let explained = explain_instantiate_failure(second.data(), err);
        assert!(pool_refusals() > before);
        let text = format!("{explained:#}");
        assert!(text.contains("host instance pool exhausted"), "{text}");
        assert!(text.contains("--core-instances"), "{text}");
        assert!(
            text.contains("maximum concurrent limit of 1 for core instances"),
            "the original refusal is still underneath: {text}"
        );
    }

    #[test]
    fn other_errors_pass_through_unchanged() {
        let ctx = crate::engine::ctx::Ctx::builder("w", "c").build();
        let data = SharedCtx::new(ctx);
        let err = explain_instantiate_failure(&data, wasmtime::format_err!("something else"));
        assert_eq!(format!("{err:#}"), "something else");
    }
}
