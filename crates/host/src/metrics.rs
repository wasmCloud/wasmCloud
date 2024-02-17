use wasmcloud_tracing::{Histogram, Meter};

/// `HostMetrics` encapsulates the set of metrics emitted by the wasmcloud host
#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct HostMetrics {
    /// Represents the time it took for each handle_rpc_message invocation in nanoseconds.
    pub wasmcloud_host_handle_rpc_message_duration_ns: Histogram<u64>,
}

impl HostMetrics {
    /// Construct a new [`HostMetrics`] instance for accessing the various wasmcloud host metrics linked to the provided meter.
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        let wasmcloud_host_handle_rpc_message_duration_ns = meter
            .u64_histogram("wasmcloud_host.handle_rpc_message.duration_ns")
            .with_description("Duration in nanoseconds each handle_rpc_message operation took")
            .init();

        Self {
            wasmcloud_host_handle_rpc_message_duration_ns,
        }
    }
}
