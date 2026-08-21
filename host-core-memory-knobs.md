# `jfleitz/host-core-memory-knobs`

Three numbers that decide how much memory a wasmCloud host may hand to guests.
All three already existed inside wasmtime; none was reachable from the outside.

**This branch changes no behaviour when the flags are unset.** Both pool knobs
fall through to exactly the values wasmtime has always used, and the budget is
used only to check the other two and report them. It is the vocabulary that a
memory-aware host needs, landed on its own so that the enforcement built on top
of it (`jfleitz/host-core-memory-features`) can be reviewed separately.

Base: `7cd553cc5`.

---

## The knobs

| Flag | Env | Bounds | Unset |
|---|---|---|---|
| `--max-memory` | `WASH_MAX_MEMORY` | Total guest memory this host may use | Derived: ¾ of the cgroup limit that would OOM-kill this process, clamped 256 MiB..1 TiB |
| `--default-heap-memory` | `WASH_DEFAULT_HEAP_MEMORY` | How large any single linear memory may grow (`max_memory_size`) | wasmtime's own default, **4 GiB** |
| `--core-instances` | `WASH_CORE_INSTANCES` | Instance slots the pooling allocator keeps | wasmtime's own default, **1000** |

### Why the budget is worth naming before anything enforces it

`default_heap_memory × core_instances` is what the pooling allocator reserves —
every slot is sized for the largest memory it might hold, whether or not
anything grows into it.

Today a stock host claims **4 GiB × 1000 = 3.9 TiB** of address space and
nobody is told. That sits close enough to the 4 TiB
`is_pooling_allocator_supported` probes for that nudging either knob crosses it,
and the failure — instantiation refusing, much later, with neither number in the
message — is unpleasant to diagnose. Naming the budget lets the host say at
startup whether the pool it is about to build is one the machine could back.

---

## What changed

### `crates/wash-runtime/src/engine/host_memory.rs` — new module

* `HostMemory` — the three resolved numbers, plus `pool_reservation()` (their
  product) and `advisory()` (what the host should say about the combination).
* `HostMemory::resolve()` — falls back to the derived budget and to wasmtime's
  own defaults; rejects a zero for any of the three, since each means "this host
  runs nothing" and two of them would misbehave inside wasmtime rather than
  saying so.
* `derive_max_memory()` — cgroup v2 → cgroup v1 → system total, ¾ of it,
  clamped. Mirrors `host::quota::default_max_connections`, which takes the same
  shape of a share of `RLIMIT_NOFILE`: an unset flag means the derived number,
  never "unbounded".
* `parse_bytes` / `render_bytes` — Kubernetes quantity semantics, see below.

### `crates/wash-runtime/src/engine/mod.rs`

* `Engine.host_memory` and `EngineBuilder::with_host_memory`, plus an
  `Engine::host_memory()` accessor that reads back what was *installed* — so an
  embedder's explicit `max_instances` winning over the flag-driven count is
  visible rather than assumed.
* `new_pooling_config` takes the heap ceiling and **gives `max_memory_size` an
  `else` branch**. It was the only knob in that function without one, which is
  why wasmtime's 4 GiB default stood on every host and an instance count never
  implied a byte count. The fallback is that same default unless the flag is
  set, so this alone changes nothing.
* An `advisory()` warning is logged at build time when the combination is
  unreservable or when a single guest could exhaust the host.

### `crates/wash-runtime/src/host/mod.rs`

`mod sysinfo` → `pub(crate) mod sysinfo`, so the budget derivation can read the
machine's total memory when there is no cgroup.

### `crates/wash/src/cli/host.rs`, `crates/wash/src/config.rs`

The three flags, and `config::host_memory()` which parses, validates and logs
them. Resolution happens **first in `handle()`**, before anything is connected
or built: a bad size is a typo in a flag, and reporting it after a NATS dial has
already failed buries the actionable error under an unrelated one.

One startup line carries all of it:

```
INFO host memory resolved max_memory=6GiB default_heap_memory=4GiB
     core_instances=1000 pool_reservation=3.9TiB
     max_memory_source="derived from the cgroup limit"
```

### `charts/runtime-operator`

`runtime.memory.{maxMemory,defaultHeapMemory,coreInstances}`, overridable per
host group.

**`maxMemory` defaults to the host group's own `resources.limits.memory`** —
the number the kernel would OOM-kill the pod at, and therefore the only honest
input to "how much may guests use". With neither set the flag is omitted and the
host derives the budget from its cgroup, which reaches the same number by a
different route.

---

## A bug this surfaced

The chart passes `resources.limits.memory` through verbatim, which is a
**Kubernetes quantity** — and `2Gi` initially failed to parse.

Fixing it properly meant adopting Kubernetes' semantics rather than "binary
everywhere":

| Suffix | Meaning |
|---|---|
| none, `B` | bytes |
| `Ki`, `KiB`, `Mi`, `MiB`, `Gi`, `GiB`, `Ti`, `TiB` | binary (1024ⁿ) |
| `K`, `KB`, `M`, `MB`, `G`, `GB`, `T`, `TB` | decimal (1000ⁿ) |

`2Gi` and `2G` are genuinely different numbers. Reading `2G` as 2 GiB would have
the host believe it has 7% more than the kernel will give it, and **over-reading
the budget is the dangerous direction** — the correction arrives as an OOMKill.
A test pins that the ambiguous-looking form resolves to the smaller number.

---

## Tests

`cargo test -p wash-runtime --lib` — **517 passed**, 7 of them new:

| Test | Property |
|---|---|
| `unset_knobs_resolve_to_wasmtimes_own_defaults` | the safety property of the whole branch: setting nothing leaves the host as it was |
| `a_zero_is_refused_for_each_knob` | zero means "run nothing" in all three cases |
| `sizes_parse_in_binary_units` | Kubernetes quantities, including `2G` < `2Gi` |
| `the_pool_reservation_is_the_product_of_the_two_pool_knobs` | the arithmetic |
| `the_stock_defaults_sit_just_under_the_probe` | pins 3.9 TiB, and that the stock configuration must **not** warn |
| `an_unreservable_combination_is_reported_with_both_numbers` | the advisory names both knobs, so it is actionable |
| `a_guest_ceiling_above_the_whole_budget_is_called_out` | a single guest able to exhaust the host is worth saying |

Verified against the built binary:

```
$ wash host --core-instances 0
--core-instances must be at least 1

$ wash host --default-heap-memory 12PiB
invalid --default-heap-memory: unknown size suffix "pib"; expected a
Kubernetes-style quantity (Ki/Mi/Gi/Ti for binary, K/M/G/T for decimal, or plain bytes)
```

Chart:

```
$ helm template t . --set 'runtime.hostGroups[0].resources.limits.memory=2Gi' | grep max-memory
- "--max-memory=2Gi"
```

---

## Review notes

1. **`max_memory_size` gaining a default is the one line with a blast radius.**
   It resolves to wasmtime's own 4 GiB unless the flag is set, so nothing moves
   — but it is the line that makes the ceiling settable at all, and worth a
   second pair of eyes.
2. **`--max-memory` gates nothing yet.** It is reported and used to check the
   other two. The admission control that spends it is in branch 2. If a
   flag-that-does-not-yet-enforce is unwelcome, it could be held back — at the
   cost of the startup advisory, which needs it.
3. **`advisory()` warns rather than refuses.** Reserving address space is not
   consuming memory, so an over-committed pool is a warning; only the
   past-the-probe case is a genuine failure, and even that fails later at
   instantiation rather than at startup. Refusing outright would be defensible.
