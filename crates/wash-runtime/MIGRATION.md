# Runtime scaling changes

These changes affect embedders and operators upgrading the runtime.

## Workload and HTTP handler API

- `ResolvedWorkload::component_count()` is synchronous. Replace
  `workload.component_count().await` with `workload.component_count()`. The count
  is fixed when the workload is resolved and does not require its component lock.
- `ResolvedWorkload::components()` still exposes a writable map for updates to
  existing component metadata. Adding or removing components after resolution
  is unsupported: the cached count and bindings would no longer describe the
  same workload. Resolve a new workload to change component membership.
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

## Process memory limit

- The process-memory limit is now on by default for every `EngineBuilder`
  embedder, not only `wash host`. Unset, it is 90% of the cgroup memory limit,
  or of physical memory where there is no cgroup. Past it, guest memory growth
  (including the initial memory of new instances) and new workload starts are
  refused. Call `EngineBuilder::without_max_process_memory()`, or pass
  `--max-process-memory off` (`WASH_HOST_MAX_PROCESS_MEMORY=off`) to
  `wash host`, to turn it off. `with_max_process_memory_bytes` sets a limit of
  your own instead of the derived one.
- Automatic guest and process limits use the smallest readable cgroup limit
  for the process and its visible ancestors, capped at physical RAM. This
  includes systemd service and parent-slice limits, rather than only the
  mount-root limit. Detection uses `/proc/self/cgroup` with the conventional
  `/sys/fs/cgroup` (v2) and `/sys/fs/cgroup/memory` (v1) mounts. Use explicit
  limits for other mount layouts or limits hidden outside the cgroup namespace.
- A configured process-memory limit is enforced even when guest memory is in
  `count` mode. Use `--max-process-memory off` to disable that separate policy.
  The process guard is sampled admission control: it cannot cap allocations
  already in flight, writes into committed guest memory, or arbitrary plugin
  buffers. Leave headroom and use OS/container limits for a hard boundary.
- Linux measures anonymous resident memory plus swap, excluding shared file
  and shmem mappings to avoid counting a shared Wasmtime memory image once
  per instance. Older kernels without `RssAnon` fall back to total RSS plus
  swap. macOS includes compressed pages; other platforms use resident memory.
- Refusals normally stop below 95% of the limit. They also stop after a full
  minute without growth below the limit, or after five consecutive failed
  periodic reads. A successful reading resumes enforcement. Initial read
  failures are retried by the shared process sampler.
- Process readings are exported as `process_memory.footprint`,
  `process_memory.high_water` and `process_memory.limit` gauges, in bytes.
  `process_memory.refused` counts guest growth refused by the process limit,
  independently of guest-budget refusals. Registering another engine after
  installing or replacing the meter provider registers these gauges with it.

## Pool diagnostics

The default pool keeps Wasmtime's 4 GiB reservation and 32 MiB guard per
memory. Lowering `--default-heap-memory` limits guest growth without reducing
that reservation; a ceiling above 4 GiB is incompatible with the default
reservation. Reduce the pool's slot count to reduce its address-space use.

Startup diagnostics use the installed pool counts, including
`WASMTIME_POOLING_TOTAL_*` overrides. Linux mapping estimates account for
memory, stack and table totals independently; stack guards can exhaust
`vm.max_map_count` during engine construction, before any workload starts.
These figures are estimates and leave room for the process's other mappings.

`Engine::pool_usage()` reports used and total slots. `pool_usage::pool_refusals()`
counts refusals across the process. Instantiation errors retain their original
cause and add the relevant pool setting; warnings are rate-limited, with
repeats at DEBUG. A `poolSize` on a WASIp2 HTTP handler now warns that HTTP
instances are not reused; use WASIp3 for warm HTTP instances.
