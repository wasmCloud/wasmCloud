# scale_density: how the limits are derived

`scale_density` ramps HTTP workloads onto one host until something refuses
them, and prints, beside what it served, the count each constraint predicts.
Its messaging flavours ramp in-flight messages the same way; see
[Messaging](#messaging).
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
- **Pool linear memories** default to what the runtime derives
  (`derive_max_memories`): half the usable address space at one 4 GiB +
  32 MiB reservation each, and no more than nine tenths of `vm.max_map_count`
  holds at six mappings per warm component (two for its pooled stack, four for
  its memory). That is 9,829 on a default Linux. A warm workload holds one
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

## Messaging

The `messaging` and `messaging-pooled` flavours measure how many deliveries
one host holds in flight, not how many workloads. Each starts
`WASH_SCALE_MSG_COMPONENTS` copies of `messaging-sleeper-p3` on the in-memory
backend and publishes a burst `WASH_SCALE_MSG_STEP` messages larger each step,
round-robin across them. Every delivery sleeps `WASH_SCALE_MSG_HOLD_MS`, and
admission waits half of that, so a burst past the ceiling sheds before any
slot it waits for comes free.

| | `messaging` (`pool_size: 0`) | `messaging-pooled` (`pool_size: 4`) |
| --- | --- | --- |
| where a delivery runs | a store of its own, built in the receive loop | a warm instance, or a store of its own from the pooled path once all four are busy |
| when it takes its pool cost | at admission | at admission, or when its store is built, where the runtime defers it |

The host is built the way `wash host` builds one with no messaging flags: the
stock pool, and the ceilings `MessagingLimits::resolve` derives from it.
`WASH_SCALE_CORE_INSTANCES` does not apply. Deliveries are counted from the
messaging plugin's own tracing: a delivery is in flight while the span it
opens on admission is open, and the plugin's log line says whether it
completed, failed, or was shed.

Each step records:

| Column | Meaning |
| --- | --- |
| offered | messages published in the burst |
| in flight | the most deliveries admitted and not yet finished at once |
| completed, shed, failed | how the burst ended; a message not published, or neither completed nor shed by the end of the step, counts as failed |
| footprint, guest mem, memories, core inst | the highest readings while the burst was in flight |
| refused | instantiations the pool refused, plus anything the memory guard or guest budget refused |

The run stops at the first burst that is not clean, and names why:

- **pool refused**: the pool refused an instantiation, with the count shed at
  admission in the same burst. A pool refusal here means admission let in more
  than the pool could build.
- **admission shed**: admission shed messages and the pool refused nothing.
  The in-flight count is the ceiling that held.
- the memory guard, the guest budget, a publish or a delivery failing,
  `WASH_SCALE_MSG_MAX` or the time budget.

Its limits are message counts: the host ceiling, the per-component ceiling
times the components, and the pool's linear memories, since every delivery
holds at least one. The published figures are `served_in_flight` (in flight
at the last clean step), `peak_in_flight` (at any step),
`baseline_footprint_bytes` and `served_footprint_bytes`.

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
