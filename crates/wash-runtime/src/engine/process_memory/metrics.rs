//! Provider-scoped registration of the shared process readings.
use super::{ProcessMemoryGuard, live_guards};

impl ProcessMemoryGuard {
    /// Publish the process's memory as OpenTelemetry gauges, read on the
    /// exporter's own schedule like the guest memory budget's.
    ///
    /// Register with the current provider, including after a provider is
    /// installed or replaced. A process-wide Once would bind these gauges to
    /// the first provider forever, even if it was a no-op. Identical gauges
    /// share a stream within the provider; callbacks report process-wide
    /// values without engine-specific attributes.
    pub(crate) fn register_metrics(meter: &opentelemetry::metrics::Meter) {
        macro_rules! observe {
            ($name:literal, $doc:literal, $read:expr) => {{
                let _ = meter
                    .u64_observable_gauge($name)
                    .with_description($doc)
                    .with_unit("By")
                    .with_callback(|observer| {
                        #[allow(clippy::redundant_closure_call)]
                        let value = $read();
                        if let Some(value) = value {
                            observer.observe(value, &[]);
                        }
                    })
                    .build();
            }};
        }
        observe!(
            "process_memory.footprint",
            "This process's memory footprint, counting swapped and compressed pages",
            || live_guards(|guards| guards.map(|g| g.footprint_bytes()).max())
        );
        observe!(
            "process_memory.high_water",
            "The largest memory footprint sampled since this host started",
            || live_guards(|guards| guards.map(|g| g.high_water_bytes()).max())
        );
        observe!(
            "process_memory.limit",
            "This host's max-process-memory",
            || live_guards(|guards| guards.map(|g| g.limit_bytes()).min())
        );
    }
}
