use wasmcloud_tracing::{Counter, Histogram, KeyValue, Meter, Unit};

/// `HostMetrics` encapsulates the set of metrics emitted by the wasmcloud host
#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct HostMetrics {
    /// Represents the time it took for each handle_rpc_message invocation in nanoseconds.
    pub handle_rpc_message_duration_ns: Histogram<u64>,
    /// The count of the number of times an actor was invoked.
    pub actor_invocations: Counter<u64>,
    /// The count of the number of times an actor invocation resulted in an error.
    pub actor_errors: Counter<u64>,

    /// The host's ID.
    // TODO this is actually configured as an InstrumentationScope attribute on the global meter,
    // but we don't really hve a way of getting at those. We should figure out a way to get at that
    // information so we don't have to duplicate it here.
    pub host_id: String,
    /// The host's lattice ID.
    // Eventually a host will be able to support multiple lattices, so this will need to either be
    // removed or metrics will need to be scoped per-lattice.
    pub lattice_id: String,
}

impl HostMetrics {
    /// Construct a new [`HostMetrics`] instance for accessing the various wasmcloud host metrics linked to the provided meter.
    #[must_use]
    pub fn new(meter: &Meter, host_id: String, lattice_id: String) -> Self {
        let wasmcloud_host_handle_rpc_message_duration_ns = meter
            .u64_histogram("wasmcloud_host.handle_rpc_message.duration")
            .with_description("Duration in nanoseconds each handle_rpc_message operation took")
            .with_unit(Unit::new("nanoseconds"))
            .init();

        let actor_invocation_count = meter
            .u64_counter("wasmcloud_host.actor.invocations")
            .with_description("Number of actor invocations")
            .init();

        let actor_error_count = meter
            .u64_counter("wasmcloud_host.actor.invocation.errors")
            .with_description("Number of actor errors")
            .init();

        Self {
            handle_rpc_message_duration_ns: wasmcloud_host_handle_rpc_message_duration_ns,
            actor_invocations: actor_invocation_count,
            actor_errors: actor_error_count,
            host_id,
            lattice_id,
        }
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
        self.actor_invocations.add(1, attributes);
        if error {
            self.actor_errors.add(1, attributes);
        }
    }
}
