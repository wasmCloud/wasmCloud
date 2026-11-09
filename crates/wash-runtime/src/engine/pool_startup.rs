//! One allocation-failure fallback for an automatically sized pool. Explicit
//! totals and per-slot limits remain authoritative on both attempts.

use anyhow::Context;
use wasmtime::{Config, InstanceAllocationStrategy, PoolingAllocationConfig};

use super::{derived_pool_totals, host_memory::WASMTIME_DEFAULT_MAX_MEMORIES};

pub(super) fn derived_fallback(
    pooling: &PoolingAllocationConfig,
    derived: bool,
    core_instances_named: bool,
    has_override: impl Fn(&str) -> bool,
) -> Option<PoolingAllocationConfig> {
    if !derived
        || has_override("WASMTIME_POOLING_TOTAL_MEMORIES")
        || pooling.get_total_memories() <= WASMTIME_DEFAULT_MAX_MEMORIES
    {
        return None;
    }

    let totals = derived_pool_totals(WASMTIME_DEFAULT_MAX_MEMORIES);
    let mut fallback = pooling.clone();
    fallback.total_memories(totals.memories);
    if !core_instances_named && !has_override("WASMTIME_POOLING_TOTAL_CORE_INSTANCES") {
        fallback.total_core_instances(totals.core_instances);
    }
    if !has_override("WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES") {
        fallback.total_component_instances(totals.component_instances);
    }
    if !has_override("WASMTIME_POOLING_TOTAL_TABLES") {
        fallback.total_tables(totals.tables);
    }
    if !has_override("WASMTIME_POOLING_TOTAL_STACKS") {
        fallback.total_stacks(totals.stacks);
    }
    if !has_override("WASMTIME_POOLING_TOTAL_GC_HEAPS") {
        fallback.total_gc_heaps(totals.gc_heaps);
    }
    Some(fallback)
}

fn allocation_failure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<wasmtime::OutOfMemory>()
            || errno_out_of_memory(cause)
            || cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::OutOfMemory)
    })
}

#[cfg(unix)]
fn errno_out_of_memory(cause: &(dyn std::error::Error + 'static)) -> bool {
    cause
        .downcast_ref::<rustix::io::Errno>()
        .is_some_and(|error| *error == rustix::io::Errno::NOMEM)
}

/// Windows mapping failures arrive as `std::io::Error`s, checked above.
#[cfg(not(unix))]
fn errno_out_of_memory(_: &(dyn std::error::Error + 'static)) -> bool {
    false
}

pub(super) fn build_with_fallback<T>(
    config: &mut Config,
    fallback: Option<PoolingAllocationConfig>,
    mut build: impl FnMut(&Config) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let initial = match build(config) {
        Ok(engine) => return Ok(engine),
        Err(error) => error,
    };
    let Some(fallback) = fallback.filter(|_| allocation_failure(&initial)) else {
        return Err(initial);
    };
    tracing::warn!(
        error = %format!("{initial:#}"),
        memories = fallback.get_total_memories(),
        "automatically sized pool allocation failed; retrying once with the default memory count"
    );
    let memories = fallback.get_total_memories();
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(fallback));
    build(config).with_context(|| {
        format!(
            "pool retry with {memories} memories also failed; initial automatic pool error: {initial:#}"
        )
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn pool(memories: u32) -> PoolingAllocationConfig {
        let mut pool = PoolingAllocationConfig::default();
        let totals = derived_pool_totals(memories);
        pool.total_memories(totals.memories)
            .total_core_instances(totals.core_instances)
            .total_component_instances(totals.component_instances)
            .total_tables(totals.tables)
            .total_stacks(totals.stacks)
            .total_gc_heaps(totals.gc_heaps);
        pool
    }

    /// The error a failed pool mapping reports on this platform.
    fn mapping_out_of_memory() -> wasmtime::Error {
        #[cfg(unix)]
        let error = rustix::io::Errno::NOMEM;
        #[cfg(not(unix))]
        let error = std::io::Error::from(std::io::ErrorKind::OutOfMemory);
        wasmtime::Error::new(error)
    }

    #[test]
    fn only_automatic_counts_above_the_floor_have_a_fallback() {
        let pool = pool(32_513);
        assert!(derived_fallback(&pool, false, false, |_| false).is_none());
        assert!(
            derived_fallback(&pool, true, false, |key| {
                key == "WASMTIME_POOLING_TOTAL_MEMORIES"
            })
            .is_none()
        );
        assert!(derived_fallback(&self::pool(1000), true, false, |_| false).is_none());
    }

    #[test]
    fn fallback_resizes_automatic_totals_and_preserves_explicit_totals() {
        let mut pool = pool(32_513);
        pool.max_memory_size(128 << 20).table_elements(1234);
        let fallback = derived_fallback(&pool, true, false, |_| false).unwrap();
        assert_eq!(
            super::super::pool_usage::PoolTotals::of(&fallback),
            derived_pool_totals(1000)
        );
        let pinned = derived_fallback(&pool, true, true, |key| {
            key != "WASMTIME_POOLING_TOTAL_MEMORIES"
        })
        .unwrap();
        assert_eq!(pinned.get_total_memories(), 1000);
        assert_eq!(pinned.get_total_tables(), pool.get_total_tables());
        assert_eq!(pinned.get_total_stacks(), pool.get_total_stacks());
        assert_eq!(pinned.get_total_gc_heaps(), pool.get_total_gc_heaps());
        assert_eq!(
            pinned.get_total_component_instances(),
            pool.get_total_component_instances()
        );
        assert_eq!(
            pinned.get_total_core_instances(),
            pool.get_total_core_instances()
        );
        assert_eq!(pinned.get_max_memory_size(), pool.get_max_memory_size());
        assert_eq!(pinned.get_table_elements(), 1234);
    }

    #[test]
    fn named_and_environment_core_counts_are_preserved_independently() {
        let pool = pool(32_513);
        for named in [true, false] {
            let fallback = derived_fallback(&pool, true, named, |key| {
                !named && key == "WASMTIME_POOLING_TOTAL_CORE_INSTANCES"
            })
            .unwrap();
            assert_eq!(
                fallback.get_total_core_instances(),
                pool.get_total_core_instances()
            );
            assert_eq!(fallback.get_total_stacks(), 1000);
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn retries_allocation_failure_once_with_the_same_memory_layout() {
        let mut config = Config::new();
        config
            .wasm_gc(false)
            .memory_reservation(4 << 30)
            .memory_guard_size(32 << 20);
        let original = pool(32_513);
        let fallback = derived_fallback(&original, true, false, |_| false);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(original));
        let mut attempts = 0;
        let engine = build_with_fallback(&mut config, fallback, |config| {
            attempts += 1;
            if attempts == 1 {
                return Err(mapping_out_of_memory()
                    .context("failed to create memory pool mapping")
                    .into());
            }
            super::super::on_signal_quiet_thread(|| Ok(wasmtime::Engine::new(config)?))
        })
        .unwrap();
        assert_eq!(
            engine.get_pooling_config().unwrap().get_total_memories(),
            1000
        );
        assert_eq!(engine.get_memory_reservation(), 4 << 30);
        assert_eq!(engine.get_memory_guard_size(), 32 << 20);
        assert_eq!(attempts, 2);
    }

    #[test]
    fn recognizes_wasmtime_oom_without_retrying_other_system_errors() {
        let oom = wasmtime::Error::new(wasmtime::OutOfMemory::new(4096));
        assert!(allocation_failure(&oom.into()));
        #[cfg(unix)]
        assert!(!allocation_failure(&rustix::io::Errno::ACCESS.into()));
        assert!(!allocation_failure(
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied).into()
        ));
    }

    #[test]
    fn successful_builds_and_configuration_errors_are_not_retried() {
        for success in [true, false] {
            let mut config = Config::new();
            let mut attempts = 0;
            let result = build_with_fallback(&mut config, Some(pool(1000)), |_| {
                attempts += 1;
                if success {
                    Ok(())
                } else {
                    anyhow::bail!("invalid memory layout")
                }
            });
            assert_eq!(result.is_ok(), success);
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn allocation_errors_without_an_automatic_fallback_are_not_retried() {
        let mut config = Config::new();
        let mut attempts = 0;
        let result = build_with_fallback::<()>(&mut config, None, |_| {
            attempts += 1;
            Err(mapping_out_of_memory().into())
        });
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn failed_retry_retains_both_errors_and_never_retries_again() {
        let mut config = Config::new();
        let mut attempts = 0;
        let error = build_with_fallback::<()>(&mut config, Some(pool(1000)), |_| {
            attempts += 1;
            Err(
                anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::OutOfMemory))
                    .context(format!("attempt {attempts}")),
            )
        })
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("attempt 1") && error.contains("attempt 2"),
            "{error}"
        );
        assert_eq!(attempts, 2);
    }
}
