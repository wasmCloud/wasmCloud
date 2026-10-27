# bench-scale

How many HTTP components one running wasmCloud host can hold
with the CPU and memory the machine gives it.

The bench attaches to a host you started, starts HTTP workloads on it in steps,
up to and a little past the limit its own measurements predict,
and records how the host fails when it gets there.
It talks to the host only over wasmCloud's NATS host API and HTTP,
so it works against any host binary that speaks that API:
a released `wash`, a local build, or another implementation.
It links `wash-runtime` only for the generated API types
and the host guard's footprint accounting.

For the same ramp with the host in-process
(guest memory, pool usage and CPU per step),
see `cargo bench -p wash-runtime --bench scale_density`.
Both write the same `bench_tools::density::Report`.

## Running it

Bring up NATS, a host, and a registry holding a P3 HTTP component, then point the bench at them:

```text
cargo xtask build-fixtures http-handler-p3
# push crates/wash-runtime/tests/wasm/http_handler_p3.wasm to a registry the host can pull from
wash host --scheduler-nats-url nats://127.0.0.1:4222 --data-nats-url nats://127.0.0.1:4222 \
  --http-addr 127.0.0.1:8080 --allow-insecure-registries &

cargo xtask bench-scale --report \
  --nats-url nats://127.0.0.1:4222 --host-id <id from the host's heartbeat> \
  --host-http 127.0.0.1:8080 --host-pid $! \
  --image localhost:5000/http-handler-p3:latest
```

`--nats-url`, `--host-id`, `--host-http` and `--image` are required.
Every flag also reads a `BENCH_SCALE_*` variable (`--image` from `BENCH_SCALE_IMAGE`, and so on);
`cargo xtask bench-scale --help` lists them.
Without `--host-pid` the bench cannot see memory, so it then needs `--max-components`.

## What it measures

Two flavours, one after the other on the same host
(each removes its workloads before the next starts):

- **warm** (`poolSize: 1`):
  every newly started workload answers one HTTP request before the next step,
  which also parks its instance.
  The image must export `wasi:http/handler@0.3.0`;
  a P2 instance cannot stay warm, and its failed warm-ups stop the run.
- **scale-to-zero** (`poolSize: 0`):
  a workload holds no instance while idle.
  Sampled requests instantiate one on demand,
  and the bench reports how long the footprint takes to settle afterwards ("spin-down").

Each step:

1. starts `--step` workloads, at most `--start-concurrency` in flight,
   retrying unanswered starts and asking their status;
2. (warm) sends one request to each new workload;
3. sends `--sample` requests spread evenly across every running workload, oldest to newest;
4. (scale-to-zero) waits for the footprint to settle back;
5. reads the host's footprint.

A step is *clean* when every start succeeded, every warm-up answered,
and the sample served with no failures.
"Served" is the workload count at the last clean step.
The memory limit estimate comes from clean steps only
(`bench_tools::density::memory_estimate`: the marginal cost between the first and last clean steps),
and without `--max-components` the run stops at that estimate × `--overshoot`.

A flavour stops at the first of:
the footprint reaching `--memory-limit` (checked every 100 ms, so a step halts mid-way),
a start the host refuses (classified as pool, memory, or other),
starts unanswered after retries,
a failed warm-up or more than half the sample failing,
most of a step's starts failing,
or the cap.

## Output

Without `--report` the bench is silent and only its exit code speaks:
0 on success, 2 when a flavour served fewer than `--min-components`,
1 on error, including cleanup that could not confirm removal
(the results are still written).

`--report` prints each step and the flavours side by side on stderr,
with the same renderer as `scale_density`.
`--json <path>` writes a list of `bench_tools::density::Report`;
written to `<target>/density/bench-scale.json`,
`bench-tools jsonl --bench bench-scale` turns it into trend-site rows.
`-` (or `null`) is a reading not taken and `?` a limit with too little evidence;
neither means unbounded.

Host messages and response bodies may echo credentials,
so they go to stderr only; the report says why a run stopped without them.

## Caveats

- The footprint is the host guard's own reading (`RssAnon + VmSwap` on Linux,
  `phys_footprint` on macOS), so the bench's limit and the host's agree.
  It is a single process: a wrapper's children are not counted.
- The limit is sampled, not enforced:
  in-flight starts and large allocations can overshoot it.
  Leave headroom, and use OS or container limits where a hard bound is needed.
- CPU is whatever the host's process is given; bound it with `taskset`,
  a cgroup or a container around the host.
- Every submitted workload ID is journaled, including unanswered starts.
  Completion, errors, Ctrl-C and SIGTERM all run a cleanup with a 60-second budget;
  unconfirmed IDs are printed and exit 1.
- Spin-down is read from the footprint,
  so an allocator that keeps freed memory shows none (`-`).
