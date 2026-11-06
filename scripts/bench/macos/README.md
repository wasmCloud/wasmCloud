# scale_density on macOS

How many small HTTP workloads one wash host holds, and how fast it serves
them, on an Apple M5 Max (18 cores: 6 Super + 12 Performance, 48 GB,
macOS 26.7). Fixture `http_handler_p3`, bench
`crates/wash-runtime/benches/scale_density.rs`.

## Results

**Density** (max-process-memory 32 GiB, every run stopped by real memory):

| | Warm | Scale-to-zero |
|---|---|---|
| CI host (Linux, 5 cores, 2 GiB limit) | 16,250 | 176,500 |
| This Mac, CI settings (2 GiB) | 13,250 | 148,000 |
| This Mac, 32 GiB, default layout | 15,000 (address-space pool cap) | 2,500,000 |
| This Mac, 32 GiB, dense layout | **232,500** | **2,430,000** |
| Memory per workload | 144 KiB | 13.7 KiB |

**Throughput at 18 cores** (stock wasmtime vs this branch with the patched wasmtime):

| | Stock | Tuned | |
|---|---|---|---|
| Scale-to-zero cold requests (instantiate each) | 22k/s, p99 25 ms | **114k/s, p99 4.8 ms** | 5.2x |
| Warm requests | 111k/s, p99 6.3 ms | **125k/s, p99 3.2 ms** | 1.1x |
| Scale-to-zero starts | 172k/s | 177-196k/s | |
| Warm starts | 62k/s | 66k/s | |

Stock, cold requests peaked at 4 cores and got slower with more. Tuned, they
keep scaling to 18, and the process keeps 14-16 of the 18 cores busy (load
generator included).

## Run it

Needs rustup (the toolchain in `rust-toolchain.toml` installs itself) and
`cargo install wasm-component-ld --version 0.5.26` (CI's pin; rustup's bundled
linker is too old for some fixtures).

```sh
scripts/bench/macos/run.sh density   # warm + scale-to-zero to the memory limit
scripts/bench/macos/run.sh cores     # core sweep, CORES=1,2,4,6,12,18
scripts/bench/macos/run.sh stock     # CI's settings
scripts/bench/run-bench.sh scale_density   # CI's full matrix, now runs on macOS
```

`MEMORY_LIMIT` (default `32GiB`) must sit well under physical RAM. The
throughput numbers need the patched wasmtime: `scripts/bench/macos/wasmtime/apply.sh`
vendors it, applies the patches and wires up `[patch.crates-io]`. Patch notes:
[wasmtime/README.md](wasmtime/README.md).

## What this branch changes

| Change | Where | Effect |
|---|---|---|
| `run-bench.sh` runs on macOS: linker from PATH, workspace target dir, `sysctl` for CPU info, bash 3.2 empty arrays | `scripts/bench/run-bench.sh` | CI's matrix runs locally |
| Growth mode's derived initial cap is the larger of 4x max-guest-memory and 16x max-process-memory | `engine/guest_memory.rs`, `engine/mod.rs` | warm 185k -> 210k with no override |
| The load generator runs one task per worker, not one task for every request | `benches/scale_density/load.rs` | warm 112k -> 124k req/s |
| Store and workload ids come from a per-thread v7 context, not `Uuid::now_v7`'s global mutex | `src/ids.rs` | removes the top mutex wait on the start path |
| EXPERIMENT: `[patch.crates-io]` to a local wasmtime with an in-place slot reset ([patches](wasmtime/patches/), [notes](wasmtime/README.md)) | `Cargo.toml` | cold requests 22k -> 114k/s; drop before merging |

## Where warm memory goes

144 KiB per warm workload, measured by diffing `vmmap` at 50k and 100k:

- **96 KiB of wasmtime pool pages, each at the one-page minimum:** 3 pages of
  guest linear memory, 2 tables, 1 fiber stack (a parked P3 instance holds its
  stack). macOS arm64 pages are 16 KiB, Linux x86's 4 KiB, which is why warm
  costs about 2x on the Mac (Linux: ~75 KiB).
- **~38 KiB of host heap** per instance's runtime state.
- **~10 KiB** remaining, most likely page tables.

No runtime knob moves these. Copy-on-write images (compile via a mapped file),
keep-resident settings and capping stacks were tried and changed nothing, or
broke warm starts.

## Dense-layout knobs

| Knob | Value | Why |
|---|---|---|
| `WASH_SCALE_MEMORY_RESERVATION` | `2MiB` | the default 4 GiB per memory caps the pool at 16,256 by address space; small slots also share page-table pages |
| `WASH_SCALE_MEMORY_GUARD` | `0` | guard regions cost address space only |
| `WASH_SCALE_GUEST_MEMORY_MODE` | `growth` | charges memory as it grows, not as declared (~1 MiB declared, ~48 KiB touched) |
| `WASH_SCALE_MAX_MEMORIES` | `300000` | the derived pool fills address space: at 4 MiB that is 16.7M slots and a 7.5 GiB baseline; at 2 MiB the engine fails to build |
| `WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT` | `262144` | enables the patched in-place reset |
| `WASMTIME_POOLING_TABLE_KEEP_RESIDENT` | `65536` | tables reset by memset, no decommit `mmap` |

## Trade-offs

- **A 2 MiB reservation is a 2 MiB heap cap.** Pooled memories cannot move; at
  the cap `memory.grow` fails. Fine for this fixture, not for real agents
  (Pi needs 4.9 MiB to instantiate). 64 MiB + 2 MiB guard gives 205,000 warm
  at 32 GiB, 159 KiB each.
- **No guard means isolation rests on bounds checks** (the host warns at
  startup), and every access pays for them.
- **Growth mode overcommits:** admitted instances may write up to the initial
  cap. The process guard refuses new instances and growth past its limit.
- **keep-resident holds up to 256 KiB per freed slot resident**, which costs
  about 3% of scale-to-zero density (2.50M -> 2.43M).

## Next

1. Upstream the wasmtime reset ([wasmtime/README.md](wasmtime/README.md)),
   after a security review of the zeroing.
2. Size the derived pool by `max-process-memory` too, so small reservations
   work without `MAX_MEMORIES`.
3. Tokio timer contention: every request arms hyper's header-read timeout and
   wash's call timeout on a shared timer lock.
4. Starts stop scaling at ~12 cores (CPU per warm start 0.17 -> 0.21 ms):
   profile the start path again.
5. Run the load generator from a second machine. It shares these 18 cores;
   the bench estimates ~145k warm and ~122k cold req/s for the host alone.
