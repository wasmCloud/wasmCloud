# Repro: `OnSubmittedWorkDone` never completes on `wasi:webgpu`

A wasip3 guest that calls `wgpuQueueOnSubmittedWorkDone` and waits on it with
`wgpuInstanceWaitAny` blocks forever. `wgpuBufferMapAsync` with the same wait
works. This is what stops llama.cpp's WebGPU backend at its first queue wait.

- Guest: [wasi-webgpu-headers](https://github.com/wasi-gfx/wasi-webgpu-headers)
  at 917765e and at 5163b36 (latest), wasi-sdk 34, `wasm32-wasip3`.
- Host: `wasi-webgpu-wasmtime` 0.3.1 on wasmtime 48.0.8 and wgpu-core 29.0.4.
  0.4.0 and 0.5.0 have the same code. Apple M1 Max, Metal.
- `runner/` is a bare wasmtime host: wasi p3 plus `wasi:webgpu`, nothing else.

## Run it

```bash
H=path/to/wasi-webgpu-headers W=path/to/wasi-sdk-34
$W/bin/clang --target=wasm32-wasip3 --sysroot=$W/share/wasi-sysroot -O1 -I$H \
  main.c $H/webgpu.c $H/async_futures.c $H/imports.c $H/imports_component_type.o -o repro.wasm
cargo run --release --manifest-path runner/Cargo.toml -- --dir .::/ repro.wasm submit
```

`main.c` takes a mode: `empty`, `write`, `submit`, `both` (each followed by
`OnSubmittedWorkDone` + `WaitAny`) or `map` (`MapAsync` + `WaitAny`).

| mode | 0.3.1 | patched |
|---|---|---|
| empty, write, submit, both | hangs | ok |
| map | ok | ok |

Polling every device from another thread doesn't help: the work-done callback
fires, but the call still never returns to the guest.

## Cause

Two problems in `on_submitted_work_done` (`src/trait_impls.rs`):

1. **Lost wake-up.** It awaits a `callback_future::CallbackFuture`, which
   keeps the waker from its first poll only. If wasmtime later polls it with
   a different waker, the callback wakes the stale one and the future is
   never polled again. `map_async` gets away with it because its callback
   runs inside that first poll.
2. **Nothing polls the device.** wgpu-core runs work-done callbacks from
   `poll_all_devices`. `map_async` calls it after registering; this does not.
   So the callback only fires if something else happens to poll.

Instrumented, an unpatched host enters `on_submitted_work_done`, registers
the closure, sees the closure fire (when something polls), and never reaches
the code after the `.await`.

## Fix

`wasi-webgpu-wasmtime-0.3.1.patch` (against the crates.io source) is the
code of upstream wasi-gfx/wasi-gfx-runtime#156. Each device gets a
`DeviceHandle` that owns its device and queue ids and runs at most one poll
thread. The thread polls only that device, waiting for the highest
submission a caller needs, and the wgpu callbacks resolve `oneshot`
channels directly. That fixes both problems: the channel wakes whichever
task polled last, and the device is always polled. `on_submitted_work_done`
submits queued writes first, and `map_async` uses the same poll instead of
`poll_all_devices` on the executor. A lost device resolves waiting calls
instead of trapping the guest. The guest's device, queues and buffers, and
a running poll, each hold the handle, so ids stay valid if the guest drops
the device mid-wait.

With an earlier version of the patch, llama-bench (Qwen2.5 0.5B Q4_K_M, all layers on the GPU, one CPU
thread) runs to completion inside wash-runtime: pp64 54.5–55.0 tok/s and tg16 33.8 tok/s, against
55.1 and 39.1 for the same llama.cpp on native wgpu (wgpu-native on
wgpu-core 29.0.4).
