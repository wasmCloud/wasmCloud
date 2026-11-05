#!/usr/bin/env bash
# Build llama-bench for one step of the gap bench, from the llama.cpp commit in
# pins.env. All three steps build the same source with the same patches:
#
#   vendor         the vendor backend: Metal on macOS, CUDA on Linux
#   webgpu-native  ggml-webgpu on wgpu-native (built by build/wgpu-native.sh)
#   wasm           ggml-webgpu for wasm32-wasip3 on wasi-webgpu-headers, run by
#                  crates/bench-gpu inside wash-runtime
#
# The two WebGPU steps both build with Dawn's native-only features off
# (GGML_WEBGPU_PORTABLE natively, __wasi__ for wasm), so they run the same
# shaders on the same features and differ only in what implements webgpu.h.
#
#   build/llama.sh STEP        prints the bin directory holding llama-bench
#                              (the gap bench) and llama-simple (the
#                              serverless turns)

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

# Callers capture stdout as the bin directory, so the build logs to stderr.
exec 3>&1 1>&2

step="${1:?usage: llama.sh vendor|webgpu-native|wasm}"
kind="$(gpubench_kind)"
src="$src_dir/llama.cpp"
out="$build_root/llama-$step"

fetch_git "$LLAMA_CPP_REPO" "$LLAMA_CPP_COMMIT" "$src"
apply_patches "$src" "$gpubench_dir/llama-patches"

# No server and no network. Examples are configured so llama-simple can be
# built; only the two targets below are.
common=(
  -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF
  -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF
  -DLLAMA_BUILD_EXAMPLES=ON -DLLAMA_BUILD_SERVER=OFF -DLLAMA_BUILD_TOOLS=ON
)

case "$step" in
  vendor)
    case "$kind" in
      metal) backend=(-DGGML_METAL=ON -DGGML_BLAS=OFF) ;;
      cuda) backend=(-DGGML_CUDA=ON -DGGML_BLAS=OFF "-DCMAKE_CUDA_ARCHITECTURES=${GPUBENCH_CUDA_ARCH:?set GPUBENCH_CUDA_ARCH, e.g. 89}") ;;
      *) die "no vendor backend for kind '$kind'" ;;
    esac
    cmake -S "$src" -B "$out" "${common[@]}" "${backend[@]}"
    ;;
  webgpu-native)
    wgpu="$("$gpubench_dir/build/wgpu-native.sh")"
    cmake -S "$src" -B "$out" "${common[@]}" \
      -DGGML_METAL=OFF -DGGML_BLAS=OFF -DGGML_CUDA=OFF -DGGML_WEBGPU=ON \
      -DCMAKE_C_FLAGS=-DGGML_WEBGPU_PORTABLE -DCMAKE_CXX_FLAGS=-DGGML_WEBGPU_PORTABLE \
      -DDawn_DIR="$gpubench_dir/cmake/wgpu-native" \
      -DGPUBENCH_DAWN_HEADERS="$(dawn_headers_path)/dawn-headers" \
      -DGPUBENCH_WGPU_NATIVE="$wgpu"
    ;;
  wasm)
    WASI_SDK_PATH="$(wasi_sdk_path)"
    export WASI_SDK_PATH
    fetch_git "$WASI_WEBGPU_HEADERS_REPO" "$WASI_WEBGPU_HEADERS_COMMIT" "$src_dir/wasi-webgpu-headers" --submodules
    cmake -S "$src" -B "$out" "${common[@]}" \
      -DCMAKE_TOOLCHAIN_FILE="$gpubench_dir/llama-wasip3/wasip3.toolchain.cmake" \
      -DCMAKE_HAVE_LIBC_PTHREAD=1 -DGGML_NATIVE=OFF -DGGML_OPENMP=OFF -DGGML_BACKEND_DL=OFF \
      -DGGML_WEBGPU=ON \
      -DDawn_DIR="$gpubench_dir/cmake/wasi-webgpu" \
      -DGPUBENCH_DAWN_HEADERS="$(dawn_headers_path)/dawn-headers" \
      -DGPUBENCH_WASI_WEBGPU_HEADERS="$src_dir/wasi-webgpu-headers"
    ;;
  *) die "unknown step '$step'" ;;
esac

cmake --build "$out" --target llama-bench llama-simple -j "$(ncpu)"
echo "$out/bin" >&3
