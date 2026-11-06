# wasi-webgpu-wasmtime 0.3.1, patched

The crates.io release of [wasi-webgpu-wasmtime](https://crates.io/crates/wasi-webgpu-wasmtime)
0.3.1 (Apache-2.0 WITH LLVM-exception, from
[wasi-gfx/wasi-gfx-runtime](https://github.com/wasi-gfx/wasi-gfx-runtime)), with
`scripts/gpubench/repro/queue-wait/wasi-webgpu-wasmtime-0.3.1.patch` applied.
The workspace `Cargo.toml` swaps it in with `[patch.crates-io]`. Its own
`Cargo.toml` adds an empty `[workspace]` and this directory has a `Cargo.lock`,
only so the license check can resolve it alone; builds use the root lock.

The patch fixes `on_submitted_work_done`, which never returns to the guest.
It is the code of upstream
[wasi-gfx/wasi-gfx-runtime#156](https://github.com/wasi-gfx/wasi-gfx-runtime/pull/156)
at 895488a, plus
[#157](https://github.com/wasi-gfx/wasi-gfx-runtime/pull/157) at d4bb0a2, which
reports the adapter strings wgpu-native does, so the guest and the
`webgpu-native` step take the same vendor-gated paths in llama.cpp. A new `DeviceHandle` owns the device and queue ids, and runs at
most one thread per device that polls only that device and runs the wgpu
callbacks. `on_submitted_work_done` and `map_async` both wait on it, and
neither traps the guest if the device is lost. `callback-future` is replaced
by `oneshot`.

`scripts/gpubench/repro/queue-wait/` has the repro. Drop this directory and the
patch entry once a release with the fix is in `Cargo.lock`.
