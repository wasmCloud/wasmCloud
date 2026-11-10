// Installed as <webgpu/webgpu.h> for step 2: wgpu-native's header, plus the
// enum values the newer webgpu-headers that Dawn's C++ wrapper targets added
// since. Every value the two headers share is identical (wgpu-native.sh checks
// that before installing this), and ggml never passes these six, so they only
// exist to let webgpu_cpp.h compile. Values from webgpu-headers 91beced.
#ifndef GPUBENCH_WGPU_NATIVE_WEBGPU_H_
#define GPUBENCH_WGPU_NATIVE_WEBGPU_H_

#include "wgpu-native-webgpu.h"

#ifndef WGPUFeatureName_SubgroupSizeControl
#define WGPUFeatureName_SubgroupSizeControl ((WGPUFeatureName)0x00000017)
#endif
#ifndef WGPUVertexFormat_Snorm10_10_10_2
#define WGPUVertexFormat_Snorm10_10_10_2 ((WGPUVertexFormat)0x0000002A)
#endif
#ifndef WGPUWGSLLanguageFeatureName_ImmediateAddressSpace
#define WGPUWGSLLanguageFeatureName_ImmediateAddressSpace ((WGPUWGSLLanguageFeatureName)0x0000000B)
#endif
#ifndef WGPUWGSLLanguageFeatureName_BufferView
#define WGPUWGSLLanguageFeatureName_BufferView ((WGPUWGSLLanguageFeatureName)0x0000000C)
#endif
#ifndef WGPUWGSLLanguageFeatureName_SwizzleAssignment
#define WGPUWGSLLanguageFeatureName_SwizzleAssignment ((WGPUWGSLLanguageFeatureName)0x0000000D)
#endif
#ifndef WGPUWGSLLanguageFeatureName_FragmentDepth
#define WGPUWGSLLanguageFeatureName_FragmentDepth ((WGPUWGSLLanguageFeatureName)0x0000000E)
#endif

#endif
