# wasi-webgpu-wasmtime 0.3.1, patched

The crates.io release of [wasi-webgpu-wasmtime](https://crates.io/crates/wasi-webgpu-wasmtime)
0.3.1 (Apache-2.0 WITH LLVM-exception, from
[wasi-gfx/wasi-gfx-runtime](https://github.com/wasi-gfx/wasi-gfx-runtime)), with
the fixes below. The workspace `Cargo.toml` swaps it in with `[patch.crates-io]`.
Its own `Cargo.toml` adds an empty `[workspace]` and this directory has a
`Cargo.lock`, only so the license check can resolve it alone; builds use the
root lock.

The fixes are the code of upstream
[wasi-gfx/wasi-gfx-runtime#156](https://github.com/wasi-gfx/wasi-gfx-runtime/pull/156)
at 895488a, plus the commits stacked on it in
[wasi-gfx/wasi-gfx-runtime#158](https://github.com/wasi-gfx/wasi-gfx-runtime/pull/158). The only other change is that `WasiWebGpuCtx::table` is a
`wasmtime_wasi::ResourceTable`, as in 0.3.1.

- #156: `on_submitted_work_done` never returned to the guest, and `map_async`
  blocked an executor thread on every device's GPU work. A new `DeviceHandle`
  owns the device and queue ids, and runs at most one thread per device that
  polls only that device and runs the wgpu callbacks.
  `scripts/gpubench/repro/queue-wait/` has the repro.
- Device loss: `GpuDevice.lost` and `GpuDeviceLostInfo` were `todo!()`, and a
  lost device panicked `GpuQueue.submit` and the error conversion. A loss now
  resolves `lost` and settles every waiting call, and `destroy()` aborts
  pending maps. A map on a buffer that is mapped or has a map pending
  rejects without touching that map, and unmapping or destroying a buffer
  before its map completes rejects that map with `AbortError`.
  `destroy()` unmaps the device's buffers. `unmap`, `write-buffer` and
  `write-texture` no longer trap on a lost device: unmap ignores wgpu's
  errors, and write errors go to the device's error scopes.
- Mapped ranges: reading or writing the range of a buffer that is not
  mapped, an out-of-bounds or unaligned range, or a write whose length
  differs from the range returns `OperationError`. So does a
  `write-buffer` data offset or size outside the data, or a size that isn't
  a multiple of 4. These used to panic or trap.
- Timeout: submitted work that outlives `WasiWebGpuOptions::wait_timeout`
  loses the device, whether or not a call waits on it. Each submission
  settles through a work-done callback, and an idle poller thread waits
  100ms for more work before it exits. The shader keeps the GPU busy until it ends or the driver resets
  it.
- Busy queue drop: dropping a queue with work in flight blocked for about 6s
  and then panicked inside wgpu. A busy queue is now dropped from a thread
  once its device is idle, or at once if the driver lost the device. If a
  lost device is still busy 60s later, its queue and the wgpu instance are
  leaked rather than dropped.

Drop this directory and the patch entry once a release with these fixes is in
`Cargo.lock`.
