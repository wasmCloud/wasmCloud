# wasmtime on macOS: patch notes

Notes on what serializes wasmtime's pooling allocator on macOS under
wash-runtime's `scale_density` bench, the patch that fixes the worst of it,
and what is left. Against **wasmtime 48.0.8**, measured on an Apple M5 Max
(18 cores: 6 Super + 12 Performance, 48 GB, macOS 26.7, 16 KiB pages).

| | |
|---|---|
| [`patches/0001-…-in-place…patch`](patches/) | the fix, in `src/runtime/vm/cow.rs` |
| [`patches/0002-…-test…patch`](patches/) | four macOS unit tests for it |
| [`apply.sh`](apply.sh) | vendors wasmtime from the cargo registry, `git am`s the series, adds `[patch.crates-io]` |

Status: experiment. wasmtime's lib tests pass on macOS with it (175 passed, 1
ignored, as upstream). It changes how memory is cleared between instances, so
it needs a security review before going upstream.

## 1. Freed memory slots are reset with `mmap` (patched)

### Symptom

Scale-to-zero instantiates a fresh instance for every request and frees it
afterwards. Cold-request throughput stopped scaling at 4 cores and then fell:

| Cores | 4 | 12 | 18 |
|---|---|---|---|
| Cold req/s, stock | 35.6k | 24.1k | 23.3k |
| p99, stock | 14.5 ms | 22.6 ms | 24.0 ms |

CPU per start rose with the core count too (0.03 -> 0.06 ms). Rising CPU and
falling throughput mean waiting on something shared, not running out of
cores.

### Evidence

`sample(1)` of the bench process at 18 cores, during cold requests, sorted by
top of stack. Idle workers parked in `__psynch_cvwait` aside, the top frame was
`__mmap` with 12,949 samples:

| Samples | Caller |
|---|---|
| 11,147 | `MemoryImageSlot::reset_with_anon_memory` <- `clear_and_remain_ready` <- `PoolingInstanceAllocator::deallocate_memory` <- `Store` drop |
| 1,802 | `DecommitQueue::flush` <- `PoolingInstanceAllocator::merge_or_flush` (tables, section 2) |

So 86% of it comes from tearing down instances.

### Cause

When a pooled linear memory is freed, `MemoryImageSlot::clear_and_remain_ready`
resets it according to `vm::decommit_behavior()`:

- **Linux, `RestoreOriginalMapping`:** memset the first `keep_resident` bytes,
  `madvise(MADV_DONTNEED)` the rest back to the original mapping (the memfd
  image, or zero). PAGEMAP_SCAN, when available, limits both to dirty pages.
  The slot stays mapped.
- **Everywhere else, `Zero`:** `reset_with_anon_memory`, one
  `mmap(MAP_FIXED | MAP_ANON)` over the whole slot (`static_size`, here the
  2 MiB reservation), leaving it inaccessible. `keep_resident` is ignored. The
  next instantiate then `mprotect`s it accessible again.

On XNU, `mmap(MAP_FIXED)` over an existing range replaces map entries, which
needs the task's `vm_map` lock held exclusively. Every thread tearing down an
instance takes that lock, and so does every page fault and every
`mprotect` while it is held. The more threads there are, the longer each
waits: hence throughput falling as cores are added. That the lock is the
contended resource is an inference from the profile and the scaling curve. We
did not trace it in the kernel.

### Fix

`patches/0001`: in the `Zero` branch of `reset_all_memory_contents`, on macOS
only, before falling back:

```rust
#[cfg(target_os = "macos")]
if self.image.is_none()
    && keep_resident.byte_count() > 0
    && let Some(resident) = unsafe { self.zero_touched_pages(keep_resident) }
{
    return Ok(resident);
}
self.reset_with_anon_memory()?; // unchanged fallback
```

`zero_touched_pages`:

1. `mincore(base, accessible, vec)` over the slot's accessible range. This reads
   page state under a shared lock and modifies no mappings.
2. Counts pages whose state has any bit other than `MINCORE_ANONYMOUS`.
3. If `touched * page_size <= keep_resident`, `memset`s each touched page to
   zero, leaves the mapping as it is, and returns the bytes now resident.
4. Otherwise, or if `mincore` fails, returns `None` having changed nothing,
   and the caller remaps as before.

It is opt-in: wasmtime's default `linear_memory_keep_resident` is 0, which
keeps the old path. wash-runtime passes `WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT`
through to `PoolingAllocationConfig::linear_memory_keep_resident`; we used
256 KiB.

### Why this is correct

The invariant `clear_and_remain_ready` must restore, from `MemoryImageSlot`'s
docs: when not dirty, bytes `0..accessible` are R+W and zero (or the image's
contents), and `accessible..static_size` is inaccessible.

- **No image.** With no image, "clean" means all zero, so zeroing every page
  that could hold data is enough. Slots with an image are skipped. On macOS a
  component compiled from bytes never has one (no memfd); one loaded with
  `Component::deserialize_file` can, and still takes the old path.
- **`0..accessible` stays R+W.** Nothing is remapped or reprotected, and the
  bytes are zero after the memset. `accessible` is unchanged, so the next
  `instantiate` sees an equal (absent) image and only adjusts protections for
  the new initial size, the same as the Linux path.
- **`accessible..static_size` stays inaccessible.** Untouched: guests cannot
  grow past `accessible` without `set_heap_limit`, which `mprotect`s.
- **Which pages are zeroed.** Every page that might hold data. XNU's
  `mincore` bits are `INCORE 0x1`, `REFERENCED 0x2`, `MODIFIED 0x4`,
  `REFERENCED_OTHER 0x8`, `MODIFIED_OTHER 0x10`, `PAGED_OUT 0x20`, `COPIED 0x40`
  and `ANONYMOUS 0x80`. Any bit except `ANONYMOUS` counts as touched, so pages
  that are compressed or paged out are zeroed too (the memset faults them back
  first). A page with no bits but `ANONYMOUS` was never instantiated by the
  VM, and reads as zero.
- **No race.** `clear_and_remain_ready` runs on a slot no instance owns, and
  `deallocate_memory` holds it exclusively until it goes back to the pool.

### The `MINCORE_ANONYMOUS` bug

The first version counted any nonzero state as touched, and changed nothing:
throughput was 30.7k / 22.9k / 20.5k at 4 / 12 / 18 cores, within noise of
stock. `mincore` was being called (3,066 samples) and every call fell back. A
C probe showed why:

```
page=16384  3 pages written, 1 read -> nonzero=68 of 68
untouched page state = 0x80, written = 0x87, read-only = 0x83
```

XNU sets `MINCORE_ANONYMOUS` on every page of an anonymous mapping, touched
or not, so every page counted and every slot exceeded `keep_resident`. Masking
with `& 0x7f` fixed it. The test
`untouched_anonymous_pages_do_not_count_as_touched` covers it.

### Result

Cold requests, scale-to-zero, 100k workloads, 10k starts and 20k requests per
step, 64 starts in flight:

| Cores | 4 | 12 | 18 |
|---|---|---|---|
| Stock | 35.6k | 24.1k | 23.3k |
| Patch, mask bug | 30.7k | 22.9k | 20.5k |
| **Patch** | **49.3k** | **95.2k** | **102.8k** |
| p99, patch | 9.3 ms | 5.1 ms | 5.0 ms |

18 cores: **4.4x the throughput, a fifth of the p99.** Throughput now keeps
rising with cores.

### Cost

Up to `keep_resident` bytes stay resident per freed slot instead of going back
to the kernel; with this fixture that is 3 pages (48 KiB). At a 32 GiB limit,
scale-to-zero density went from 2.50M to 2.43M workloads (13.4 -> 13.7 KiB
each, -3%). Warm density did not change: warm slots are not freed.

The bytes reported resident feed the pool's warm-slot accounting
(`unused_bytes_resident`, `max_unused_warm_slots`), as they already do on
Linux.

### Tests (`patches/0002`)

`cow.rs`'s existing slot tests are Linux-only, so the patch adds a
`#[cfg(target_os = "macos")]` module:

| Test | Checks |
|---|---|
| `touched_pages_are_zeroed_in_place` | written and read-only-touched pages are zeroed, the slot keeps its accessible size (it was not remapped), and it instantiates again clean |
| `more_than_keep_resident_falls_back_to_remapping` | 8 touched pages against a 4-page budget: remapped, accessible 0 |
| `zero_keep_resident_keeps_the_old_behaviour` | the default (0) remaps, as before the patch |
| `untouched_anonymous_pages_do_not_count_as_touched` | one written page fits a one-page budget despite `MINCORE_ANONYMOUS` |

They pass a `decommit` callback that panics, so they also show the fast path
never decommits.

### Before going upstream

- **Security review** of the zeroing argument above. This is the line between
  one tenant's data and the next one's.
- **Cost of `mincore` on large memories.** It needs one byte per page of
  `accessible`, allocated per teardown: 68 bytes here, 256 KiB for a 4 GiB
  memory. Use a stack buffer for small slots, and skip the fast path when
  `accessible` is far beyond what `keep_resident` could cover.
- **Other `Zero` platforms.** FreeBSD has `mincore` with different bits. Gate
  per OS, or keep it macOS-only.
- **Slots with images.** The fast path could also restore image-backed slots
  by copying only touched pages from the image, as Linux's
  `manually_reset_region` does.
- **Alternatives to check.** `madvise` on XNU does not promise zero-fill for
  private anonymous memory (`MADV_FREE` and `MADV_FREE_REUSABLE` leave the
  contents undefined), so it cannot replace the memset. Whether
  `mach_vm_behavior_set(..., VM_BEHAVIOR_ZERO)` zeroes a range without the
  exclusive lock is unverified.

## 2. Table slots are decommitted with `mmap` (a setting)

The other 1,802 `__mmap` samples: `DecommitQueue::flush` remapping table pages
past `table_keep_resident`. `TablePool::reset_table_pages_to_zero` only resets
up to the table's used size (`size() * element_size`), so a
`table_keep_resident` at least that large memsets the table and decommits
nothing. This fixture's tables need a few hundred bytes; 64 KiB is plenty.

`WASMTIME_POOLING_TABLE_KEEP_RESIDENT=65536`: cold requests 102.8k -> 106.9k/s
at 18 cores. No patch needed.

Stacks do not need this: wash leaves `async_stack_zeroing` off, so
`zero_stack` returns before decommitting.

## 3. Page-granular costs (not fixable by settings)

A warm instance dirties at least one host page per pooled resource. Measured
by diffing `vmmap -interleaved` at 20k and 50k warm workloads, grouping
regions by size and protection:

| Region | Per workload | What |
|---|---|---|
| 1088 KiB R+W (in a 2 MiB slot) | 48 KiB | linear memory: 17 wasm pages declared, 3 host pages touched |
| 128 MiB chunks after the memory pool | 32 KiB | table pool: 2 tables x 160 KiB slots (20,000 elements x 8 B), one page touched each |
| 2048 KiB R+W | 16 KiB | async fiber stack: a parked P3 instance holds one |

That's 96 KiB of the 144 KiB a warm workload costs, all at one-page minimums.
On 4 KiB pages it would be ~24 KiB, which is why warm costs about 2x on the
Mac (Linux CI: ~75 KiB). Ideas for upstream:

- **Pack small tables:** let tables smaller than a page share pages, or size
  table slots from the module rather than `table_elements`.
- **Stacks:** one page is the floor. Capping `total_stacks` below the warm
  count breaks warm starts.

## 4. Tried, no effect

| Setting | Why not |
|---|---|
| `WASMTIME_MEMORY_INIT_COW=0` | macOS has no CoW images from bytes anyway |
| Compiling via a mapped `.cwasm` (`Component::deserialize_file`) for CoW images | this fixture's data segments are too small to matter (warm 159 KiB either way) |
| `WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT` without the patch | ignored on `Zero` platforms |
| `WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT` | stack zeroing is off |
| `WASMTIME_POOLING_TOTAL_STACKS=2048` | warm starts fail: parked instances hold stacks |

## 5. Contention that isn't wasmtime

Found in the same profiles, for completeness:

- **`Uuid::now_v7`'s global mutex** (three per store): fixed on this branch with
  a per-thread context (`crates/wash-runtime/src/ids.rs`).
- **The bench's single-task load generator:** fixed on this branch
  (`benches/scale_density/load.rs`).
- **The tokio timer lock:** `Sleep::poll` spinning in `parking_lot::RawMutex::lock_slow`,
  from hyper's header-read timeout and wash's call timeout on every request.
  Open.

## Reproduce

```sh
scripts/bench/macos/wasmtime/apply.sh             # vendors + patches + [patch.crates-io]
CORES=4,12,18 scripts/bench/macos/run.sh cores     # patched (run.sh sets keep-resident)
WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT=0 \
  CORES=4,12,18 scripts/bench/macos/run.sh cores   # same binary, old path
```

To rebase onto a newer wasmtime: vendor it, `git am` the patches, fix the
conflicts in `cow.rs`, rerun the four tests (`cargo test --lib
macos_inplace_reset` in the crate) and `git format-patch` the result back
into `patches/`.
