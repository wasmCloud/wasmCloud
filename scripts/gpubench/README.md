# gpubench

GPU benchmarks for wasmCloud's `wasi:webgpu` host, for two bench boxes: a Mac
(Apple GPU, Metal) and a Linux box with an NVIDIA GPU (CUDA).

## 1. How far from native

The same model on the same GPU, three ways:

| step | what runs | WebGPU implementation |
|---|---|---|
| `vendor` | llama.cpp's Metal or CUDA backend | none |
| `webgpu-native` | llama.cpp's WebGPU backend, a native binary | wgpu-native on the host's wgpu-core |
| `wasm` | the same backend built for `wasm32-wasip3`, run by `bench-gpu` inside wash-runtime | the host's `wasi:webgpu` (`wasi-webgpu-wasmtime` on wgpu-core) |

`webgpu-native` and `wasm` run one implementation of WebGPU: wgpu-native is
rebuilt on exactly the wgpu crates `Cargo.lock` resolves for wash-runtime,
and the build refuses to finish if they differ. Both are built with Dawn's
native-only features off, and both run on one CPU thread, so the step from
`webgpu-native` to `wasm` measures what the component model and the host
interface cost, and the step from `vendor` to `webgpu-native` what WebGPU
costs.

Measured with `llama-bench` (pp512 and tg128, 5 reps, all layers on the GPU)
on Qwen2.5-7B-Instruct Q4_K_M, a 4.7 GB file: more than a wasm32 guest can
address, so the run shows the weights don't pass through guest memory. The
`llama-memory` rows record the model size and the guest's memory high-water
mark side by side.

ONNX Runtime (MobileNetV2, SqueezeNet) is not wired up yet: its WebGPU EP
builds for wasip3 on a fork, and that build is the next piece of work.

## 2. Serverless

Three models (7B, 1.5B and 0.5B) take turns on one GPU. Each turn starts the
model's workload cold, generates one token, and stops it, then the GPU sits
idle for 20 s. `sample-gpu.sh` records GPU memory throughout.

Reported: cold start to first token (weights included), GPU memory while busy
and while idle (over the box's idle baseline), and how many models shared the
GPU. The `peak_gpu_mem` row carries the memory-over-time timeline for the
chart.

## Running

```bash
scripts/gpubench/run.sh --smoke /tmp/gpubench   # 0.5B model, short tests
scripts/gpubench/run.sh /tmp/gpubench           # the real run
cargo run -p bench-tools -- gpu report /tmp/gpubench
cargo run -p bench-tools -- gpu jsonl /tmp/gpubench
```

Everything downloaded or built goes under `~/.cache/wasmcloud-gpubench`
(`GPUBENCH_CACHE`). Every download is pinned by commit or checksum in
`pins.env` and `models.json`. On a cuda box, set `GPUBENCH_CUDA_ARCH` (for
example `89` for an L4).

## Status

| piece | macOS / Metal | Linux / CUDA |
|---|---|---|
| step 1 `vendor` build and run | done on an M1 Max | **untested** |
| step 2 `webgpu-native` build and run | done on an M1 Max | **untested** |
| step 3 `wasm` build | done | done (same artifact) |
| step 3 `wasm` run | done on an M1 Max | **untested** |
| GPU memory sampler | done | **untested** |
| serverless turns | smoke on an M1 Max | **untested** |
| ONNX Runtime rows | not started | not started |

Smoke numbers on an M1 Max with the 0.5B model, pp64 / tg16 tok/s, one CPU
thread for the WebGPU steps: `vendor` 4360 / 228, `webgpu-native` 55.1 / 39.1,
`wasm` 54.5–55.0 / 33.8 in wash-runtime. The vendor-to-WebGPU gap needs a
look before it is published.

### Patched host crate

`wasi-webgpu-wasmtime` (0.3.1 through 0.5.0) never completes
`on_submitted_work_done`: its callback future keeps a stale waker, and nothing
polls the device afterwards. Any guest that waits on queue completion hangs,
llama.cpp at its first queue wait. `repro/queue-wait/` has a minimal repro and
the cause. The workspace builds wash-runtime on a fixed copy in
`vendor/wasi-webgpu-wasmtime/` through `[patch.crates-io]`, until a fixed
release is in `Cargo.lock`.

## Credits

The wasip3 toolchain file and the header shims in `llama-wasip3/` are Mendy
Berger's, from the `benchmark` branch of his llama.cpp fork, which first ran
llama-bench on `wasi:webgpu`.
