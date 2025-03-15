use std::sync::Arc;
use std::time::Duration;

use sysinfo::System;
use tokio::task::JoinHandle;
use wasmcloud_tracing::{Counter, Histogram, KeyValue, Meter, ObservableGauge};

const DEFAULT_REFRESH_TIME: Duration = Duration::from_secs(5);

/// `HostMetrics` encapsulates the set of metrics emitted by the wasmcloud host
#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct HostMetrics {
    /// Represents the time it took for each handle_rpc_message invocation in nanoseconds.
    pub handle_rpc_message_duration_ns: Histogram<u64>,
    /// The count of the number of times an component was invoked.
    pub component_invocations: Counter<u64>,
    /// The count of the number of times an component invocation resulted in an error.
    pub component_errors: Counter<u64>,

    /// The total amount of available system memory in bytes.
    pub system_total_memory_bytes: ObservableGauge<u64>,
    /// The total amount of used system memory in bytes.
    pub system_used_memory_bytes: ObservableGauge<u64>,
    /// The total cpu usage.
    pub system_cpu_usage: ObservableGauge<f64>,

    /// The host's ID.
    // TODO this is actually configured as an InstrumentationScope attribute on the global meter,
    // but we don't really have a way of getting at those. We should figure out a way to get at that
    // information so we don't have to duplicate it here.
    pub host_id: String,
    /// The host's lattice ID.
    // Eventually a host will be able to support multiple lattices, so this will need to either be
    // removed or metrics will need to be scoped per-lattice.
    pub lattice_id: String,

    // Task handle for dropping when the metrics are no longer needed.
    _refresh_task_handle: Arc<RefreshWrapper>,
}

struct SystemMetrics {
    system_total_memory_bytes: u64,
    /// The total amount of used system memory in bytes.
    system_used_memory_bytes: u64,
    /// The total cpu usage.
    system_cpu_usage: f64,
}

/// A helper struct for encapsulating the system metrics that should be wrapped in an Arc.
///
/// When the final reference is removed, the drop will abort the watch task. This allows the metrics
/// to be clonable
#[derive(Debug)]
struct RefreshWrapper(JoinHandle<()>);

impl Drop for RefreshWrapper {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl HostMetrics {
    /// Construct a new [`HostMetrics`] instance for accessing the various wasmcloud host metrics
    /// linked to the provided meter.
    ///
    /// The `refresh_time` is optional and defaults to 5 seconds. This time is used to configure how
    /// often system level metrics are refreshed
    pub fn new(
        meter: &Meter,
        host_id: String,
        lattice_id: String,
        refresh_time: Option<Duration>,
    ) -> anyhow::Result<Self> {
        let wasmcloud_host_handle_rpc_message_duration_ns = meter
            .u64_histogram("wasmcloud_host.handle_rpc_message.duration")
            .with_description("Duration in nanoseconds each handle_rpc_message operation took")
            .with_unit("nanoseconds")
            .build();

        let component_invocation_count = meter
            .u64_counter("wasmcloud_host.component.invocations")
            .with_description("Number of component invocations")
            .build();

        let component_error_count = meter
            .u64_counter("wasmcloud_host.component.invocation.errors")
            .with_description("Number of component errors")
            .build();

        let mut system = System::new();
        // Get the initial metrics
        system.refresh_memory();
        system.refresh_cpu_usage();
        let initial_metrics = SystemMetrics {
            system_total_memory_bytes: system.total_memory(),
            system_used_memory_bytes: system.used_memory(),
            system_cpu_usage: system.global_cpu_usage() as f64,
        };
        let (tx, rx) = tokio::sync::watch::channel(initial_metrics);

        let refresh_time = refresh_time.unwrap_or(DEFAULT_REFRESH_TIME);

        let refresh_task_handle = tokio::spawn(async move {
            loop {
                system.refresh_memory();
                system.refresh_cpu_usage();

                tx.send_modify(|current| {
                    current.system_total_memory_bytes = system.total_memory();
                    current.system_used_memory_bytes = system.used_memory();
                    current.system_cpu_usage = system.global_cpu_usage() as f64;
                });
                tokio::time::sleep(refresh_time).await;
            }
        });
        // System Memory
        let system_memory_total_bytes = meter
            .u64_observable_gauge("wasmcloud_host.process.memory.total.bytes")
            .with_description("The total amount of memory in bytes")
            .with_unit("bytes")
            .with_callback({
                let rx = rx.clone();
                move |observer| {
                    let metrics = rx.borrow();
                    observer.observe(metrics.system_total_memory_bytes, &[]);
                }
            })
            .build();

        let system_memory_used_bytes = meter
            .u64_observable_gauge("wasmcloud_host.process.memory.used.bytes")
            .with_description("The used amount of memory in bytes")
            .with_unit("bytes")
            .with_callback({
                let rx_clone = rx.clone();
                move |observer| {
                    let metrics = rx_clone.borrow();
                    observer.observe(metrics.system_used_memory_bytes, &[]);
                }
            })
            .build();

        // System CPU
        let system_cpu_usage = meter
            .f64_observable_gauge("wasmcloud_host.process.cpu.usage")
            .with_description("The CPU usage of the process")
            .with_unit("percentage")
            .with_callback({
                let rx = rx.clone();
                move |observer| {
                    let metrics = rx.borrow();
                    observer.observe(metrics.system_cpu_usage, &[]);
                }
            })
            .build();

        Ok(Self {
            handle_rpc_message_duration_ns: wasmcloud_host_handle_rpc_message_duration_ns,
            component_invocations: component_invocation_count,
            component_errors: component_error_count,
            system_total_memory_bytes: system_memory_total_bytes,
            system_used_memory_bytes: system_memory_used_bytes,
            system_cpu_usage,
            host_id,
            lattice_id,
            _refresh_task_handle: Arc::new(RefreshWrapper(refresh_task_handle)),
        })
    }

    /// Record the result of invoking a component, including the elapsed time, any attributes, and whether the invocation resulted in an error.
    pub(crate) fn record_component_invocation(
        &self,
        elapsed: u64,
        attributes: &[KeyValue],
        error: bool,
    ) {
        self.handle_rpc_message_duration_ns
            .record(elapsed, attributes);
        self.component_invocations.add(1, attributes);
        if error {
            self.component_errors.add(1, attributes);
        }
    }
}
