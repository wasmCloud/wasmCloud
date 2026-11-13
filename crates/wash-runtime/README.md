# wash-runtime

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](../../LICENSE)

**wash-runtime** is an opinionated Wasmtime wrapper that provides a runtime and workload API for executing WebAssembly components. It offers a simplified interface for embedding Wasm component execution in Rust applications with built-in support for WASI interfaces.

## Features

- **Component Model Runtime**: Native support for WebAssembly Component Model using Wasmtime
- **WASI Interface Support**: Built-in plugins for WASI HTTP, Config, Logging, Blobstore, and Key-Value
- **Workload API**: High-level API for managing and executing component workloads
- **Plugin System**: Extensible architecture for custom capability providers
- **OCI Integration**: Optional support for pulling components from OCI registries
- **Hot-Reload Ready**: Designed for development workflows with fast iteration

## Component plugin secret migration

Host plugins that consume plugin-backed binding secrets must rebuild against
`wasmcloud:host@0.1.6`, export `workload-lifecycle`, and import `binding-secrets`.
During `on-workload-bind`, call `binding-secrets.get` for the binding, read the
returned handles through `wasmcloud:secrets/reveal@2.1.0`, and drop them when done.
Secret keys are omitted from `interface-binding.config`; plain text remains there.

This changes compatibility with plugins that read secrets as config strings.
The host refuses secret-bearing declarations for plugins without these interfaces.
A timed-out bind loses access to its secrets, and the workload ID cannot bind
again until the unfinished hook and its cleanup return. Bind rejections with
secrets withhold guest error details.

## Usage

### Basic Example

```rust
use std::sync::Arc;
use std::collections::HashMap;

use wash_runtime::{
    engine::Engine,
    host::{HostBuilder, HostApi,
      http::{Ingress, DynamicRouter},
  },
    plugin::{
        wasi_config::DynamicConfig,
    },
    types::{WorkloadStartRequest, Workload},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a Wasmtime engine
    let engine = Engine::builder().build()?;

    // Configure plugins
    let http_router = DynamicRouter::default();
    let ingress = Ingress::new(http_router, "127.0.0.1:8080".parse()?).await?;
    let wasi_config_plugin = DynamicConfig::default();

    // Build and start the host
    let host = HostBuilder::new()
        .with_engine(engine)
        // if a handler is not provided, a 'deny all' implementation
        // will be used for outgoing http requests
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(wasi_config_plugin))?
        .build()?;

    let host = host.start().await?;

    // Start a workload
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    Ok(())
}
```

### Custom caches

The runtime has two kinds of retention, each behind its own trait and its own
handle. Replacing one leaves the other on its default.

- `cache::Cache` retains compiled components. An engine takes it:
  `Engine::builder().with_cache(Arc<dyn Cache>)`. The default, `DefaultCache`,
  is bounded and in memory; the engine builder's compilation capacity and idle
  timeout configure it.
- `cache::OciCache` (with the `oci` feature) retains pulled artifacts, completed
  pulls and credential-helper answers. A host takes it:
  `Host::builder().with_oci_cache(Arc<dyn OciCache>)`. The default,
  `DefaultOciCache`, keeps completed pulls and helper answers in memory and,
  given a directory, artifacts in the credential-scoped disk cache.
  `HostConfig::oci_cache_dir` selects that directory for the default backend.

Neither handle's type depends on which features are enabled, so a `Cache`
implementation keeps compiling when another crate in the build turns `oci` on.

Both traits use `async-trait`; storage methods return `anyhow::Result`, with
`Ok(None)` indicating a miss. Every method defaults to no retention.
`Cache::name()` is a synchronous diagnostic label, defaulting to the Rust type
name; names need not be unique and do not determine isolation.

This backend replaces compiled-component retention only. OCI retention is
untouched, so the host still caches artifacts in `oci_cache_dir`:

```rust,no_run
use std::{collections::HashMap, sync::Arc};
use wash_runtime::{
    cache::{Cache, CacheKey, CacheValue},
    engine::Engine,
    host::{Host, HostConfig},
};

/// Illustrative only: a real backend bounds what it keeps.
#[derive(Default)]
struct MemoryCache(tokio::sync::RwLock<HashMap<CacheKey, CacheValue>>);

#[async_trait::async_trait]
impl Cache for MemoryCache {
    async fn get_compiled(&self, key: &CacheKey) -> anyhow::Result<Option<CacheValue>> {
        Ok(self.0.read().await.get(key).cloned())
    }

    async fn put_compiled(&self, key: &CacheKey, value: CacheValue) -> anyhow::Result<()> {
        self.0.write().await.insert(key.clone(), value);
        Ok(())
    }
}

# fn example() -> anyhow::Result<()> {
let engine = Engine::builder()
    .with_cache(Arc::new(MemoryCache::default()))
    .build()?;
let host = Host::builder()
    .with_engine(engine)
    .with_config(HostConfig {
        oci_cache_dir: Some("/var/cache/wash/oci".into()),
        ..Default::default()
    })
    .build()?;
# Ok(())
# }
```

A host supplied with an engine uses that engine's `Cache`, and the handle from
`Engine::cache()` stays valid across host construction. `HostBuilder::with_cache`
selects the backend for an engine the host builds itself, and rejects a handle
that differs from a supplied engine's.

An `OciCache` implementation overrides only what it retains; anything left at
its default is not retained at all. To replace one kind of OCI retention and
keep the built-in behaviour for the rest, hold a `DefaultOciCache` and delegate
the other methods to it. An explicit OCI backend owns its storage: building a
host with both `with_oci_cache` and `oci_cache_dir` is an error, not a silently
unused directory. Give the directory to the backend instead
(`DefaultOciCache::default().with_directory(..)`).

`Host::oci_cache()` returns the backend the host pulls through. Standalone OCI
operations accept one with `OciConfig::with_cache`; without it, calls naming
the same `cache_dir` share a default backend with each other and with a host
configured with that `oci_cache_dir`, so plugins pulled before a host is built
need no extra wiring. A backend with scheduled expiry implements `oci_maintain`
and returns true from `oci_needs_maintenance`; `oci_reserved_paths` protects a
backend's local storage from guest volume mounts.

Compiled keys contain a process-local engine identity and a digest. Store the
whole `CacheValue`, including its metadata; compiled values cannot be reused
across engines or persisted as arbitrary machine code. Running workloads retain
their compiles independently of eviction. The runtime coordinates concurrent
misses, so a no-retention implementation does not need its own compilation locks.
Host component plugins are compiled once and are not cached.

OCI backends use `ArtifactKey::storage_key` and `credential_proof` with a
backend-owned secret to preserve principal, transport, trust, and offline
credential isolation. Publish bytes and digest together and update credential
proofs only after registry acceptance. Completed pulls expire within five seconds
and helper results within thirty seconds. `Always` bypasses completed-result
retention.

A failing cache costs the work it would have saved, not the operation. An error
from compiled-component, completed-pull or credential retention is logged and
treated as a miss, or as a write that kept nothing: the workload still starts
and the image is still pulled. OCI artifact errors fail the pull or push, as
the disk cache's always have, since an unreadable entry may be the only copy
`Never` is allowed to use. Credential-proof refresh and background maintenance
log their failures.

`Engine::initialize_workload` and `prepare_host_component` are now async: callers
must `.await` them. Exhaustive `OciConfig` literals also need the new `cache`
field (`None` preserves default selection), or a `..Default::default()` tail.
Cache I/O runs asynchronously; hashing and compilation run on blocking workers.
Cancelling a start drops its pending cache lookup. A compilation already running
finishes and is published, so a retry finds it, and callers arriving meanwhile
join it. OCI pull deadlines include cache I/O; the push timeout bounds the
registry push alone.

Connection pools, quota/accounting state, DNS authorization records, linked
instances, telemetry memoization, and process configuration remain owned by their
existing runtime subsystems. CLI WIT package caching remains with its package
client; it is not host artifact storage.

### Embedded commands

An embedder can resolve a workload and run one of its components as a command:

```rust,ignore
use wash_runtime::engine::command::CommandOptions;

let resolved = engine.initialize_workload("shell-run", workload).await?
    .resolve(Some(&plugins), &bindings, &host_ref, &meters).await?;
let options = CommandOptions::new()
    .args(["agent", "hello"])
    .stdin(wasmtime_wasi::cli::stdin());
let result = resolved.run_command(&component_id, options).await;
let cleanup = resolved.unbind_all_plugins().await;
let exit_code = result?.exit_code;
cleanup?;
```

`run_command` runs a WASIp2 or WASIp3 `wasi:cli/run` exactly once in a fresh
store, using the resolved workload's grants, plugins, environment and volumes.
Arguments include the program name.
Stdin defaults to EOF; stdout and stderr default to the host's streams.
All three accept custom WASI streams, including memory pipes and terminal adapters.
Successful and unsuccessful command returns become exit codes 0 and 1;
`wasi:cli/exit` preserves its exit code, and traps return errors.
Dropping the invocation future drops its store.
Services retain their existing supervision behavior.

### Structured streams across component-plugin stores

Scalar and string stream/future payloads work without configuration.
For application-defined records, variants, lists, options and results,
register the generated Wasmtime binding type before loading plugins or resolving
workloads:

```rust,ignore
// `payload_type` is the Type from a compiled component's StreamType::ty()
// or FutureType::ty(), e.g. the record used by stream<entry>.
engine.stream_payloads().register::<bindings::Entry>(payload_type)?;
```

Registrations belong to the engine and propagate to both workload and plugin
stores, including restarted plugin stores.
The bridge uses the same bounded, backpressured pumps as scalar streams;
it does not collect a stream before returning it.
Wasmtime checks the registered Rust type against each source handle before
transferring it.
Payloads containing resources, nested streams/futures or error contexts are
rejected because their handles belong to the source store.
Native Rust plugins can use their generated typed bindings directly and need
no cross-store registration.

### Cargo Features

The crate supports the following cargo features:

- `oci`: OCI registry integration for pulling components
- `wasi-blobstore` (default): Blob storage interface
- `wasi-config` (default): Runtime configuration interface
- `wasi-http` (default): HTTP client and server support via `wasmtime-wasi-http`
- `wasi-keyvalue` (default): Key-value storage interface
- `wasi-logging` (default): Logging interface
- `wasi-otel` (default): OpenTelemetry interface
- `wasi-webgpu` WebGPU interface

### Architecture

wash-runtime provides three main abstractions:

1. **Engine**: Wasmtime configuration and component compilation
2. **Host**: Runtime environment with plugin management
3. **Workload**: High-level API for managing component lifecycles

## Sizing guest memory in containers

The two guest-memory modes charge memory differently:

- `enforce` (the default) charges declared initial memory
  and subsequent growth to `--max-guest-memory`.
- `growth` (opt-in) leaves up to 2 MiB of each memory's first allocation uncharged;
  any excess and subsequent growth are charged.
  A separate `--max-initial-guest-memory` bounds the sum of those uncharged bytes
  across all live stores.
  Memories that start at zero receive the allowance on their first successful allocation.
- In both modes, dropping a store releases its reservations.

Let `C` be the container memory limit, `G` the charged guest budget, and `I` the
initial allowance. The defaults are:

- `G` is 75% of `C`, clamped between 256 MiB and 1 TiB.
- **`I = 4 × G`**, so an explicit `G` also changes the default `I`.
- When the clamp does not apply, that permits `3 × C` in uncharged initial memory,
  plus `0.75 × C` in charged memory.

These are declared byte reservations, not resident-memory measurements.
The factor of four is a density policy for sparse guests;
it does not guarantee that the container can back every admitted page.

For one component, compute `e = sum(min(initial_bytes_per_memory, 2 MiB))`.
If `e > 0`, the initial allowance permits at most `floor(I / e)` such live instances.
Charged memory, pool limits, and the process guard can stop admission earlier.
For a 768 MiB container with default `G = 576 MiB` and `I = 2304 MiB`:

| Initial memory per component | Initial reservation | Charged initial memory | Budget-only instance bound |
| --- | ---: | ---: | ---: |
| One 1 MiB memory | 1 MiB | 0 | 2304 |
| Four 1 MiB memories | 4 MiB | 0 | 576 |
| One 8 MiB memory | 2 MiB | 6 MiB | 96 |

Touching only 64 KiB during a request does not constrain later writes. Several
already admitted guests can make their remaining initial pages resident at
once without calling `memory.grow` or consulting either admission limit.
Copy-on-write (CoW) can share initialized data until a guest writes it;
disabling CoW, data-heavy initialization, and transparent huge pages can move
that physical cost earlier. Include both CoW settings in capacity testing.

### Choosing an initial allowance

Plan against the whole cgroup's peak memory, including sibling processes and
file/shared pages. On Linux the process guard normally measures
`RssAnon + VmSwap` plus page tables (`VmPTE`, and `VmPMD` on 4.x kernels), so
its reading is not interchangeable with cgroup usage.
The default process threshold is 90% of detected memory. It refuses new work
and growth after a sampled crossing; it cannot stop writes to pages already
admitted or enforce a hard physical ceiling. Keep a container limit and reserve
headroom for allocations between samples.

Use this planning inequality, with every term in bytes:

```text
H + B + p × I + R <= C
I <= (C - H - B - R) / p       (p > 0)
```

- `H`: peak memory outside the guest reservations, including runtime metadata,
  host/plugin buffers, allocation rounding, other processes, and shared images
  counted once. Measure it at the planned store count and concurrency, not on
  an empty host.
- `B`: resident cost reserved for charged guest memory. Use the full `G` when
  planning for all authorized charged memory to become resident.
- `p`: a conservative upper bound on physical bytes per uncharged initial byte
  after correlated activity. Use `1` when every initial page can be written,
  with allocation rounding included in `H`. A steady-state average is insufficient.
- `R`: additional headroom for transient allocations and measurement uncertainty.

For the default `I = 4G` and `B = G`, this becomes
`G <= (C - H - R) / (1 + 4p)`. Even with no host overhead or reserve, the default
`G = 0.75C` leaves room for only `p <= 1/12` (about 8.3%) if all charged memory
also becomes resident. If all initial pages may be written, use `G + I + H + R
<= C`, lower the initial allowance, or use `enforce`. A workload-specific lower
`B` requires its own defensible bound.

For example, suppose a 768 MiB container needs `H = 96 MiB`, `B = G = 256 MiB`,
and `R = 128 MiB`. It has 288 MiB left for initial resident pages. A validated
`p <= 0.5` permits `I <= 576 MiB`; `p = 1` permits only `I <= 288 MiB`.
For the latter assumptions, the corresponding host flags are:

```bash
wash host --guest-memory-mode growth \
  --max-guest-memory 256MiB \
  --max-initial-guest-memory 288MiB \
  --max-process-memory 640MiB
```

These numbers illustrate the calculation; measure `H`, `B`, and `R` for the
actual application. The process threshold is still sampled and does not cover
every cgroup charge. Revalidate after changing components, concurrency, CoW,
or kernel memory policy. Measure cold/data-heavy initialization and synchronized
writes as well as sparse steady-state requests. Do not estimate `p` by dividing
raw process RSS by declared initial bytes: repeated CoW mappings can count the
same shared image repeatedly in RSS. Attribute private initial pages separately
from `H` and `B`, or use the conservative `p = 1` bound.

The [container pressure test](../../scripts/bench/README.md#95-guest-memory-pressure-test)
compares these shapes and records the actual limits, counters, cgroup memory,
and verified OOM outcomes. Reducing `I` reduces admitted density; it does not
reclaim memory from stores already running.

## Testing

The integration tests and benchmarks load precompiled wasm components from
`tests/wasm/`. Build them once before running them:

```bash
# from the repo root
cargo xtask build-fixtures
```

Re-run it after editing a fixture under [`tests/fixtures/`](./tests/fixtures/)
(see that directory's README for details).

The opt-in [guest-memory pressure test](../../scripts/bench/README.md#95-guest-memory-pressure-test)
runs in disposable Docker containers. It is separate from `cargo test` because
some cases deliberately exhaust their container's memory.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.

### Process memory failures and recovery

If memory capacity cannot be detected, automatic process sizing fails at startup.
Set an explicit `max-process-memory` to supply the missing capacity policy, or
explicitly disable the guard only when another mechanism bounds the process.
Zero is rejected through both the CLI and the engine builder.

Use `--strict-process-memory` (`WASH_HOST_STRICT_PROCESS_MEMORY=true`), or
`EngineBuilder::with_strict_process_memory(true)`, to require detected capacity
and reject a threshold at or above it. Strict mode also rejects disabling the
guard. Without strict mode, explicit swap-backed thresholds remain supported,
with a warning when they leave no headroom below detected capacity.

An unavailable initial footprint blocks starts and guest growth. Five consecutive
failed periodic samples also block admission until a valid reading returns.
`process_memory.readable` is 0 while accounting is unavailable and 1 when it is
readable; alert on sustained 0. The footprint gauge retains its last reading.
On Linux, missing `VmPTE` or malformed accounting fields invalidate the sample;
absent optional `VmSwap` and `VmPMD` fields still mean zero.

If the footprint stays above the threshold after workloads drain, inspect retained
allocator memory and pool page tables. Decommitting a pooled slot can retain its
page tables. After draining, recreate the engine/pool or restart the host to
release those mappings. Refusals remain in force while the footprint is above
the threshold; the approximately one-minute recovery applies only after memory
has fallen within it. Restarting or unmapping an active pool is not automatic.

The measured 9 KiB per instance and roughly 10% page-table overhead are workload
observations, not capacity bounds. Sparse reads can allocate page tables without
allocating private data pages. Include page tables in the host overhead `H` above,
and bound pool size, admitted guest memory, and concurrency for the intended
workload. Reserve enough space that the process threshold plus uncounted memory
and worst-case allocations already admitted remain below the actual capacity.
The default 90% threshold alone cannot establish that bound. Use OS/container
limits to contain total usage and workload isolation where tenant availability
must be independent.
