// TODO: in this file:
// - Remove all calls to `Default::default()`. Instead, manually set them, and link to the spec stating what defaults should be used.
// - Implement all todos.
// - Remove all unwraps.
// - Implement all the drop handlers.
// - Remove clippy allows, and either fix the code, or add comments explaining why it's okay to leave it.

#![allow(clippy::unwrap_or_default)]
#![allow(clippy::new_without_default)]

use std::sync::Arc;

use wasmtime::component::HasData;

// ToCore trait used for resources, records, and variants.
// Into trait used for enums and flags, since they never need table access.
mod enum_conversions;
mod flags_conversions;
mod to_core_conversions;

mod trait_impls;
mod types;
pub use types::*;

/// Re-export of `wgpu_core` and `wgpu_types` so that runtime implementors don't need to keep track of what version of wgpu this crate is using.
pub mod reexports {
    pub use wgpu_core;
    pub use wgpu_types;
}

// https://searchfox.org/mozilla-central/source/dom/webgpu/Instance.h#68
#[cfg(target_os = "android")]
const PREFERRED_CANVAS_FORMAT: wasi::webgpu::webgpu::GpuTextureFormat =
    wasi::webgpu::webgpu::GpuTextureFormat::Rgba8unorm;
#[cfg(not(target_os = "android"))]
const PREFERRED_CANVAS_FORMAT: wasi::webgpu::webgpu::GpuTextureFormat =
    wasi::webgpu::webgpu::GpuTextureFormat::Bgra8unorm;

#[cfg(all(
    not(target_os = "linux"),
    not(target_os = "android"),
    not(target_os = "windows"),
    not(target_os = "macos"),
    not(target_os = "ios"),
))]
pub(crate) type Backend = wgpu_core::api::Gl;

wasmtime::component::bindgen!({
    world: "wasi:webgpu/imports",
    imports: {
        "wasi:webgpu/webgpu.[method]gpu-device.lost": store | trappable,
        "wasi:webgpu/webgpu.[method]gpu-device.on-uncaptured-error": store | trappable,
        default: trappable,
    },
    with: {
        "wasi:webgpu/webgpu.gpu-adapter": types::Adapter,
        "wasi:webgpu/webgpu.gpu-device": types::Device,
        "wasi:webgpu/webgpu.gpu-queue": types::Queue,
        "wasi:webgpu/webgpu.gpu-command-encoder": types::CommandEncoder,
        "wasi:webgpu/webgpu.gpu-render-pass-encoder": types::RenderPassEncoder,
        "wasi:webgpu/webgpu.gpu-compute-pass-encoder": types::ComputePassEncoder,
        "wasi:webgpu/webgpu.gpu-shader-module": wgpu_core::id::ShaderModuleId,
        "wasi:webgpu/webgpu.gpu-render-pipeline": types::RenderPipeline,
        "wasi:webgpu/webgpu.gpu-render-bundle-encoder": types::RenderBundleEncoder,
        "wasi:webgpu/webgpu.gpu-render-bundle": wgpu_core::id::RenderBundleId,
        "wasi:webgpu/webgpu.gpu-command-buffer": wgpu_core::id::CommandBufferId,
        "wasi:webgpu/webgpu.gpu-buffer": types::Buffer,
        "wasi:webgpu/webgpu.gpu-pipeline-layout": wgpu_core::id::PipelineLayoutId,
        "wasi:webgpu/webgpu.gpu-bind-group-layout": wgpu_core::id::BindGroupLayoutId,
        "wasi:webgpu/webgpu.gpu-sampler": wgpu_core::id::SamplerId,
        "wasi:webgpu/webgpu.gpu-supported-features": wgpu_types::Features,
        "wasi:webgpu/webgpu.gpu-texture": types::Texture,
        "wasi:webgpu/webgpu.gpu-compute-pipeline": types::ComputePipeline,
        "wasi:webgpu/webgpu.gpu-bind-group": wgpu_core::id::BindGroupId,
        "wasi:webgpu/webgpu.gpu-texture-view": wgpu_core::id::TextureViewId,
        "wasi:webgpu/webgpu.gpu-adapter-info": wgpu_types::AdapterInfo,
        "wasi:webgpu/webgpu.gpu-query-set": wgpu_core::id::QuerySetId,
        "wasi:webgpu/webgpu.gpu-supported-limits": wgpu_types::Limits,
        "wasi:webgpu/webgpu.gpu-error": types::GpuError,
        "wasi:webgpu/webgpu.wgsl-language-features": types::WgslLanguageFeatures,
        "wasi:webgpu/webgpu.record-gpu-pipeline-constant-value": types::RecordGpuPipelineConstantValue,
        "wasi:webgpu/webgpu.record-option-gpu-size64": types::RecordOptionGpuSize64,
    },
});

// linker connection
pub fn add_to_linker<T>(l: &mut wasmtime::component::Linker<T>) -> wasmtime::Result<()>
where
    T: WasiWebGpuCtxView,
{
    wasi::webgpu::webgpu::add_to_linker::<_, HasWasiWebGpuCtx>(l, T::webgpu_ctx)?;
    Ok(())
}

/// returns a struct of references.
/// Returning all references in a struct allows us to use different mutable references at the same time.
pub trait WasiWebGpuCtxView: Send {
    /// Return a [WasiWebGpu] from mutable reference to self.
    fn webgpu_ctx(&mut self) -> WasiWebGpuCtx<'_>;
}

pub struct WasiWebGpuCtx<'a> {
    // wrapped in arc to allow cloning for async. might be able to remove
    pub instance: &'a Arc<wgpu_core::global::Global>,
    pub table: &'a mut wasmtime_wasi::ResourceTable,
    pub options: &'a WasiWebGpuOptions,
}

/// Extra WebGPU options
#[derive(Clone, Debug, Default)]
pub struct WasiWebGpuOptions {
    /// Memory allocation strategy passed to wgpu on every `request-device`.
    pub device_memory_hints: wgpu_types::MemoryHints,
}

struct HasWasiWebGpuCtx;

impl HasData for HasWasiWebGpuCtx {
    type Data<'a> = WasiWebGpuCtx<'a>;
}
