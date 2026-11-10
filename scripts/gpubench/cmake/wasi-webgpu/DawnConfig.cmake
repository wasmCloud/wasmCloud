# Stands in for Dawn when llama.cpp's WebGPU backend is built for wasm32-wasip3
# (step 3): Dawn's C++ wrapper over a webgpu.h that wasi-webgpu-headers
# implements on wasi:webgpu imports. The same stub llama.cpp's build-wasi.yml
# writes, with the two checkouts passed in rather than laid out beside it.
if(NOT GPUBENCH_DAWN_HEADERS OR NOT GPUBENCH_WASI_WEBGPU_HEADERS)
  message(FATAL_ERROR "set GPUBENCH_DAWN_HEADERS and GPUBENCH_WASI_WEBGPU_HEADERS")
endif()
set(_wasi_webgpu "${GPUBENCH_WASI_WEBGPU_HEADERS}")
add_library(dawn::webgpu_dawn INTERFACE IMPORTED)
set_target_properties(dawn::webgpu_dawn PROPERTIES
  INTERFACE_SOURCES "${_wasi_webgpu}/webgpu.c;${_wasi_webgpu}/async_futures.c;${_wasi_webgpu}/imports.c;${_wasi_webgpu}/imports_component_type.o"
  INTERFACE_INCLUDE_DIRECTORIES "${GPUBENCH_DAWN_HEADERS}/include/webgpu_upstream;${_wasi_webgpu}")
