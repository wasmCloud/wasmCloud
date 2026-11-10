# Stands in for Dawn when llama.cpp's WebGPU backend is built natively for
# step 2: Dawn's C++ wrapper over the webgpu.h that wgpu-native implements on
# wgpu-core. Steps 2 and 3 then differ only in what sits under webgpu.h, which
# is the overhead the gap bench measures.
if(NOT GPUBENCH_DAWN_HEADERS OR NOT GPUBENCH_WGPU_NATIVE)
  message(FATAL_ERROR "set GPUBENCH_DAWN_HEADERS and GPUBENCH_WGPU_NATIVE")
endif()
add_library(dawn::webgpu_dawn STATIC IMPORTED)
set(_libs "")
if(APPLE)
  list(APPEND _libs "-framework Metal" "-framework QuartzCore" "-framework CoreFoundation"
    "-framework Foundation" "-framework IOKit" "-framework IOSurface")
elseif(UNIX)
  list(APPEND _libs dl pthread m)
endif()
set_target_properties(dawn::webgpu_dawn PROPERTIES
  IMPORTED_LOCATION "${GPUBENCH_WGPU_NATIVE}/lib/libwgpu_native.a"
  INTERFACE_LINK_LIBRARIES "${_libs}"
  INTERFACE_INCLUDE_DIRECTORIES "${GPUBENCH_DAWN_HEADERS}/include/webgpu_upstream;${GPUBENCH_WGPU_NATIVE}/include")
