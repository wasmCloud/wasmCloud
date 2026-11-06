// Minimal repro: wait on queue completion through wasi-webgpu-headers.
//
//   repro MODE [timed]
//
// MODE picks what precedes OnSubmittedWorkDone + WaitAny:
//   empty   nothing
//   write   wgpuQueueWriteBuffer (what ggml does first)
//   submit  an empty command buffer, submitted
//   both    write, then submit
//   map     no work-done wait: MapAsync(Read) + WaitAny instead
// `timed` waits with a 10 s timeout, as ggml does, instead of UINT64_MAX.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "webgpu/webgpu.h"

static uint64_t g_timeout = UINT64_MAX;

#define STEP(...) (fprintf(stderr, __VA_ARGS__), fputc('\n', stderr), fflush(stderr))

static void on_adapter(WGPURequestAdapterStatus status, WGPUAdapter adapter, WGPUStringView msg,
                       void* ud1, void* ud2) {
    (void)msg; (void)ud2;
    if (status != WGPURequestAdapterStatus_Success) { STEP("adapter failed"); exit(2); }
    *(WGPUAdapter*)ud1 = adapter;
}

static void on_device(WGPURequestDeviceStatus status, WGPUDevice device, WGPUStringView msg,
                      void* ud1, void* ud2) {
    (void)msg; (void)ud2;
    if (status != WGPURequestDeviceStatus_Success) { STEP("device failed"); exit(2); }
    *(WGPUDevice*)ud1 = device;
}

static void on_done(WGPUQueueWorkDoneStatus status, WGPUStringView msg, void* ud1, void* ud2) {
    (void)msg; (void)ud2;
    *(int*)ud1 = (int)status;
}

static void on_map(WGPUMapAsyncStatus status, WGPUStringView msg, void* ud1, void* ud2) {
    (void)msg; (void)ud2;
    *(int*)ud1 = (int)status;
}

static void wait(WGPUInstance instance, WGPUFuture future, const char* what) {
    STEP("waiting: %s (future %llu)", what, (unsigned long long)future.id);
    WGPUFutureWaitInfo info = {.future = future};
    WGPUWaitStatus s = wgpuInstanceWaitAny(instance, 1, &info, g_timeout);
    STEP("done:    %s (wait status %d, completed %d)", what, (int)s, (int)info.completed);
}

int main(int argc, char** argv) {
    const char* mode = argc > 1 ? argv[1] : "write";
    if (argc > 2 && strcmp(argv[2], "timed") == 0) g_timeout = 10ull * 1000 * 1000 * 1000;

    WGPUInstance instance = wgpuCreateInstance(NULL);
    STEP("instance");

    WGPUAdapter adapter = NULL;
    wait(instance,
         wgpuInstanceRequestAdapter(instance, NULL,
             (WGPURequestAdapterCallbackInfo){.mode = WGPUCallbackMode_AllowSpontaneous,
                                              .callback = on_adapter, .userdata1 = &adapter}),
         "request adapter");

    WGPUDevice device = NULL;
    wait(instance,
         wgpuAdapterRequestDevice(adapter, NULL,
             (WGPURequestDeviceCallbackInfo){.mode = WGPUCallbackMode_AllowSpontaneous,
                                             .callback = on_device, .userdata1 = &device}),
         "request device");
    WGPUQueue queue = wgpuDeviceGetQueue(device);

    WGPUBufferDescriptor desc = {.size = 256};
    if (strcmp(mode, "map") == 0) {
        desc.usage = WGPUBufferUsage_MapRead | WGPUBufferUsage_CopyDst;
    } else {
        desc.usage = WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst;
    }
    WGPUBuffer buffer = wgpuDeviceCreateBuffer(device, &desc);
    static uint8_t data[256];

    if (strcmp(mode, "write") == 0 || strcmp(mode, "both") == 0) {
        wgpuQueueWriteBuffer(queue, buffer, 0, data, sizeof data);
        STEP("wrote buffer");
    }
    if (strcmp(mode, "submit") == 0 || strcmp(mode, "both") == 0) {
        WGPUCommandEncoder enc = wgpuDeviceCreateCommandEncoder(device, NULL);
        WGPUCommandBuffer cmd = wgpuCommandEncoderFinish(enc, NULL);
        wgpuQueueSubmit(queue, 1, &cmd);
        STEP("submitted");
    }

    if (strcmp(mode, "map") == 0) {
        int status = -1;
        wait(instance,
             wgpuBufferMapAsync(buffer, WGPUMapMode_Read, 0, sizeof data,
                 (WGPUBufferMapCallbackInfo){.mode = WGPUCallbackMode_AllowSpontaneous,
                                             .callback = on_map, .userdata1 = &status}),
             "map");
        STEP("map status %d", status);
    } else {
        int status = -1;
        wait(instance,
             wgpuQueueOnSubmittedWorkDone(queue,
                 (WGPUQueueWorkDoneCallbackInfo){.mode = WGPUCallbackMode_AllowSpontaneous,
                                                 .callback = on_done, .userdata1 = &status}),
             "on submitted work done");
        STEP("work done status %d", status);
    }
    STEP("ok");
    return 0;
}
