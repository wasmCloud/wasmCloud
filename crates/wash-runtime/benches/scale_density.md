# scale_density: how the limits are derived

`scale_density` ramps HTTP workloads onto one host until something refuses
them, and prints, beside what it served, the count each constraint predicts.
How to run it and every knob are in the module docs of `scale_density.rs`.
The report is a `bench_tools::density::Report` per flavour, shared with
`bench-scale`. In the text report, `?` means there is not enough measurement
evidence to estimate a limit, and `-` means a reading was not taken (a
spin-down, a CPU share). Neither means the capacity is infinite: an idle
workload uses no CPU, so CPU bounds rates, not counts. `WASH_SCALE_REPORT_JSON`
writes the reports, with unknowns as `null`; in CI (`bench.yml`, bench
`scale_density`) it is `<target>/density/scale_density.json`, which `bench-tools
jsonl` publishes to the trend site as `served_workloads`, `binding_limit`,
`marginal_bytes_per_workload` and the other figures `Report::metrics` names.

If a baseline or step process-footprint reading is unavailable, the runtime
benchmark stops with a measurement error instead of recording 0 B or making a
capacity claim from it.

Warm runs need the P3 fixture. `WASH_SCALE_FIXTURE=p2` runs scale-to-zero
only, because a P2 HTTP instance cannot stay warm.

## What a workload costs

Measured with the `http-handler-p3` fixture on Linux x86_64. The cost scales
with the component, so these are a guide, not constants.

| | scale-to-zero (`pool_size: 0`) | warm (`pool_size: 1`) |
| --- | --- | --- |
| host footprint, marginal | ~11 KiB | ~65-75 KiB |
| guest memory charged | 0 | 1.06 MiB (17 pages, the declared initial memory) |
| pool core instances | 0 | 4 (core modules in the component) |
| pool linear memories | 0 | 1 |
| address space reserved | 0 | one 4 GiB + 32 MiB slot per memory (virtual, not in the footprint) |
| CPU per start | ~0.25 ms | ~0.5 ms, including its first request |

- **Scale-to-zero** holds only host bookkeeping: the workload record, its
  component metadata and route, and the `InstancePre` the HTTP handler keeps.
  The WASI and `wasi:http` linker that used to dominate this (~180 KiB per
  workload) is now built once per engine and shared. A component that a
  plugin or a link writes to still gets a copy of its own (~140 KiB), so a
  workload importing, say, `wasi:logging` costs about that much more.
- **Warm** adds a parked instance: its stores, its core instances, and the
  pages of linear memory the guest has touched. The guest budget charges the
  declared size whether or not it is resident, which is why the guest charge
  is many times the footprint growth.

## The limits

Each one comes from the last *clean* step: one where nothing was refused and
every new workload served. Past that point, refusals distort what a workload
seems to cost. `n` is the number of workloads at that step, and `F` is the
footprint there.

| Limit | Formula | Binds for |
| --- | --- | --- |
| max-process-memory | `n + (limit - F) / marginal`, where `marginal = (F - F_first) / (n - n_first)` over the clean steps | both |
| max-guest-memory | `max_guest_memory / (guest charged / n)` | warm (? when no retained charge is measurable) |
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
- **Pool totals** default to half the usable address space at one 4 GiB +
  32 MiB slot each, capped at `vm.max_map_count / 5`. That is 13,106 slots
  on a default Linux. A warm workload holds four core instances, so the pool
  fits about 3,276 of them.

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
