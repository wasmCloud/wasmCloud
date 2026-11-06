# wasmtime on macOS: what serializes, and what fixes it

Found profiling `scale_density` at 18 cores with `sample(1)`. Against
wasmtime 48.0.8.

## 1. Freed linear-memory slots are reset with `mmap` (fixed by the patch)

**Symptom.** Scale-to-zero cold requests (instantiate per request) peaked at
4 cores (35k/s) and fell to 22k/s at 18, with CPU per request rising. `__mmap`
was the top non-idle frame; 86% of it came from
`MemoryImageSlot::reset_with_anon_memory` under `deallocate_memory`.

**Cause.** On Linux, `decommit_behavior()` is `RestoreOriginalMapping`: a freed
slot is memset up to `linear_memory_keep_resident` and the rest `madvise`d,
guided by pagemap. Everywhere else it is `Zero`: every freed slot is remapped
with `mmap(MAP_FIXED)` across its whole size, and `keep_resident` is ignored.
On XNU that takes the process's `vm_map` lock exclusively, so every teardown
on every thread queues behind it.

**Fix** ([wasmtime-macos-inplace-reset.patch](wasmtime-macos-inplace-reset.patch),
`src/runtime/vm/cow.rs`). When the slot has no image (always true on macOS
for components compiled from bytes, since there is no memfd) and
`keep_resident > 0`:

1. `mincore` the accessible range (a shared lock, not an exclusive one).
2. If the touched pages fit in `keep_resident`, `memset` just those to zero
   and keep the slot mapped: that is exactly the clean-slot invariant
   (`0..accessible` R+W and zero).
3. Otherwise fall back to `reset_with_anon_memory`, unchanged.

A page counts as touched if any `mincore` bit other than
`MINCORE_ANONYMOUS` (0x80) is set. macOS sets 0x80 on every anonymous page,
touched or not; the first version missed this and never took the fast path.
In-core, modified and paged-out pages are all zeroed, so no data survives into
the next instance. Needs a security review before upstreaming: it changes how
memory is cleared between instances.

**Result.** 18 cores: 22k -> 103k cold req/s; p99 24 -> 5 ms. Cost: up to
256 KiB per freed slot stays resident (scale-to-zero density -3%).

## 2. Tables are decommitted with `mmap` (a setting)

`DecommitQueue::flush` remaps table pages beyond `table_keep_resident`.
Tables reset only up to their used size, so `table_keep_resident` at or above
that size (64 KiB here) memsets them and skips the `mmap`. No patch needed:
`WASMTIME_POOLING_TABLE_KEEP_RESIDENT=65536`. 103k -> 107k cold req/s.

## 3. Page-granular pool costs (not fixable by settings)

Each warm instance dirties at least one host page per pooled resource: 3 of
linear memory, 2 tables, 1 fiber stack. On 16 KiB pages that is 96 KiB,
against ~24 KiB on 4 KiB pages. Ideas for upstream:

- Pack small tables (this fixture's fit in a few hundred bytes) into shared
  pages instead of a 160 KiB slot each.
- Smaller fiber stacks don't help: one page is the floor, and parked P3
  instances hold their stack.

## Settings that did nothing here

- `WASMTIME_MEMORY_INIT_COW=0`, and compiling via a mapped `.cwasm` so images
  are CoW: this fixture's data segments are too small to matter.
- `WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT`: stack zeroing is off, so stacks
  are never decommitted.
- Capping `WASMTIME_POOLING_TOTAL_STACKS`: breaks warm starts, since each
  parked P3 instance holds a stack.

## Applying the patch locally

```sh
cp -R ~/.cargo/registry/src/index.crates.io-*/wasmtime-48.0.8 ../wasmtime-48.0.8-patched
git -C ../wasmtime-48.0.8-patched apply "$PWD/scripts/bench/macos/wasmtime-macos-inplace-reset.patch"
```

Then keep the `[patch.crates-io]` entry in `Cargo.toml` (the EXPERIMENT
commit) and set `WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT`.
