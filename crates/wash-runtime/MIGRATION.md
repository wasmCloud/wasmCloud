# Runtime scaling changes

These changes affect embedders and operators upgrading the runtime.

## Workload and HTTP handler API

- `ResolvedWorkload::component_count()` is synchronous. Replace
  `workload.component_count().await` with `workload.component_count()`. The count
  is fixed when the workload is resolved and does not require its component lock.
- `host::http::WorkloadHandles` now stores `Arc<ComponentHandle>` values.
  `ComponentHandle` contains runtime-owned pre-instantiation and pooling state;
  its fields are private. Code that only shares or creates an empty map can
  continue using the alias. Code that populated the old map directly must move
  registration to the `Ingress`/`HttpHandler` workload binding lifecycle. This is
  a source-breaking change, not a drop-in replacement for constructing old map
  entries. Prefer `HostBuilder::with_http_handler` for managed HTTP workloads.

## Log levels

These logs run per request, per guest call or per binding, and moved from INFO
to DEBUG. Run with `RUST_LOG=wash_runtime=debug` to see them again.

- Plugin bindings: "`<interface>` was set, so its default bundle was not
  applied".
- `wasi_otel`: "WASI OTel interfaces bound to component", "WASI OTel unbound
  from workload", the per-call metrics and span summaries ("Processing WASI
  resource metrics", "Successfully processed WASI metrics", "WASI span
  started", "Processing WASI span end", "Successfully exported WASI span") and
  the outer span context lookups.
- `wasi_otel` guest log records ("emitting log record") below WARN severity.
  Guest records at WARN severity are still logged at WARN, and at ERROR or
  FATAL severity at ERROR.
- `wasmcloud:nats`: "opened NATS connection", and the publish ledger at unbind
  when nothing was shed. A ledger that shed publishes is logged at WARN.
- The `wasmcloud.messaging.subscriber` span is a DEBUG span. Subscriber
  warnings carry `workload.id` themselves.
- HTTP ingress warnings for an invalid Host, a routing failure and a Host with
  no bound workload are logged at WARN at most once every 10 seconds each, with
  a `suppressed` count. Repeats in between are logged at DEBUG.

"Starting workload" and "Stopping workload" stay at INFO.
