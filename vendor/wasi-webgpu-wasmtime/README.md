# wasi-webgpu-wasmtime 0.3.1, patched

The crates.io release of [wasi-webgpu-wasmtime](https://crates.io/crates/wasi-webgpu-wasmtime)
0.3.1 (Apache-2.0 WITH LLVM-exception, from
[wasi-gfx/wasi-gfx-runtime](https://github.com/wasi-gfx/wasi-gfx-runtime)), with
the fixes below. The workspace `Cargo.toml` swaps it in with `[patch.crates-io]`.
Its own `Cargo.toml` adds an empty `[workspace]` and this directory has a
`Cargo.lock`, only so the license check can resolve it alone; builds use the
root lock.

- `on_submitted_work_done` never returned to the guest.
  `scripts/gpubench/repro/queue-wait/` has the repro and the patch for this one.
- `map_async` blocked an executor thread on every device's GPU work, with no
  timeout, and panicked on a map error.
- `GpuDevice.lost` and `GpuDeviceLostInfo` were `todo!()`, and `GpuQueue.submit`
  panicked on a lost device.
- Dropping a queue with work still in flight blocked for about 6s and then
  panicked inside wgpu.

Each device now has one poller thread, started while a call waits on the
device or a released queue waits for it to go idle; it polls only that
device. Work that outlives `WasiWebGpuOptions::wait_timeout` loses the
device, which resolves `GpuDevice.lost`, fails pending maps and makes the
device ignore new submissions. The shader keeps the GPU busy until it ends or
the driver resets it. A busy queue is dropped once its device is idle; if a
lost device is still busy 60s later, its queue and the wgpu instance are
leaked rather than dropped, since wgpu panics on dropping a busy queue.

Drop this directory and the patch entry once a release with these fixes is in
`Cargo.lock`.
