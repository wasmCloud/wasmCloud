# Runtime scaling changes

These changes affect embedders and operators upgrading from the API before
PR #5695.

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
- A configured process-memory limit is enforced even when guest memory is in
  `count` mode. Use `--max-process-memory off` to disable that separate policy.
  The process guard is sampled admission control: it cannot cap allocations
  already in flight, writes into committed guest memory, or arbitrary plugin
  buffers. Leave headroom and use OS/container limits for a hard boundary.
- OCI disk entries are scoped by credentials. Existing unscoped entries are
  ignored and will need to be fetched again. `Never` permits offline reuse only
  inside the same credential context. `Always` resolves the registry afresh
  after any prior pull has completed; genuinely concurrent pulls can coalesce.
  CA environment paths and their bundle changes apply to subsequent pulls and invalidate client
  and completed-result reuse. Docker helper results retain their documented
  30-second reuse window.
- Warm density experiments require a P3 HTTP handler. The default fixture now
  uses `wasi:http/handler@0.3.0`. P2 warm runs and unverified external-image warm
  runs are rejected. Pass a local P3 `--wasm`, or use `--flavour scale-to-zero`
  for an external image.
- Density JSON includes warm-up failures and an explicit `idle-unbounded` CPU
  capacity value. Missing numeric estimates mean insufficient measurement;
  they do not establish infinite capacity. Text reports render them as `?`.
  Request rates and latency percentiles describe successful responses, with
  failures reported separately.

The OCI implementation separates disk-cache policy, credential resolution,
client/trust snapshots, and coalescing into `src/oci/`. Process-footprint readers
and metric registration live in `src/engine/process_memory/`. The in-process
density benchmark separates settings, load generation, estimation, and reports
under `benches/scale_density/`; common sampling and memory-budget helpers are in
the small `bench-tools` library.
