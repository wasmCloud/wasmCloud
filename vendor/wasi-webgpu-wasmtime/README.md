# wasi-webgpu-wasmtime 0.3.1, patched

The crates.io release of [wasi-webgpu-wasmtime](https://crates.io/crates/wasi-webgpu-wasmtime)
0.3.1 (Apache-2.0 WITH LLVM-exception, from
[wasi-gfx/wasi-gfx-runtime](https://github.com/wasi-gfx/wasi-gfx-runtime)), with
`scripts/gpubench/repro/queue-wait/wasi-webgpu-wasmtime-0.3.1.patch` applied.
The workspace `Cargo.toml` swaps it in with `[patch.crates-io]`. Its own
`Cargo.toml` adds an empty `[workspace]` and this directory has a `Cargo.lock`,
only so the license check can resolve it alone; builds use the root lock.

The patch fixes `on_submitted_work_done`, which never returns to the guest.
It now waits on a blocking thread, polling only the queue's device for the
submission the callback was registered on, and a poll error traps the guest.
`GpuQueue` keeps a reference to its device for that poll.

`scripts/gpubench/repro/queue-wait/` has the repro. Drop this directory and the
patch entry once a release with the fix is in `Cargo.lock`.
