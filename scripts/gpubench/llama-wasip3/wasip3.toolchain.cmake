# wasm32-wasip3 toolchain for llama.cpp. Output executables are components
# (wasm-component-ld). Requires WASI_SDK_PATH pointing at a wasi-sdk 34+ install
# with a wasm32-wasip3 sysroot and the EH runtime (eh/libunwind.a).
#
# Derived from the wasip2 toolchain in llama-cpp-wrappers. Differences: wasip3
# target, process-clocks emulation linked instead of patching common/, and a
# small shim include dir for headers wasi-libc does not ship.

set(WASI_SDK_PATH $ENV{WASI_SDK_PATH})
if(NOT WASI_SDK_PATH OR NOT EXISTS "${WASI_SDK_PATH}/bin/clang")
    message(FATAL_ERROR "Set WASI_SDK_PATH to a valid wasi-sdk (need bin/clang). Got: ${WASI_SDK_PATH}")
endif()

set(CMAKE_SYSTEM_NAME WASI)
set(CMAKE_SYSTEM_VERSION 1)
# Make ggml-cpu take its "wasm" arch branch (arch/wasm/quants.c).
set(CMAKE_SYSTEM_PROCESSOR wasm32)

set(CMAKE_C_COMPILER   "${WASI_SDK_PATH}/bin/clang")
set(CMAKE_CXX_COMPILER "${WASI_SDK_PATH}/bin/clang++")
set(CMAKE_AR           "${WASI_SDK_PATH}/bin/llvm-ar")
set(CMAKE_RANLIB       "${WASI_SDK_PATH}/bin/llvm-ranlib")
set(CMAKE_C_COMPILER_TARGET   wasm32-wasip3)
set(CMAKE_CXX_COMPILER_TARGET wasm32-wasip3)

set(CMAKE_SYSROOT "${WASI_SDK_PATH}/share/wasi-sysroot")
set(CMAKE_FIND_ROOT_PATH "${WASI_SDK_PATH}/share/wasi-sysroot")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# -idirafter: real sysroot headers always win over these shims.
set(_wasi_shims "${CMAKE_CURRENT_LIST_DIR}/wasi-shims")
# -I: these deliberately shadow a sysroot header. See the files for why.
set(_wasi_shims_force "${CMAKE_CURRENT_LIST_DIR}/wasi-shims-force")

# Single-threaded model. _WASI_EMULATED_SIGNAL is compile-only (ggml includes
# signal.h, whose wasi header errors without it); no signal function is called.
# _WASI_EMULATED_PROCESS_CLOCKS lets common.cpp include <sys/resource.h>.
set(_wasi_flags "-mthread-model single -msimd128 -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -D_WASI_EMULATED_MMAN -I ${_wasi_shims_force} -idirafter ${_wasi_shims}")
set(CMAKE_C_FLAGS_INIT   "${_wasi_flags}")
# C++ exceptions via the wasm EH proposal; -wasm-use-legacy-eh=false selects the
# standardized exnref encoding. Run with wasmtime 46+ (`-W exceptions=y`).
# -include errno.h/stdlib.h: some sources use errno/getenv without including them.
set(CMAKE_CXX_FLAGS_INIT "${_wasi_flags} -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false -include errno.h -include stdlib.h")
# -ldl provides wasi-libc's dlopen stubs, so ggml's dl code builds unmodified.
# The default 64 KiB shadow stack overflows in llama_context graph building; the
# overflow wraps past 0 and shows up as an out-of-bounds access near 0xffffffff.
set(_wasi_link "-fwasm-exceptions -lunwind -ldl -lwasi-emulated-process-clocks -lwasi-emulated-mman -Wl,-z,stack-size=16777216")
set(CMAKE_EXE_LINKER_FLAGS_INIT "${_wasi_link}")

# wasi-libc provides pthread stubs in the default (single-threaded) sysroot, but
# CMake's FindThreads test-compile can't detect them. Assert their presence so
# find_package(Threads REQUIRED) succeeds without a real -pthread.
set(CMAKE_THREAD_LIBS_INIT "")
set(CMAKE_HAVE_THREADS_LIBRARY 1)
set(CMAKE_USE_WIN32_THREADS_INIT 0)
set(CMAKE_USE_PTHREADS_INIT 1)
set(THREADS_PREFER_PTHREAD_FLAG OFF)
