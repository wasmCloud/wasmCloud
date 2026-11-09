# scale_density: how the limits are derived

`scale_density` ramps HTTP workloads onto one host until something refuses
them, and prints, beside what it served, the count each constraint predicts.
How to run it and every knob are in the module docs of `scale_density.rs`.
The report is a `bench_tools::density::Report` per flavour, shared with
`bench-scale`. In the text report, `?` means there is not enough measurement
evidence to estimate a limit, and `-` means a reading was not taken (a
spin-down, a CPU share). Neither means the capacity is infinite: an idle
workload uses no CPU, so CPU bounds rates, not counts. `WASH_SCALE_REPORT_JSON`
writes the reports, including `guest_memory_mode`, with unknowns as `null`; in
CI (`bench.yml`, bench `scale_density`) it is
`<target>/density/scale_density.json`, which `bench-tools
jsonl` publishes to the trend site as `served_workloads`, `binding_limit`,
`marginal_bytes_per_workload` and the other figures `Report::metrics` names.

If a baseline or step process-footprint reading is unavailable, the runtime
benchmark stops with a measurement error instead of recording 0 B or making a
capacity claim from it.

The default enforce-mode series keeps its existing name. Count and growth
runs append `-count` or `-growth` to the label, so their results publish as
separate series.
`WASH_SCALE_GUEST_MEMORY_MODE` takes a list, and `scripts/bench/run-bench.sh`
runs `enforce,growth`, so CI publishes both series.
`WASH_SCALE_MEMORY_RESERVATION` takes a list too, and `run-bench.sh` runs
`default,1GiB,256MiB,64MiB`:

- `default` runs every flavour and mode at wasmtime's layout.
- Each reduced reservation runs both flavours in the first mode, as
  `warm-layout<reservation>+<guard>` and
  `scale-to-zero-layout<reservation>+<guard>`. An explicit flavour filter
  still selects only that flavour.
- The `max_scale` workflow variant selects only `256MiB` with a `64KiB`
  guard and `growth` mode, producing `warm-growth-layout256MiB+64KiB`
  and `scale-to-zero-growth-layout256MiB+64KiB`.
- A reduced reservation moves an isolation boundary; see `THREAT_MODEL.md`.

- Count mode measures guest bytes,
  but does not treat the guest budget as a binding limit.
- Growth mode charges only growth beyond each memory's first allocation,
  so a retained charge of zero does not measure its capacity.
  What binds it instead is the cap on uncharged initial memory
  (`WASH_SCALE_INITIAL_GUEST_MEMORY`), which it reports as
  max-initial-guest-memory.

Warm runs need the P3 fixture. `WASH_SCALE_FIXTURE=p2` runs scale-to-zero
only, because a P2 HTTP instance cannot stay warm.

## What a workload costs

Measured with the `http-handler-p3` fixture on Linux x86_64. The cost scales
with the component, so these are a guide, not constants.

| | scale-to-zero (`pool_size: 0`) | warm (`pool_size: 1`) |
| --- | --- | --- |
| host footprint, marginal | ~11 KiB | ~75-85 KiB, ~9 KiB of it page tables |
| guest memory charged | 0 | 1.06 MiB (17 pages, the declared initial memory) |
| pool core instances | 0 | 4 (core modules in the component) |
| pool linear memories | 0 | 1 |
| address space reserved | 0 | one 4 GiB + 32 MiB slot per memory (virtual, not in the footprint) |
| CPU per start | ~0.25 ms | ~0.5 ms, including its first request |

- **Scale-to-zero** holds only host bookkeeping: the workload record, its
  component metadata and route, and the `InstancePre` the HTTP handler keeps.
  The WASI and `wasi:http` linker that used to dominate this (~180 KiB per
  workload) is now built once per engine and shared.
  - A component that a plugin or a link writes to gets a copy of its own
    (~140 KiB) while the workload resolves.
  - Once it resolves, the component keeps its `InstancePre` and lets the copy
    go; later invocations and service starts reuse the kept link.
  - A component that does not link keeps only the error, which every attempt
    to instantiate it reports.
  - The bench's fixture imports nothing a plugin writes to, so it holds no
    copy, and this does not show in its figures.
- **Warm** adds a parked instance: its stores, its core instances, the
  pages of linear memory the guest has touched, and the page tables that map
  its slot. The page-table share was measured at a 64 MiB + 64 KiB layout on
  arm64 (73.4 KiB a warm instance before the footprint counted page tables,
  82.2 KiB after); it grows with how much of the slot is touched. Page tables
  stay counted after the instance is dropped, until the slot's pool goes.
  The guest budget charges the declared size whether or not it is resident,
  which is why the guest charge is many times the footprint growth.

## The limits

Each one comes from the last *clean* step: one where nothing was refused and
every new workload served. Past that point, refusals distort what a workload
seems to cost. `n` is the number of workloads at that step, and `F` is the
footprint there.

| Limit | Formula | Binds for |
| --- | --- | --- |
| max-process-memory | `n + (limit - F) / marginal`, where `marginal = (F - F_first) / (n - n_first)` over the clean steps | both |
| max-guest-memory | `max_guest_memory / (guest charged / n)` | warm in enforce or growth mode (? when no retained charge is measurable); omitted in count mode |
| max-initial-guest-memory | `initial cap / (initial memory uncharged / n)` | warm in growth mode only |
| pool core instances | `core instance total / (core instances in use / n)` | warm |
| pool memories | `memory total / (memories in use / n)` | warm |
| cpu | unbounded idle capacity: an idle workload uses no CPU | neither |

- **Why marginal, not average:** the first workload pays once for everyone,
  for the compile and the shared linkers. An average over a few thousand
  workloads counts that, and overstates each one.
- **The binding limit** is the lowest of these. The run should stop within
  one step of it, and the stop line says which limit stopped it.
- **CPU** caps rates, not counts:
  - `cpu max starts/s = cores / (CPU spent starting / workloads started)`
  - `cpu max req/s = cores / (CPU spent serving the sample / successful requests)`

  Both are summed over the clean steps. On Linux the load generator's own
  CPU is left out.
- **Pool linear memories** default to what the runtime derives
  (`derive_max_memories`): half the usable address space at one 4 GiB +
  32 MiB reservation each plus its stack and tables, no more than nine tenths
  of `vm.max_map_count` holds at six mappings per warm component (two for its
  pooled stack, four for its memory, or two with
  `WASMTIME_MEMORY_INIT_COW=false`), and no more than 1% of the memory limit
  at 512 bytes a memory. That is 9,829 on a default Linux, 14,744 with images
  off, and 16,246 on macOS. A warm workload holds one
  linear memory, one fiber stack and one component instance, from totals of
  the same size; its four core instances and two tables draw on totals sized
  as multiples of that count, so they do not bind first. Windows, 32-bit
  targets, and Linux with strict overcommit accounting or a finite
  `RLIMIT_DATA` keep the host's old default of 1,000 memories, since the
  writable table and (outside Windows) stack mappings can otherwise prevent
  engine startup
  (`wash host --set-max-memories-from-memory-precommit-limit` sizes such a host against a
  quarter of its commit limit instead; the bench keeps the floor).
  (The bench's own earlier derivation, a fifth of the mapping limit, gave
  13,106 on a default Linux; it is gone, so runs there are not comparable
  across the change.)

## The memory guard

max-process-memory is enforced by sampling the footprint every second, and
again on demand: at most every 50 ms, whenever a workload starts or a guest
grows. Once the footprint is over the limit, guest growth, new instances and
new workloads are refused. This is sampled admission control, not a hard
allocation cap. Work already admitted, large individual allocations, guest
writes into committed memory, and host/plugin buffers can cross the threshold
between readings. Queued starts recheck after acquiring their permit and before
pulling or compiling. Reserve headroom; use OS/container limits for a hard bound.


If the footprint passes 125% of the limit without the guard tripping, the
run stops and reports that the guard did not hold.

## Presets

`cargo xtask scale-density <preset>` runs this bench with one of three presets,
building `http-handler-p3` first if it is missing, then the bench, whose cold
build can take ten minutes or more. Each run writes its JSON
report and a log to `<target>/scale-density/`, or to `--out` (relative to the
current directory), so it refuses a `WASH_SCALE_REPORT_JSON` you set. A log records the commit, whether the tree had uncommitted
or untracked changes, the fixture's SHA-256, and the host's OS version, CPU
and core count. Before each run, its previous JSON report is removed so a
failure cannot leave old results beside the new log. A failed flavour does
not skip the next one, but the task still fails. Every preset runs the P3
fixture and refuses another `WASH_SCALE_FIXTURE`.

| Preset | Configuration |
| --- | --- |
| `stock` | The bench's defaults; adds nothing |
| `density` | Warm and scale-to-zero ramps, 2 GiB process limit, 30 minutes per flavour |
| `cores` | Both flavours at 1, 2 and 4 cores (pinned on Linux; host runtime workers elsewhere), 2 GiB process limit; at most 50k warm or 100k scale-to-zero workloads |

The density and core presets use 64 concurrent starts, 10k starts per step and
20k sampled requests, keep up to 64 KiB of freed table memory per slot
resident, and cap a derived pool count at 300k memories
(`WASH_SCALE_MAX_MEMORIES_CAP`). Warm runs use growth mode, a 2 MiB
reservation and no guard region; scale-to-zero runs keep the bench's layout and
mode. A variable you set wins over the preset's value:

```sh
WASH_SCALE_MEMORY_LIMIT=32GiB WASH_SCALE_CPUS=1,2,4,6,12,18 cargo xtask scale-density cores

# Compare table retention; its own --out keeps the baseline.
WASMTIME_POOLING_TABLE_KEEP_RESIDENT=0 cargo xtask scale-density cores --out target/scale-density/no-table-keep

# Give larger guests more heap and keep a guard region.
WASH_SCALE_MEMORY_RESERVATION=64MiB WASH_SCALE_MEMORY_GUARD=2MiB \
  cargo xtask scale-density density --out target/scale-density/64mib-guarded
```

Reading the preset runs:

- A 2 MiB pooled reservation is also a 2 MiB heap cap. It suits this fixture;
  larger components can fail to instantiate or grow.
- Removing the guard region makes isolation depend on generated bounds checks
  (`THREAT_MODEL.md`). The runtime's default layout is unchanged.
- Growth mode leaves some initial memory uncharged; see the initial-memory
  limit above, and use `WASH_SCALE_INITIAL_GUEST_MEMORY` to compare revisions
  with a fixed cap.
- The runtime bounds a derived count by its bookkeeping, about 512 bytes a
  memory within 1% of the memory limit (a cgroup's, else physical memory):
  1.3 million at 2 MiB slots at 64 GiB. The cap limits it to at most 300k;
  machines with tighter memory, address-space or mapping limits still derive
  fewer. Retained table pages still consume real memory.
- Off Linux, the core sweep changes host runtime workers, not CPU affinity. The
  load generator shares the machine and its CPU is in the process total, so
  throughput is not host-only capacity.
- The presets come from an [M5 Max experiment](https://github.com/wasmCloud/wasmCloud/blob/30168eb1c3389d63a8b6dd8f6a158a127354f114/scripts/bench/macos/README.md)
  that also used an experimental Wasmtime linear-memory reset. Its tuned
  cold-request results depend on that patch; these presets apply no patch and
  set no linear-memory keep-resident. Its macOS and Linux warm memory also
  differ by host page size, which is no evidence for changing production heap
  limits.
