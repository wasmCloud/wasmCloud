# bench-scale

How many HTTP components one host can hold
with the CPU and memory the machine gives it.

The bench starts HTTP workloads on a host in steps,
up to and a little past the limit its own measurements predict,
and records how the host fails when it gets there.
It talks to the host only over wasmCloud's NATS host API and HTTP,
so it works against any host binary that speaks that API:
a released `wash`, a local build, or another implementation.
It does not link against wash-runtime.

```text
cargo xtask bench-scale --flavour warm --report
```

## What it measures

Two flavours, each against a fresh host:

- **warm** (`poolSize: 1`):
  every newly started workload answers one HTTP request before the next step,
  which also parks its instance.
  Each workload then holds a pooled instance and a live linear memory,
  so the pool's slots and the memory limit compete to stop it.
- **scale-to-zero** (`poolSize: 0`):
  a workload holds no instance while idle.
  Sampled requests instantiate one on demand,
  so their latency is the cold-start latency,
  and the bench reports how long the footprint takes to settle afterwards ("spin-down").
  The ceiling is the host's per-workload bookkeeping against the memory limit.

Each step:

1. starts `--step` workloads, at most `--start-concurrency` in flight;
2. (warm) sends one request to each new workload;
3. sends `--sample` requests spread evenly across every running workload,
   oldest to newest;
4. (scale-to-zero) waits for the footprint to settle back;
5. asks the host for a heartbeat and reads its process footprint.

It records per step:
running workloads, starts/s, start p50/p99,
start refusals, timeouts and no-responders,
host footprint, bytes per workload over the baseline,
the heartbeat's workload and component counts,
request successes, failures, p50/p99,
and the theoretical limit so far.

The **theoretical limit** is `(memory limit − baseline footprint) / bytes per workload`,
and for warm also `core instances / core instances per workload` when that is known
(from `--core-instances-per-workload`,
or inferred from a "maximum concurrent limit of N for core instances" refusal).

A flavour stops at the first of:

| Stop | Meaning |
| --- | --- |
| `start-refused` | a start came back `WORKLOAD_STATE_ERROR`; classified as pool exhausted, memory limit, or other |
| `memory-limit` | the host's footprint reached `--memory-limit` (checked every 100 ms, so a step halts mid-way) |
| `requests` | more than half the warm-up or sampled requests failed |
| `host-exited` | the host process ended; its exit status and log tail are kept |
| `starts-unanswered` | a start got no answer even after retries |
| `starts-failed` | most of a step's starts failed for another reason |
| `cap` | `--max-components`, or the theoretical limit × `--overshoot`, was reached |

"Served" is the largest workload count with a nonempty, error-free sample
and no failed warm-up requests. Zero samples are rejected.

## Provisioning

Everything is cached under `--cache-dir` (default `target/bench-scale/` in the workspace),
so a second run downloads and builds nothing.

- **Host**: `--host-bin <path>`, or by default the released `wash` for `--wash-version`
  (default: this workspace's version),
  downloaded from `https://github.com/wasmCloud/wasmCloud/releases/download/v<version>/wash-<target>`
  the way `install.sh` does.
  The bench never builds wash.
  `cargo binstall wash` does not work for this:
  wash 2.x is not on crates.io, and its binstall metadata points at stale tags.
- **NATS**: `--nats-url`, or a `nats-server` (`--nats-version`, default 2.12.8)
  downloaded from its GitHub release and run on a free loopback port.
- **Component**: `--image <oci ref> --flavour scale-to-zero` for an image the host can already pull,
  `--wasm <path>` for a P3 HTTP component to inspect and serve,
  or by default the `http-handler-p3` test fixture
  (`crates/wash-runtime/tests/fixtures/http-handler-p3`, exporting `wasi:http/handler@0.3.0`).
  The fixture is built without wash:
  the WIT packages its `wkg.toml` names are copied into its `wit/deps`,
  then `cargo build --target wasm32-wasip1 --release` builds the core module,
  which is wrapped with the WASI reactor adapter. Install the `wasm32-wasip1` Rust target first.
  Warm runs require a verified P3 HTTP export; an external OCI reference cannot
  establish that, so use `--wasm` for a custom warm experiment.
  A `--wasm` component without a `wasi:http/handler@0.3.0` export (a P2 handler, say)
  is rejected for either flavour.
  The component is served from a read-only OCI registry inside the bench,
  tagged by its content hash: `127.0.0.1:<port>/bench/http-handler:<hash>`.
  The registry answers exactly what the host's OCI client (oci-client 0.17) asks in a pull:
  `GET /v2/`, `GET`/`HEAD /v2/<name>/manifests/<tag|digest>` (an OCI image manifest
  with a `application/vnd.wasm.config.v0+json` config and one `application/wasm` layer,
  with `Docker-Content-Digest`), and `GET /v2/<name>/blobs/<digest>`.

With a wash host, the bench runs
`wash host --scheduler-nats-url <url> --data-nats-url <url> --http-addr 127.0.0.1:0
--allow-insecure-registries --max-process-memory <limit> --max-guest-memory <¾ limit>
--core-instances <n> --oci-cache-dir <run dir> --host-name <unique>` plus every `--host-arg`.
It reads `wash host --help` first and leaves out flags the binary does not have
(wash 2.10.3 has no `--max-process-memory`, for example);
the report notes them, and the bench's own footprint check still enforces the limit.
The host gets a cleared environment except `PATH`, `HOME`, `TMPDIR`, `USER`, `LANG`
and the `SSL_CERT_*` variables (on Windows also `SystemRoot`, `SystemDrive`, `windir`,
`TEMP`, `TMP`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `PATHEXT` and `ComSpec`),
with `RUST_LOG` from `--host-log` (default `warn`).
Its stdout and stderr go to `host-<flavour>.log` in the run directory.

All child processes are killed when the bench exits, errors, panics or is interrupted.

## Other hosts

```text
# A local build of wash
cargo build -p wash --release
cargo xtask bench-scale --host-bin target/release/wash --report

# A host with another command line: only --host-arg, with placeholders
cargo xtask bench-scale --host-bin ./my-host --host-cli raw \
  --host-arg serve --host-arg --nats={nats_url} --host-arg --memory={memory_limit} --report

# A host that is already running on an existing NATS
cargo xtask bench-scale --host-nats-only --nats-url nats://127.0.0.1:4222 \
  --host-pid 12345 --report

# Any image the host can pull
cargo xtask bench-scale --image registry.example.com/team/http-handler:1.0.0 --flavour scale-to-zero --report
```

Raw placeholders are `{nats_url}`, `{memory_limit}` and `{guest_memory}` (bytes),
`{core_instances}`, `{http_addr}` and `{host_name}`.
On shared NATS, a spawned host must advertise the generated `{host_name}` in its
heartbeat; wash gets it through `--host-name`, and raw hosts must receive it in a
`--host-arg`. `--host-id` selects an existing host only with `--host-nats-only`.
The host must publish heartbeats on `runtime.operator.heartbeat.<id>`,
answer `runtime.host.<id>.{heartbeat,workload.start,workload.status,workload.stop}`,
and report its HTTP port in the heartbeat (or pass `--host-http`).

## Flags

Every flag also reads a `BENCH_SCALE_*` environment variable
(`--memory-limit` from `BENCH_SCALE_MEMORY_LIMIT`, and so on),
except the repeatable `--host-arg`.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--flavour` | `both` | `warm`, `scale-to-zero` or `both` (each on a fresh host) |
| `--memory-limit` | half of the smaller of physical RAM and the cgroup limit | host memory budget and the footprint the bench stops at (`4GiB`, `512MiB`, `1G`) |
| `--max-components` | theoretical × overshoot | stop after this many workloads |
| `--overshoot` | `1.1` | how far past the theoretical limit the default cap goes |
| `--step` | `250` | workloads started between measurements |
| `--start-concurrency` | `32` | starts in flight (the host answers at most 64 pending starts) |
| `--start-timeout` | `60` | seconds to wait for a start reply before asking its status |
| `--start-retries` | `3` | retries per start after no reply or no responders |
| `--sample` | `50` | requests per step, spread across all workloads |
| `--request-concurrency` | `64` | HTTP requests in flight |
| `--request-timeout` | `30` | seconds before a request counts as failed |
| `--spin-down-wait` | `3000` | scale-to-zero: ms to wait for the footprint to settle |
| `--core-instances` | derived | pool slots passed to the host; default min(usable address space / 2 / (4 GiB + 32 MiB), `vm.max_map_count` / 5) |
| `--core-instances-per-workload` | inferred | for the pool's share of the warm theoretical limit |
| `--min-components` | `1` | exit 2 if a flavour served fewer |
| `--report` | off (`BENCH_SCALE_REPORT=1`) | print the human report; otherwise the run is silent |
| `--json <path>` | none | write full results as JSON, report or not |
| `--run-dir` | `<cache>/runs/<unix time>` | host and NATS logs, OCI caches |
| `--cache-dir` | `target/bench-scale` | downloads |
| `--host-bin` | released wash | host binary |
| `--host-cli` | `wash` | `wash` (flags probed from `host --help`) or `raw` |
| `--host-arg` | none | extra host argument, repeatable |
| `--host-log` | `warn` | `RUST_LOG` for the host |
| `--wash-version` | workspace version | released wash to download |
| `--host-nats-only` (`--attach`) | off | spawn no host; use one on `--nats-url` |
| `--host-id` | first heartbeat | pick a host when several share NATS |
| `--host-pid` | none | attached host's pid, for memory sampling |
| `--host-http` | `127.0.0.1:<httpPort>` | where to send HTTP requests |
| `--heartbeat-timeout` | `60` | seconds to wait for the host's first heartbeat |
| `--nats-url` | own server | existing NATS server |
| `--nats-version` | `2.12.8` | nats-server release to download |
| `--image` | built fixture | OCI reference of the component |
| `--wasm` | built fixture | component file to serve instead |

## Output

Without `--report` the bench prints nothing and only its exit code speaks:
0 on success, 2 when a flavour served fewer than `--min-components`, 1 on error.

With `--report` it prints a header
(machine cores, RAM and OS, limits, image, NATS, run directory),
then per flavour the host binary, version (from its heartbeat), arguments
and how memory is read, one table row per step, and a summary;
then both flavours side by side.
Table columns:

| Column | Meaning |
| --- | --- |
| `workloads` | running workloads after the step |
| `starts/s` | starts completed this step per second of starting |
| `start p50/p99` | start round trip, ms |
| `ref/to/nr` | refused starts / reply timeouts / no-responders |
| `footprint` | host process footprint |
| `per-wkld` | (footprint − baseline) / workloads |
| `hb wkld/cmp` | the heartbeat's workload and component counts |
| `warm ok/f` | warm-up requests to new workloads |
| `req ok/f`, `p50 ms`, `p99 ms` | the step's sampled requests |
| `down ms` | scale-to-zero: footprint settle time after the sample |
| `limit` | theoretical limit so far |

`-` marks a value that was not measured, or a limit with too little evidence to estimate.
It never means the capacity is unbounded.

`--json` writes everything (settings, machine, image, every step, stop reasons,
registry request counts, warm-up failures beside request failures)
whether or not `--report` is set,
so runs against different host binaries can be compared.
Unmeasured values are `null` there.

## Caveats

- CPU cannot be limited from inside the process.
  The host uses whatever the machine gives it; the report states the core count.
  Use `taskset`, a cgroup or a container around the whole bench to bound it.
- The memory limit is enforced by design, not by the kernel:
  the footprint is checked every 100 ms and after every step,
  and a step stops starting workloads once the limit is reached,
  but in-flight work, large allocations, and guest writes can still overshoot.
  Leave headroom and use OS/container limits where a hard bound is needed.
- Footprint is `VmRSS + VmSwap` from `/proc/<pid>/status` on Linux,
  `phys_footprint` on macOS (including compressed pages), and sysinfo's resident
  size elsewhere (which misses compressed and swapped pages).
  The report says which.
- Attached to a running host (`--host-nats-only`), memory is unknown without `--host-pid`,
  and the run then needs `--max-components` to bound it.
  Every submitted workload ID is journaled, including unanswered starts. Normal
  completion, errors, Ctrl-C, and Unix SIGTERM all run a cleanup phase with a 60-second budget.
  Stops are retried and removal is checked. Unconfirmed IDs are printed as an
  error and the next flavour is not started. An unanswered start that remains
  absent is still reported as uncertain because it could be queued at the host.
- The released wash is what runs by default.
  To bench a local build, pass `--host-bin target/release/wash`.
- Scale-to-zero spin-down is read from the footprint,
  so an allocator that keeps freed memory shows no spin-down (`-`).

Latency percentiles describe successful responses only. Request rates count successful responses; failures are reported separately. Capacity requires a positive sample with no request or warm-up failures.
