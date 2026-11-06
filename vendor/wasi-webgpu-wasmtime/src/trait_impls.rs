use core::slice;
use shared::StreamPipeMap;
use std::{
    borrow::Cow,
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, Mutex},
};
use wasmtime::component::{Access, Accessor, FutureReader, Resource, StreamReader};

use crate::{
    device_handle::{aborted, DeviceHandle},
    lost::{DeviceLostFuture, LostWait},
    to_core_conversions::ToCore,
    types::{
        Buffer, CommandEncoder, ComputePassEncoder, ComputePipeline, Device, ErrorHandler, Queue,
        RenderBundleEncoder, RenderBundleEncoderInner, RenderPassEncoder, RenderPipeline, Texture,
        WgslLanguageFeatures,
    },
    wasi::webgpu::webgpu,
    WasiWebGpuCtx, WasiWebGpuCtxView, PREFERRED_CANVAS_FORMAT,
};

impl<'a> webgpu::Host for WasiWebGpuCtx<'a> {
    fn get_gpu(&mut self) -> wasmtime::Result<Resource<webgpu::Gpu>> {
        Ok(Resource::new_own(0))
    }
}

impl<'a> webgpu::HostRecordGpuPipelineConstantValue for WasiWebGpuCtx<'a> {
    fn new(&mut self) -> wasmtime::Result<Resource<webgpu::RecordGpuPipelineConstantValue>> {
        Ok(self.table.push(HashMap::new())?)
    }

    fn add(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
        key: String,
        value: webgpu::GpuPipelineConstantValue,
    ) -> wasmtime::Result<()> {
        let record = self.table.get_mut(&record)?;
        record.insert(key, value);
        Ok(())
    }

    fn get(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
        key: String,
    ) -> wasmtime::Result<Option<webgpu::GpuPipelineConstantValue>> {
        let record = self.table.get(&record)?;
        let value = record.get(&key).copied();
        Ok(value)
    }

    fn has(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
        key: String,
    ) -> wasmtime::Result<bool> {
        let record = self.table.get(&record)?;
        Ok(record.contains_key(&key))
    }

    fn remove(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
        key: String,
    ) -> wasmtime::Result<()> {
        let record = self.table.get_mut(&record)?;
        record.remove(&key);
        Ok(())
    }

    fn keys(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
    ) -> wasmtime::Result<Vec<String>> {
        let record = self.table.get(&record)?;
        let keys = record.keys().cloned().collect();
        Ok(keys)
    }

    fn values(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
    ) -> wasmtime::Result<Vec<webgpu::GpuPipelineConstantValue>> {
        let record = self.table.get(&record)?;
        let values = record.values().copied().collect();
        Ok(values)
    }

    fn entries(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
    ) -> wasmtime::Result<Vec<(String, webgpu::GpuPipelineConstantValue)>> {
        let record = self.table.get(&record)?;
        let entries = record.iter().map(|(k, v)| (k.clone(), *v)).collect();
        Ok(entries)
    }

    fn drop(
        &mut self,
        record: Resource<webgpu::RecordGpuPipelineConstantValue>,
    ) -> wasmtime::Result<()> {
        self.table.delete(record)?;
        Ok(())
    }
}

impl<'a> webgpu::HostRecordOptionGpuSize64 for WasiWebGpuCtx<'a> {
    fn new(&mut self) -> wasmtime::Result<Resource<webgpu::RecordOptionGpuSize64>> {
        let record = std::collections::HashMap::new();
        Ok(self.table.push(record)?)
    }
    fn add(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
        key: String,
        value: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let record = self.table.get_mut(&record)?;
        record.insert(key, value);
        Ok(())
    }
    fn get(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
        key: String,
    ) -> wasmtime::Result<Option<Option<webgpu::GpuSize64>>> {
        let record = self.table.get(&record)?;
        Ok(record.get(&key).copied())
    }
    fn has(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
        key: String,
    ) -> wasmtime::Result<bool> {
        let record = self.table.get(&record)?;
        Ok(record.contains_key(&key))
    }
    fn remove(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
        key: String,
    ) -> wasmtime::Result<()> {
        let record = self.table.get_mut(&record)?;
        record.remove(&key);
        Ok(())
    }
    fn keys(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
    ) -> wasmtime::Result<Vec<String>> {
        let record = self.table.get(&record)?;
        Ok(record.keys().cloned().collect())
    }
    fn values(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
    ) -> wasmtime::Result<Vec<Option<webgpu::GpuSize64>>> {
        let record = self.table.get(&record)?;
        Ok(record.values().cloned().collect())
    }
    fn entries(
        &mut self,
        record: Resource<webgpu::RecordOptionGpuSize64>,
    ) -> wasmtime::Result<Vec<(String, Option<webgpu::GpuSize64>)>> {
        let record = self.table.get(&record)?;
        Ok(record.iter().map(|(k, v)| (k.clone(), *v)).collect())
    }
    fn drop(&mut self, record: Resource<webgpu::RecordOptionGpuSize64>) -> wasmtime::Result<()> {
        self.table.delete(record)?;
        Ok(())
    }
}

impl<'a> webgpu::HostGpuDevice for WasiWebGpuCtx<'a> {
    fn adapter_info(
        &mut self,
        device: Resource<Device>,
    ) -> wasmtime::Result<Resource<webgpu::GpuAdapterInfo>> {
        let adapter_id = *self.table.get(&device)?.adapter;
        let info = self.instance.adapter_get_info(adapter_id);
        let info = self.table.push(info)?;
        Ok(info)
    }

    fn create_command_encoder(
        &mut self,
        device: Resource<Device>,
        descriptor: Option<webgpu::GpuCommandEncoderDescriptor>,
    ) -> wasmtime::Result<Resource<CommandEncoder>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (command_encoder_id, err) = self.instance.device_create_command_encoder(
            device_id,
            &descriptor
                .map(|d| d.to_core(self.table))
                .unwrap_or(wgpu_types::CommandEncoderDescriptor::default()),
            None,
        );

        error_handler.handle_possible_error(err);

        let command_encoder = self.table.push(CommandEncoder {
            command_encoder_id,
            error_handler,
        })?;
        Ok(command_encoder)
    }

    fn create_shader_module(
        &mut self,
        device: Resource<Device>,
        descriptor: webgpu::GpuShaderModuleDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuShaderModule>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let code =
            wgpu_core::pipeline::ShaderModuleSource::Wgsl(Cow::Owned(descriptor.code.to_owned()));
        let (shader, err) = self.instance.device_create_shader_module(
            device_id,
            &descriptor.to_core(self.table),
            code,
            None,
        );

        error_handler.handle_possible_error(err);

        Ok(self.table.push(shader)?)
    }

    fn create_render_pipeline(
        &mut self,
        device: Resource<Device>,
        descriptor: webgpu::GpuRenderPipelineDescriptor,
    ) -> wasmtime::Result<Resource<RenderPipeline>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (render_pipeline_id, err) = self.instance.device_create_render_pipeline(
            device_id,
            &descriptor.to_core(self.table),
            None,
        );

        error_handler.handle_possible_error(err);

        let render_pipeline = self.table.push(RenderPipeline {
            render_pipeline_id,
            error_handler,
        })?;
        Ok(render_pipeline)
    }

    fn queue(&mut self, device: Resource<Device>) -> wasmtime::Result<Resource<webgpu::GpuQueue>> {
        let device = self.table.get(&device)?;
        let queue = Queue {
            handle: Arc::clone(&device.handle),
            error_handler: Arc::clone(&device.error_handler),
        };
        Ok(self.table.push(queue)?)
    }

    fn features(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
    ) -> wasmtime::Result<Resource<webgpu::GpuSupportedFeatures>> {
        let device = *self.table.get(&device)?.device_id();
        let features = self.instance.device_features(device);
        Ok(self.table.push(features)?)
    }

    fn limits(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
    ) -> wasmtime::Result<Resource<webgpu::GpuSupportedLimits>> {
        let device = *self.table.get(&device)?.device_id();
        let limits = self.instance.device_limits(device);
        Ok(self.table.push(limits)?)
    }

    fn destroy(&mut self, device: Resource<webgpu::GpuDevice>) -> wasmtime::Result<()> {
        self.table.get(&device)?.handle.destroy();
        Ok(())
    }

    fn create_buffer(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuBufferDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuBuffer>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let device_handle = Arc::clone(&device.handle);
        let error_handler = Arc::clone(&device.error_handler);
        let descriptor = descriptor.to_core(self.table);

        let size = descriptor.size;
        let usage = descriptor.usage;
        let map_state = Arc::new(Mutex::new(match descriptor.mapped_at_creation {
            true => webgpu::GpuBufferMapState::Mapped,
            false => webgpu::GpuBufferMapState::Unmapped,
        }));

        let (buffer_id, err) = self
            .instance
            .device_create_buffer(device_id, &descriptor, None);

        error_handler.handle_possible_error(err);

        let buffer = Buffer {
            buffer_id,
            device: device_handle,
            size,
            usage,
            map_state,
        };

        Ok(self.table.push(buffer)?)
    }

    fn create_texture(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuTextureDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuTexture>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (texture_id, err) =
            self.instance
                .device_create_texture(device_id, &descriptor.to_core(self.table), None);

        error_handler.handle_possible_error(err);

        Ok(self.table.push(Texture {
            texture_id,
            error_handler,
        })?)
    }

    fn create_sampler(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: Option<webgpu::GpuSamplerDescriptor>,
    ) -> wasmtime::Result<Resource<webgpu::GpuSampler>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let descriptor = descriptor
            .map(|d| d.to_core(self.table))
            // https://www.w3.org/TR/webgpu/#dictdef-gpusamplerdescriptor
            .unwrap_or_else(|| wgpu_core::resource::SamplerDescriptor {
                label: None,
                address_modes: [wgpu_types::AddressMode::ClampToEdge; 3],
                mag_filter: wgpu_types::FilterMode::Nearest,
                min_filter: wgpu_types::FilterMode::Nearest,
                mipmap_filter: wgpu_types::MipmapFilterMode::Nearest,
                lod_min_clamp: 0.0,
                lod_max_clamp: 32.0,
                compare: None,
                // TODO: make sure that anisotropy_clamp actually corresponds to maxAnisotropy
                anisotropy_clamp: 1,
                // border_color is not present in WebGPU
                border_color: None,
            });

        let (sampler, err) = self
            .instance
            .device_create_sampler(device_id, &descriptor, None);

        error_handler.handle_possible_error(err);

        Ok(self.table.push(sampler)?)
    }

    fn create_bind_group_layout(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuBindGroupLayoutDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuBindGroupLayout>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (bind_group_layout, err) = self.instance.device_create_bind_group_layout(
            device_id,
            &descriptor.to_core(self.table),
            None,
        );

        error_handler.handle_possible_error(err);

        Ok(self.table.push(bind_group_layout)?)
    }

    fn create_pipeline_layout(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuPipelineLayoutDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuPipelineLayout>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (pipeline_layout, err) = self.instance.device_create_pipeline_layout(
            device_id,
            &descriptor.to_core(self.table),
            None,
        );

        error_handler.handle_possible_error(err);

        Ok(self.table.push(pipeline_layout)?)
    }

    fn create_bind_group(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuBindGroupDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuBindGroup>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        // not using to_core for conversion since we need instance or self for `GpuBindingResource::GpuTexture`
        let descriptor = wgpu_core::binding_model::BindGroupDescriptor {
            label: descriptor.label.map(|l| l.into()),
            layout: descriptor.layout.to_core(self.table),
            entries: descriptor
                .entries
                .into_iter()
                .map(|entry| wgpu_core::binding_model::BindGroupEntry {
                    binding: entry.binding,
                    resource: match entry.resource {
                        webgpu::GpuBindingResource::GpuBuffer(buffer) => {
                            let binding = webgpu::GpuBufferBinding {
                                buffer,
                                offset: None,
                                size: None,
                            };
                            wgpu_core::binding_model::BindingResource::Buffer(
                                binding.to_core(self.table),
                            )
                        }
                        webgpu::GpuBindingResource::GpuBufferBinding(buffer) => {
                            wgpu_core::binding_model::BindingResource::Buffer(
                                buffer.to_core(self.table),
                            )
                        }
                        webgpu::GpuBindingResource::GpuSampler(sampler) => {
                            wgpu_core::binding_model::BindingResource::Sampler(
                                sampler.to_core(self.table),
                            )
                        }
                        webgpu::GpuBindingResource::GpuTexture(texture) => {
                            let view =
                                webgpu::HostGpuTexture::create_view(self, texture, None).unwrap();
                            let view = *self.table.get(&view).unwrap();
                            wgpu_core::binding_model::BindingResource::TextureView(view)
                        }
                        webgpu::GpuBindingResource::GpuTextureView(texture_view) => {
                            wgpu_core::binding_model::BindingResource::TextureView(
                                texture_view.to_core(self.table),
                            )
                        }
                    },
                })
                .collect(),
        };

        let (bind_group, err) =
            self.instance
                .device_create_bind_group(device_id, &descriptor, None);

        error_handler.handle_possible_error(err);

        Ok(self.table.push(bind_group)?)
    }

    fn create_compute_pipeline(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuComputePipelineDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuComputePipeline>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (compute_pipeline_id, err) = self.instance.device_create_compute_pipeline(
            device_id,
            &descriptor.to_core(self.table),
            None,
        );

        error_handler.handle_possible_error(err);

        Ok(self.table.push(ComputePipeline {
            compute_pipeline_id,
            error_handler,
        })?)
    }

    fn create_render_bundle_encoder(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuRenderBundleEncoderDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuRenderBundleEncoder>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);
        let render_bundle_encoder = wgpu_core::command::RenderBundleEncoder::new(
            &descriptor.to_core(self.table),
            device_id,
        )?;
        let render_bundle_encoder =
            self.table
                .push(RenderBundleEncoder::new(RenderBundleEncoderInner {
                    render_bundle_encoder,
                    error_handler,
                }))?;
        Ok(render_bundle_encoder)
    }

    fn create_query_set(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuQuerySetDescriptor,
    ) -> wasmtime::Result<Result<Resource<webgpu::GpuQuerySet>, webgpu::CreateQuerySetError>> {
        let device = self.table.get(&device)?;
        let device_id = *device.device_id();
        let error_handler = Arc::clone(&device.error_handler);

        let (query_set, err) =
            self.instance
                .device_create_query_set(device_id, &descriptor.to_core(self.table), None);

        error_handler.handle_possible_error(err);

        Ok(Ok(self.table.push(query_set)?))
    }

    fn label(&mut self, _device: Resource<webgpu::GpuDevice>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _device: Resource<webgpu::GpuDevice>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn push_error_scope(
        &mut self,
        device: Resource<webgpu::GpuDevice>,
        filter: webgpu::GpuErrorFilter,
    ) -> wasmtime::Result<()> {
        let device = self.table.get(&device)?;
        device.error_handler.push_scope(filter);
        Ok(())
    }

    fn drop(&mut self, device: Resource<webgpu::GpuDevice>) -> wasmtime::Result<()> {
        let device = self.table.delete(device)?;
        if let Some(adapter_id) = Arc::into_inner(device.adapter) {
            self.instance.adapter_drop(adapter_id);
        }
        Ok(())
    }
}

impl<T: Send + WasiWebGpuCtxView> webgpu::HostGpuDeviceWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn create_compute_pipeline_async(
        accessor: &Accessor<T, Self>,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuComputePipelineDescriptor,
    ) -> wasmtime::Result<Result<Resource<webgpu::GpuComputePipeline>, webgpu::CreatePipelineError>>
    {
        accessor.with(|mut access| {
            let ctx = access.get();
            let device = ctx.table.get(&device)?;
            let device_id = *device.device_id();
            let error_handler = Arc::clone(&device.error_handler);

            let (compute_pipeline_id, err) = ctx.instance.device_create_compute_pipeline(
                device_id,
                &descriptor.to_core(ctx.table),
                None,
            );

            error_handler.handle_possible_error(err);

            Ok(Ok(ctx.table.push(ComputePipeline {
                compute_pipeline_id,
                error_handler,
            })?))
        })
    }

    async fn create_render_pipeline_async(
        accessor: &Accessor<T, Self>,
        device: Resource<webgpu::GpuDevice>,
        descriptor: webgpu::GpuRenderPipelineDescriptor,
    ) -> wasmtime::Result<Result<Resource<webgpu::GpuRenderPipeline>, webgpu::CreatePipelineError>>
    {
        accessor.with(|mut access| {
            let ctx = access.get();
            let device = ctx.table.get(&device)?;
            let device_id = *device.device_id();
            let error_handler = Arc::clone(&device.error_handler);

            let (render_pipeline_id, err) = ctx.instance.device_create_render_pipeline(
                device_id,
                &descriptor.to_core(ctx.table),
                None,
            );

            error_handler.handle_possible_error(err);

            let render_pipeline = ctx.table.push(RenderPipeline {
                render_pipeline_id,
                error_handler,
            })?;
            Ok(Ok(render_pipeline))
        })
    }

    async fn pop_error_scope(
        accessor: &Accessor<T, Self>,
        device: Resource<webgpu::GpuDevice>,
    ) -> wasmtime::Result<Result<Option<Resource<webgpu::GpuError>>, webgpu::PopErrorScopeError>>
    {
        accessor.with(|mut access| {
            let ctx = access.get();
            let device = ctx.table.get(&device)?;
            let error = device.error_handler.pop_scope();
            let error = error.map(|error| error.map(|error| ctx.table.push(error).unwrap()));
            Ok(error)
        })
    }

    fn on_uncaptured_error(
        mut access: Access<T, Self>,
        device: Resource<webgpu::GpuDevice>,
    ) -> wasmtime::Result<StreamReader<Resource<webgpu::GpuError>>> {
        let ctx = access.get();
        let receiver = ctx
            .table
            .get(&device)
            .unwrap()
            .error_handler
            .new_error_receiver();
        Ok(StreamReader::new(
            access,
            StreamPipeMap(receiver, |data: &mut T, err| {
                Ok(data.webgpu_ctx().table.push(err)?)
            }),
        )
        .unwrap())
    }

    fn lost(
        mut access: Access<T, Self>,
        device: Resource<webgpu::GpuDevice>,
    ) -> wasmtime::Result<FutureReader<Resource<webgpu::GpuDeviceLostInfo>>> {
        let lost = Arc::clone(access.get().table.get(&device)?.handle.lost());
        FutureReader::new(access, DeviceLostFuture(LostWait::new(lost)))
    }
}

impl<'a> webgpu::HostGpuTexture for WasiWebGpuCtx<'a> {
    fn create_view(
        &mut self,
        texture: Resource<Texture>,
        descriptor: Option<webgpu::GpuTextureViewDescriptor>,
    ) -> wasmtime::Result<Resource<wgpu_core::id::TextureViewId>> {
        let texture = self.table.get(&texture)?;
        let texture_id = texture.texture_id;
        let error_handler = Arc::clone(&texture.error_handler);
        let (texture_view, err) = self.instance.texture_create_view(
            texture_id,
            &descriptor
                .map(|d| d.to_core(self.table))
                .unwrap_or(wgpu_core::resource::TextureViewDescriptor::default()),
            None,
        );
        error_handler.handle_possible_error(err);
        Ok(self.table.push(texture_view)?)
    }

    fn destroy(&mut self, texture: Resource<webgpu::GpuTexture>) -> wasmtime::Result<()> {
        let texture = self.table.get(&texture)?.texture_id;
        self.instance.texture_destroy(texture);
        Ok(())
    }

    fn width(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuIntegerCoordinateOut> {
        todo!()
    }

    fn height(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuIntegerCoordinateOut> {
        todo!()
    }

    fn depth_or_array_layers(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuIntegerCoordinateOut> {
        todo!()
    }

    fn mip_level_count(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuIntegerCoordinateOut> {
        todo!()
    }

    fn sample_count(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuSize32Out> {
        todo!()
    }

    fn dimension(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuTextureDimension> {
        todo!()
    }

    fn format(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuTextureFormat> {
        todo!()
    }

    fn usage(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<webgpu::GpuTextureUsage> {
        todo!()
    }

    fn texture_binding_view_dimension(
        &mut self,
        _texture: Resource<webgpu::GpuTexture>,
    ) -> wasmtime::Result<Option<webgpu::GpuTextureViewDimension>> {
        todo!()
    }

    fn label(&mut self, _self_: Resource<webgpu::GpuTexture>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuTexture>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, texture: Resource<webgpu::GpuTexture>) -> wasmtime::Result<()> {
        let texture = self.table.delete(texture)?;
        self.instance.texture_drop(texture.texture_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuTextureView for WasiWebGpuCtx<'a> {
    fn label(
        &mut self,
        _self_: Resource<wgpu_core::id::TextureViewId>,
    ) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<wgpu_core::id::TextureViewId>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, view: Resource<wgpu_core::id::TextureViewId>) -> wasmtime::Result<()> {
        let view_id = self.table.delete(view)?;
        self.instance.texture_view_drop(view_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuCommandBuffer for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuCommandBuffer>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuCommandBuffer>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, command_buffer: Resource<webgpu::GpuCommandBuffer>) -> wasmtime::Result<()> {
        let command_buffer_id = self.table.delete(command_buffer)?;
        self.instance.command_buffer_drop(command_buffer_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuShaderModule for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuShaderModule>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuShaderModule>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, shader: Resource<webgpu::GpuShaderModule>) -> wasmtime::Result<()> {
        let shader_id = self.table.delete(shader)?;
        self.instance.shader_module_drop(shader_id);
        Ok(())
    }
}

impl<T: Send> webgpu::HostGpuShaderModuleWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn get_compilation_info(
        _accessor: &Accessor<T, Self>,
        _shader: Resource<webgpu::GpuShaderModule>,
    ) -> wasmtime::Result<Resource<webgpu::GpuCompilationInfo>> {
        todo!()
    }
}

impl<'a> webgpu::HostGpuRenderPipeline for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<RenderPipeline>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<RenderPipeline>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn get_bind_group_layout(
        &mut self,
        pipeline: Resource<RenderPipeline>,
        index: u32,
    ) -> wasmtime::Result<Resource<webgpu::GpuBindGroupLayout>> {
        let pipeline = self.table.get(&pipeline)?;
        let pipeline_id = pipeline.render_pipeline_id;
        let error_handler = Arc::clone(&pipeline.error_handler);
        let (layout, err) =
            self.instance
                .render_pipeline_get_bind_group_layout(pipeline_id, index, None);
        error_handler.handle_possible_error(err);
        Ok(self.table.push(layout)?)
    }

    fn drop(&mut self, pipeline: Resource<webgpu::GpuRenderPipeline>) -> wasmtime::Result<()> {
        let pipeline = self.table.delete(pipeline)?;
        self.instance
            .render_pipeline_drop(pipeline.render_pipeline_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuAdapter for WasiWebGpuCtx<'a> {
    fn features(
        &mut self,
        adapter: Resource<webgpu::GpuAdapter>,
    ) -> wasmtime::Result<Resource<webgpu::GpuSupportedFeatures>> {
        let adapter = *(*self.table.get(&adapter)?);
        let features = self.instance.adapter_features(adapter);
        Ok(self.table.push(features)?)
    }

    fn limits(
        &mut self,
        adapter: Resource<webgpu::GpuAdapter>,
    ) -> wasmtime::Result<Resource<webgpu::GpuSupportedLimits>> {
        let adapter = *(*self.table.get(&adapter)?);
        let limits = self.instance.adapter_limits(adapter);
        Ok(self.table.push(limits)?)
    }

    fn info(
        &mut self,
        adapter: Resource<webgpu::GpuAdapter>,
    ) -> wasmtime::Result<Resource<webgpu::GpuAdapterInfo>> {
        let adapter_id = *(*self.table.get(&adapter)?);
        let info = self.instance.adapter_get_info(adapter_id);
        Ok(self.table.push(info)?)
    }

    fn drop(&mut self, adapter: Resource<webgpu::GpuAdapter>) -> wasmtime::Result<()> {
        let adapter_id = self.table.delete(adapter)?;
        if let Some(adapter_id) = Arc::into_inner(adapter_id) {
            self.instance.adapter_drop(adapter_id);
        }
        Ok(())
    }
}

impl<T: Send> webgpu::HostGpuAdapterWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn request_device(
        accessor: &Accessor<T, Self>,
        adapter: Resource<webgpu::GpuAdapter>,
        descriptor: Option<webgpu::GpuDeviceDescriptor>,
    ) -> wasmtime::Result<Result<Resource<webgpu::GpuDevice>, webgpu::RequestDeviceError>> {
        accessor.with(|mut access| {
            let ctx = access.get();
            let table = ctx.table;
            let adapter = Arc::clone(table.get(&adapter)?);

            let descriptor = match descriptor {
                Some(desc) => wgpu_types::DeviceDescriptor {
                    label: desc.label.map(|l| l.into()),
                    required_features: desc
                        .required_features
                        .map(|f| f.to_core(table))
                        .unwrap_or_default(),
                    required_limits: desc
                        .required_limits
                        .map(|limit| table.get(&limit).unwrap().to_core(table))
                        .unwrap_or(wgpu_types::Limits::defaults()),
                    // TODO: use descriptor.default_queue?
                    // trace is not present in WebGPU
                    trace: wgpu_types::Trace::default(),
                    // Don't enable any experimental features
                    experimental_features: wgpu_types::ExperimentalFeatures::disabled(),
                    // memory_hints is not present in WebGPU, comes from options
                    memory_hints: ctx.options.device_memory_hints.clone(),
                },
                None => wgpu_types::DeviceDescriptor::default(),
            };

            let device_queue_result =
                ctx.instance
                    .adapter_request_device(*adapter, &descriptor, None, None);

            Ok(match device_queue_result {
                Ok((device_id, queue_id)) => {
                    let device = table.push(Device {
                        handle: Arc::new(DeviceHandle::new(
                            Arc::clone(ctx.instance),
                            device_id,
                            queue_id,
                            ctx.options.wait_timeout,
                        )),
                        adapter,
                        error_handler: Arc::new(ErrorHandler::default()),
                    })?;
                    Ok(device)
                }

                Err(err) => {
                    let message = err.to_string();
                    // https://www.w3.org/TR/webgpu/#dom-gpuadapter-requestdevice
                    match err {
                        wgpu_core::instance::RequestDeviceError::UnsupportedFeature(_) => {
                            // From the spec:
                            // > 1. If any of the following requirements are unmet:
                            // >  - The set of values in descriptor.requiredFeatures must be a subset of those in adapter.[[features]].
                            // > Then issue the following steps on contentTimeline and return:
                            // >  1. Reject promise with a TypeError.
                            Err(webgpu::RequestDeviceError {
                                kind: webgpu::RequestDeviceErrorKind::TypeError,
                                message,
                            })
                        }
                        wgpu_core::instance::RequestDeviceError::LimitsExceeded(_) => {
                            // From the spec:
                            // > 2. All of the requirements in the following steps must be met.
                            // >  2. For each [key, value] in descriptor.requiredLimits for which value is not undefined:
                            // >   1. key must be the name of a member of supported limits.
                            // >   2. value must be no better than adapter.[[limits]][key].
                            // >   3. If key’s class is alignment, value must be a power of 2 less than 232.
                            // > 3. If any are unmet, issue the following steps on contentTimeline and return:
                            // >  1. Reject promise with an OperationError.
                            Err(webgpu::RequestDeviceError {
                                kind: webgpu::RequestDeviceErrorKind::OperationError,
                                message,
                            })
                        }
                        err => todo!("unhandled request device error: {:#?}", err),
                    }
                }
            })
        })
    }
}

impl<'a> webgpu::HostGpuQueue for WasiWebGpuCtx<'a> {
    fn submit(
        &mut self,
        queue: Resource<webgpu::GpuQueue>,
        val: Vec<Resource<webgpu::GpuCommandBuffer>>,
    ) -> wasmtime::Result<()> {
        let command_buffers = val
            .into_iter()
            .map(|buffer| Ok(*self.table.get(&buffer)?))
            .collect::<wasmtime::Result<Vec<_>>>()?;
        let queue = self.table.get(&queue)?;
        // A lost device refuses the submission, and generates no error for it.
        if let Err((_, err)) = self
            .instance
            .queue_submit(*queue.queue_id(), &command_buffers)
        {
            queue.error_handler.handle_possible_error(Some(err));
        }
        Ok(())
    }

    fn write_buffer_with_copy(
        &mut self,
        queue: Resource<webgpu::GpuQueue>,
        buffer: Resource<webgpu::GpuBuffer>,
        buffer_offset: webgpu::GpuSize64,
        data: Vec<u8>,
        data_offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<Result<(), webgpu::WriteBufferError>> {
        let queue = *self.table.get(&queue)?.queue_id();
        let buffer_id = self.table.get(&buffer)?.buffer_id;
        let mut data = &data[..];
        if let Some(data_offset) = data_offset {
            let data_offset = data_offset as usize;
            data = &data[data_offset..];
        }
        if let Some(size) = size {
            let size = size as usize;
            data = &data[..size];
        }
        self.instance
            .queue_write_buffer(queue, buffer_id, buffer_offset, data)?;
        Ok(Ok(()))
    }

    fn write_texture_with_copy(
        &mut self,
        queue: Resource<webgpu::GpuQueue>,
        destination: webgpu::GpuTexelCopyTextureInfo,
        data: Vec<u8>,
        data_layout: webgpu::GpuTexelCopyBufferLayout,
        size: webgpu::GpuExtent3D,
    ) -> wasmtime::Result<()> {
        let queue = *self.table.get(&queue)?.queue_id();
        self.instance.queue_write_texture(
            queue,
            &destination.to_core(self.table),
            &data,
            &data_layout.to_core(self.table),
            &size.to_core(self.table),
        )?;
        Ok(())
    }

    fn label(&mut self, _self_: Resource<webgpu::GpuQueue>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuQueue>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, queue: Resource<webgpu::GpuQueue>) -> wasmtime::Result<()> {
        self.table.delete(queue)?;
        Ok(())
    }
}

impl<T: Send> webgpu::HostGpuQueueWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn on_submitted_work_done(
        accessor: &Accessor<T, Self>,
        queue: Resource<webgpu::GpuQueue>,
    ) -> wasmtime::Result<()> {
        let handle = accessor.with(|mut access| -> wasmtime::Result<_> {
            Ok(Arc::clone(&access.get().table.get(&queue)?.handle))
        })?;
        if handle.on_submitted_work_done().await.is_err() {
            log::warn!("work-done callback dropped without running");
        }
        Ok(())
    }
}

impl<'a> webgpu::HostGpuCommandEncoder for WasiWebGpuCtx<'a> {
    fn begin_render_pass(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        descriptor: webgpu::GpuRenderPassDescriptor,
    ) -> wasmtime::Result<Resource<webgpu::GpuRenderPassEncoder>> {
        let command_encoder = self.table.get(&command_encoder)?;
        let command_encoder_id = command_encoder.command_encoder_id;
        let error_handler = Arc::clone(&command_encoder.error_handler);
        let timestamp_writes = descriptor.timestamp_writes.map(|tw| tw.to_core(self.table));
        // can't use to_core because depth_stencil_attachment is Option<&x>.
        let depth_stencil_attachment = descriptor
            .depth_stencil_attachment
            .map(|d| d.to_core(self.table));
        let descriptor = wgpu_core::command::RenderPassDescriptor {
            label: descriptor.label.map(|l| l.into()),
            color_attachments: descriptor
                .color_attachments
                .into_iter()
                .map(|c| c.map(|c| c.to_core(self.table)))
                .collect::<Vec<_>>()
                .into(),
            depth_stencil_attachment: depth_stencil_attachment.as_ref(),
            timestamp_writes: timestamp_writes.as_ref(),
            occlusion_query_set: descriptor
                .occlusion_query_set
                .map(|oqs| oqs.to_core(self.table)),
            // multiview_mask is not present in WebGPU
            multiview_mask: None,
            // TODO: self.max_draw_count not used
        };
        let (render_pass, err) = self
            .instance
            .command_encoder_begin_render_pass(command_encoder_id, &descriptor);

        error_handler.handle_possible_error(err);

        Ok(self.table.push(RenderPassEncoder::new(render_pass))?)
    }

    fn finish(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        descriptor: Option<webgpu::GpuCommandBufferDescriptor>,
    ) -> wasmtime::Result<Resource<webgpu::GpuCommandBuffer>> {
        let command_encoder = self.table.get(&command_encoder)?;
        let command_encoder_id = command_encoder.command_encoder_id;
        let error_handler = Arc::clone(&command_encoder.error_handler);
        let (command_buffer, err) = self.instance.command_encoder_finish(
            command_encoder_id,
            &descriptor
                .map(|d| d.to_core(self.table))
                .unwrap_or(wgpu_types::CommandBufferDescriptor::default()),
            None,
        );
        // dropping the label.
        // TODO: reconsider when implementing real labels.
        let err = err.map(|(_label, err)| err);
        error_handler.handle_possible_error(err);
        Ok(self.table.push(command_buffer)?)
    }

    fn begin_compute_pass(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        descriptor: Option<webgpu::GpuComputePassDescriptor>,
    ) -> wasmtime::Result<Resource<webgpu::GpuComputePassEncoder>> {
        let command_encoder = self.table.get(&command_encoder)?;
        let command_encoder_id = command_encoder.command_encoder_id;
        let error_handler = Arc::clone(&command_encoder.error_handler);
        let (compute_pass, err) = self.instance.command_encoder_begin_compute_pass(
            command_encoder_id,
            // can't use to_core because timestamp_writes is Option<&x>.
            &wgpu_core::command::ComputePassDescriptor {
                // TODO: can we get rid of the clone here?
                label: descriptor
                    .as_ref()
                    .and_then(|d| d.label.clone().map(|l| l.into())),
                timestamp_writes: descriptor
                    .and_then(|d| d.timestamp_writes.map(|tw| tw.to_core(self.table))),
            },
        );
        error_handler.handle_possible_error(err);
        Ok(self.table.push(ComputePassEncoder::new(compute_pass))?)
    }

    fn copy_buffer_to_buffer(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        source: Resource<webgpu::GpuBuffer>,
        source_offset: Option<webgpu::GpuSize64>,
        destination: Resource<webgpu::GpuBuffer>,
        destination_offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        let source = self.table.get(&source)?.buffer_id;
        let destination = self.table.get(&destination)?.buffer_id;
        // https://www.w3.org/TR/webgpu/#dom-gpucommandencoder-copybuffertobuffer
        // Note: wasi:webgpu uses `option` for offsets in lieu of the shorthand overload
        self.instance.command_encoder_copy_buffer_to_buffer(
            command_encoder,
            source,
            source_offset.unwrap_or(0),
            destination,
            destination_offset.unwrap_or(0),
            size,
        )?;
        Ok(())
    }

    fn copy_buffer_to_texture(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        source: webgpu::GpuTexelCopyBufferInfo,
        destination: webgpu::GpuTexelCopyTextureInfo,
        copy_size: webgpu::GpuExtent3D,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance.command_encoder_copy_buffer_to_texture(
            command_encoder,
            &source.to_core(self.table),
            &destination.to_core(self.table),
            &copy_size.to_core(self.table),
        )?;
        Ok(())
    }

    fn copy_texture_to_buffer(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        source: webgpu::GpuTexelCopyTextureInfo,
        destination: webgpu::GpuTexelCopyBufferInfo,
        copy_size: webgpu::GpuExtent3D,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance.command_encoder_copy_texture_to_buffer(
            command_encoder,
            &source.to_core(self.table),
            &destination.to_core(self.table),
            &copy_size.to_core(self.table),
        )?;
        Ok(())
    }

    fn copy_texture_to_texture(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        source: webgpu::GpuTexelCopyTextureInfo,
        destination: webgpu::GpuTexelCopyTextureInfo,
        copy_size: webgpu::GpuExtent3D,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance.command_encoder_copy_texture_to_texture(
            command_encoder,
            &source.to_core(self.table),
            &destination.to_core(self.table),
            &copy_size.to_core(self.table),
        )?;
        Ok(())
    }

    fn clear_buffer(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        buffer: Resource<webgpu::GpuBuffer>,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let buffer_id = self.table.get(&buffer)?.buffer_id;
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        // https://www.w3.org/TR/webgpu/#gpucommandencoder
        self.instance.command_encoder_clear_buffer(
            command_encoder,
            buffer_id,
            offset.unwrap_or(0),
            size,
        )?;
        Ok(())
    }

    fn resolve_query_set(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        query_set: Resource<webgpu::GpuQuerySet>,
        first_query: webgpu::GpuSize32,
        query_count: webgpu::GpuSize32,
        destination: Resource<webgpu::GpuBuffer>,
        destination_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let query_set_id = *self.table.get(&query_set)?;
        let destination = self.table.get(&destination)?.buffer_id;
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance.command_encoder_resolve_query_set(
            command_encoder,
            query_set_id,
            first_query,
            query_count,
            destination,
            destination_offset,
        )?;
        Ok(())
    }

    fn label(&mut self, command_encoder: Resource<CommandEncoder>) -> wasmtime::Result<String> {
        let _command_encoder = self.table.get(&command_encoder)?;
        // TODO: return real label
        Ok(String::new())
    }

    fn set_label(
        &mut self,
        _self_: Resource<CommandEncoder>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn push_debug_group(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        group_label: String,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance
            .command_encoder_push_debug_group(command_encoder, &group_label)?;
        Ok(())
    }

    fn pop_debug_group(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance
            .command_encoder_pop_debug_group(command_encoder)?;
        Ok(())
    }

    fn insert_debug_marker(
        &mut self,
        command_encoder: Resource<CommandEncoder>,
        marker_label: String,
    ) -> wasmtime::Result<()> {
        let command_encoder = self.table.get(&command_encoder)?.command_encoder_id;
        self.instance
            .command_encoder_insert_debug_marker(command_encoder, &marker_label)?;
        Ok(())
    }

    fn drop(&mut self, command_encoder: Resource<CommandEncoder>) -> wasmtime::Result<()> {
        let command_encoder = self.table.delete(command_encoder)?;
        self.instance
            .command_encoder_drop(command_encoder.command_encoder_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuRenderPassEncoder for WasiWebGpuCtx<'a> {
    fn set_pipeline(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        pipeline: Resource<webgpu::GpuRenderPipeline>,
    ) -> wasmtime::Result<()> {
        let pipeline_id = self.table.get(&pipeline)?.render_pipeline_id;
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_set_pipeline(render_pass, pipeline_id)?;
        Ok(())
    }

    fn draw(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        vertex_count: webgpu::GpuSize32,
        instance_count: Option<webgpu::GpuSize32>,
        first_vertex: Option<webgpu::GpuSize32>,
        first_instance: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
        self.instance.render_pass_draw(
            render_pass,
            vertex_count,
            instance_count.unwrap_or(1),
            first_vertex.unwrap_or(0),
            first_instance.unwrap_or(0),
        )?;
        Ok(())
    }

    fn end(&mut self, render_pass: Resource<RenderPassEncoder>) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let mut render_pass = render_pass.take().unwrap();
        self.instance.render_pass_end(&mut render_pass)?;
        Ok(())
    }

    fn set_viewport(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_set_viewport(
            render_pass,
            x,
            y,
            width,
            height,
            min_depth,
            max_depth,
        )?;
        Ok(())
    }

    fn set_scissor_rect(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        x: webgpu::GpuIntegerCoordinate,
        y: webgpu::GpuIntegerCoordinate,
        width: webgpu::GpuIntegerCoordinate,
        height: webgpu::GpuIntegerCoordinate,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_set_scissor_rect(render_pass, x, y, width, height)?;
        Ok(())
    }

    fn set_blend_constant(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        color: webgpu::GpuColor,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_set_blend_constant(render_pass, color.into())?;
        Ok(())
    }

    fn set_stencil_reference(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        reference: webgpu::GpuStencilValue,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_set_stencil_reference(render_pass, reference)?;
        Ok(())
    }

    fn begin_occlusion_query(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        query_index: webgpu::GpuSize32,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_begin_occlusion_query(render_pass, query_index)?;
        Ok(())
    }

    fn end_occlusion_query(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_end_occlusion_query(render_pass)?;
        Ok(())
    }

    fn execute_bundles(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        bundles: Vec<Resource<webgpu::GpuRenderBundle>>,
    ) -> wasmtime::Result<()> {
        let render_bundle_ids = bundles
            .iter()
            .map(|bundle| *self.table.get(bundle).unwrap())
            .collect::<Vec<_>>();
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_execute_bundles(render_pass, &render_bundle_ids)?;
        Ok(())
    }

    fn label(&mut self, _self_: Resource<RenderPassEncoder>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<RenderPassEncoder>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn push_debug_group(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        group_label: String,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_push_debug_group(render_pass, &group_label, 0)?;
        Ok(())
    }

    fn pop_debug_group(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_pop_debug_group(render_pass)?;
        Ok(())
    }

    fn insert_debug_marker(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        marker_label: String,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_insert_debug_marker(render_pass, &marker_label, 0)?;
        Ok(())
    }

    fn set_bind_group(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        index: webgpu::GpuIndex32,
        bind_group: Option<Resource<webgpu::GpuBindGroup>>,
        dynamic_offsets_data: Option<Vec<webgpu::GpuBufferDynamicOffset>>,
        dynamic_offsets_data_start: Option<webgpu::GpuSize64>,
        dynamic_offsets_data_length: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<Result<(), webgpu::SetBindGroupError>> {
        let bind_group = bind_group.map(|bind_group| *self.table.get(&bind_group).unwrap());
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#programmable-passes
        let dynamic_offsets = &dynamic_offsets_data.unwrap_or(vec![]);
        let mut dynamic_offsets = &dynamic_offsets[..];
        if let Some(dynamic_offsets_data_start) = dynamic_offsets_data_start {
            let dynamic_offsets_data_start = dynamic_offsets_data_start as usize;
            dynamic_offsets = &dynamic_offsets[dynamic_offsets_data_start..];
        }
        if let Some(dynamic_offsets_data_length) = dynamic_offsets_data_length {
            let dynamic_offsets_data_length = dynamic_offsets_data_length as usize;
            dynamic_offsets = &dynamic_offsets[..dynamic_offsets_data_length];
        }

        self.instance.render_pass_set_bind_group(
            render_pass,
            index,
            bind_group,
            dynamic_offsets,
        )?;
        Ok(Ok(()))
    }

    fn set_index_buffer(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        buffer: Resource<webgpu::GpuBuffer>,
        index_format: webgpu::GpuIndexFormat,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let buffer_id = self.table.get(&buffer)?.buffer_id;
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_set_index_buffer(
            render_pass,
            buffer_id,
            index_format.into(),
            // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
            offset.unwrap_or(0),
            size.map(|s| NonZeroU64::new(s).expect("Size can't be zero")),
        )?;
        Ok(())
    }

    fn set_vertex_buffer(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        slot: webgpu::GpuIndex32,
        buffer: Option<Resource<webgpu::GpuBuffer>>,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let buffer_id = self
            .table
            .get(&buffer.expect("TODO: deal null buffers"))?
            .buffer_id;
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_set_vertex_buffer(
            render_pass,
            slot,
            buffer_id,
            // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
            offset.unwrap_or(0),
            size.map(|s| NonZeroU64::new(s).expect("Size can't be zero")),
        )?;
        Ok(())
    }

    fn draw_indexed(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        index_count: webgpu::GpuSize32,
        instance_count: Option<webgpu::GpuSize32>,
        first_index: Option<webgpu::GpuSize32>,
        base_vertex: Option<webgpu::GpuSignedOffset32>,
        first_instance: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<()> {
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_draw_indexed(
            render_pass,
            index_count,
            // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
            instance_count.unwrap_or(1),
            first_index.unwrap_or(0),
            base_vertex.unwrap_or(0),
            first_instance.unwrap_or(0),
        )?;
        Ok(())
    }

    fn draw_indirect(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        indirect_buffer: Resource<webgpu::GpuBuffer>,
        indirect_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let indirect_buffer = self.table.get(&indirect_buffer)?.buffer_id;
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance
            .render_pass_draw_indirect(render_pass, indirect_buffer, indirect_offset)?;
        Ok(())
    }

    fn draw_indexed_indirect(
        &mut self,
        render_pass: Resource<RenderPassEncoder>,
        indirect_buffer: Resource<webgpu::GpuBuffer>,
        indirect_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let indirect_buffer = self.table.get(&indirect_buffer)?.buffer_id;
        let mut render_pass = self.table.get_mut(&render_pass)?.lock();
        let render_pass = render_pass.as_mut().unwrap();
        self.instance.render_pass_draw_indexed_indirect(
            render_pass,
            indirect_buffer,
            indirect_offset,
        )?;
        Ok(())
    }

    fn set_immediates(
        &mut self,
        _render_pass: Resource<webgpu::GpuRenderPassEncoder>,
        _range_offset: u32,
        _data: Vec<u8>,
        _data_offset: Option<u64>,
        _data_size: Option<u64>,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, render_pass: Resource<RenderPassEncoder>) -> wasmtime::Result<()> {
        self.table.delete(render_pass)?;
        Ok(())
    }
}

impl<'a> webgpu::HostGpuUncapturedErrorEvent for WasiWebGpuCtx<'a> {
    // fn new(
    //     &mut self,
    //     _type_: String,
    //     _gpu_uncaptured_error_event_init_dict: webgpu::GpuUncapturedErrorEventInit,
    // ) -> Resource<webgpu::GpuUncapturedErrorEvent> {
    //     todo!()
    // }

    fn error(
        &mut self,
        _self_: Resource<webgpu::GpuUncapturedErrorEvent>,
    ) -> wasmtime::Result<Resource<webgpu::GpuError>> {
        todo!()
    }

    fn drop(&mut self, _error: Resource<webgpu::GpuUncapturedErrorEvent>) -> wasmtime::Result<()> {
        todo!()
    }
}
// impl<'a> webgpu::HostGpuInternalError for WasiWebGpu<'a> {
//     fn new(&mut self, _message: String) -> Resource<webgpu::GpuInternalError> {
//         todo!()
//     }

//     fn message(&mut self, _self_: Resource<webgpu::GpuInternalError>) -> String {
//         todo!()
//     }

//     fn drop(&mut self, error: Resource<webgpu::GpuInternalError>) -> wasmtime::Result<()> {
//         self.table.delete(error)?;
//         Ok(())
//     }
// }
// impl<'a> webgpu::HostGpuOutOfMemoryError for WasiWebGpu<'a> {
//     fn new(&mut self, _message: String) -> Resource<webgpu::GpuOutOfMemoryError> {
//         todo!()
//     }

//     fn message(&mut self, _self_: Resource<webgpu::GpuOutOfMemoryError>) -> String {
//         todo!()
//     }

//     fn drop(&mut self, error: Resource<webgpu::GpuOutOfMemoryError>) -> wasmtime::Result<()> {
//         self.table.delete(error)?;
//         Ok(())
//     }
// }
// impl<'a> webgpu::HostGpuValidationError for WasiWebGpu<'a> {
//     fn new(&mut self, _message: String) -> Resource<webgpu::GpuValidationError> {
//         todo!()
//     }

//     fn message(&mut self, _self_: Resource<webgpu::GpuValidationError>) -> String {
//         todo!()
//     }

//     fn drop(&mut self, error: Resource<webgpu::GpuValidationError>) -> wasmtime::Result<()> {
//         self.table.delete(error)?;
//         Ok(())
//     }
// }
impl<'a> webgpu::HostGpuError for WasiWebGpuCtx<'a> {
    fn message(&mut self, error: Resource<webgpu::GpuError>) -> wasmtime::Result<String> {
        let error = self.table.get(&error)?;
        Ok(error.message.clone())
    }

    fn kind(
        &mut self,
        error: Resource<webgpu::GpuError>,
    ) -> wasmtime::Result<webgpu::GpuErrorKind> {
        let error = self.table.get(&error)?;
        Ok(error.kind)
    }

    fn drop(&mut self, _error: Resource<webgpu::GpuError>) -> wasmtime::Result<()> {
        self.table.delete(_error)?;
        Ok(())
    }
}
impl<'a> webgpu::HostGpuDeviceLostInfo for WasiWebGpuCtx<'a> {
    fn reason(
        &mut self,
        info: Resource<webgpu::GpuDeviceLostInfo>,
    ) -> wasmtime::Result<webgpu::GpuDeviceLostReason> {
        Ok(self.table.get(&info)?.reason)
    }

    fn message(&mut self, info: Resource<webgpu::GpuDeviceLostInfo>) -> wasmtime::Result<String> {
        Ok(self.table.get(&info)?.message.clone())
    }

    fn drop(&mut self, info: Resource<webgpu::GpuDeviceLostInfo>) -> wasmtime::Result<()> {
        self.table.delete(info)?;
        Ok(())
    }
}
impl<'a> webgpu::HostGpuCanvasContext for WasiWebGpuCtx<'a> {
    fn configure(
        &mut self,
        _self_: Resource<webgpu::GpuCanvasContext>,
        _configuration: webgpu::GpuCanvasConfiguration,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn get_configuration(
        &mut self,
        _self_: Resource<webgpu::GpuCanvasContext>,
    ) -> wasmtime::Result<Option<webgpu::GpuCanvasConfigurationOwned>> {
        todo!()
    }

    fn unconfigure(&mut self, _self_: Resource<webgpu::GpuCanvasContext>) -> wasmtime::Result<()> {
        todo!()
    }

    fn get_current_texture(
        &mut self,
        _self_: Resource<webgpu::GpuCanvasContext>,
    ) -> wasmtime::Result<Resource<webgpu::GpuTexture>> {
        todo!()
    }

    fn drop(&mut self, _rep: Resource<webgpu::GpuCanvasContext>) -> wasmtime::Result<()> {
        todo!()
    }
}
impl<'a> webgpu::HostGpuRenderBundle for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuRenderBundle>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuRenderBundle>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, _bundle: Resource<webgpu::GpuRenderBundle>) -> wasmtime::Result<()> {
        let bundle_id = self.table.delete(_bundle)?;
        self.instance.render_bundle_drop(bundle_id);
        Ok(())
    }
}
impl<'a> webgpu::HostGpuComputePassEncoder for WasiWebGpuCtx<'a> {
    fn set_pipeline(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        pipeline: Resource<webgpu::GpuComputePipeline>,
    ) -> wasmtime::Result<()> {
        let pipeline = self.table.get(&pipeline)?.compute_pipeline_id;
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        self.instance
            .compute_pass_set_pipeline(compute_pass, pipeline)?;
        Ok(())
    }

    fn dispatch_workgroups(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        workgroup_count_x: webgpu::GpuSize32,
        workgroup_count_y: Option<webgpu::GpuSize32>,
        workgroup_count_z: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<()> {
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpucomputepassencoder
        self.instance.compute_pass_dispatch_workgroups(
            compute_pass,
            workgroup_count_x,
            workgroup_count_y.unwrap_or(1),
            workgroup_count_z.unwrap_or(1),
        )?;
        Ok(())
    }

    fn dispatch_workgroups_indirect(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        indirect_buffer: Resource<webgpu::GpuBuffer>,
        indirect_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let indirect_buffer = self.table.get(&indirect_buffer)?.buffer_id;
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        self.instance.compute_pass_dispatch_workgroups_indirect(
            compute_pass,
            indirect_buffer,
            indirect_offset,
        )?;
        Ok(())
    }

    fn end(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
    ) -> wasmtime::Result<()> {
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let mut compute_pass = compute_pass.take().unwrap();
        self.instance.compute_pass_end(&mut compute_pass)?;
        Ok(())
    }

    fn label(
        &mut self,
        _self_: Resource<webgpu::GpuComputePassEncoder>,
    ) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuComputePassEncoder>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn push_debug_group(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        group_label: String,
    ) -> wasmtime::Result<()> {
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        self.instance
            .compute_pass_push_debug_group(compute_pass, &group_label, 0)?;
        Ok(())
    }

    fn pop_debug_group(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
    ) -> wasmtime::Result<()> {
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        self.instance.compute_pass_pop_debug_group(compute_pass)?;
        Ok(())
    }

    fn insert_debug_marker(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        label: String,
    ) -> wasmtime::Result<()> {
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        self.instance
            .compute_pass_insert_debug_marker(compute_pass, &label, 0)?;
        Ok(())
    }

    fn set_bind_group(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
        index: webgpu::GpuIndex32,
        bind_group: Option<Resource<webgpu::GpuBindGroup>>,
        dynamic_offsets_data: Option<Vec<webgpu::GpuBufferDynamicOffset>>,
        dynamic_offsets_data_start: Option<webgpu::GpuSize64>,
        dynamic_offsets_data_length: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<Result<(), webgpu::SetBindGroupError>> {
        let bind_group = bind_group.map(|bind_group| *self.table.get(&bind_group).unwrap());
        let mut compute_pass = self.table.get_mut(&compute_pass)?.lock();
        let compute_pass = compute_pass.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#programmable-passes

        let dynamic_offsets = &dynamic_offsets_data.unwrap_or(vec![]);
        let mut dynamic_offsets = &dynamic_offsets[..];
        if let Some(dynamic_offsets_data_start) = dynamic_offsets_data_start {
            let dynamic_offsets_data_start = dynamic_offsets_data_start as usize;
            dynamic_offsets = &dynamic_offsets[dynamic_offsets_data_start..];
        }
        if let Some(dynamic_offsets_data_length) = dynamic_offsets_data_length {
            let dynamic_offsets_data_length = dynamic_offsets_data_length as usize;
            dynamic_offsets = &dynamic_offsets[..dynamic_offsets_data_length];
        }

        self.instance.compute_pass_set_bind_group(
            compute_pass,
            index,
            bind_group,
            dynamic_offsets,
        )?;
        Ok(Ok(()))
    }

    fn set_immediates(
        &mut self,
        _render_pass: Resource<webgpu::GpuComputePassEncoder>,
        _range_offset: u32,
        _data: Vec<u8>,
        _data_offset: Option<u64>,
        _data_size: Option<u64>,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(
        &mut self,
        compute_pass: Resource<webgpu::GpuComputePassEncoder>,
    ) -> wasmtime::Result<()> {
        self.table.delete(compute_pass)?;
        Ok(())
    }
}
// impl<'a> webgpu::HostGpuPipelineError for WasiWebGpu<'a> {
//     fn new(
//         &mut self,
//         _message: Option<String>,
//         _options: webgpu::GpuPipelineErrorInit,
//     ) -> Resource<webgpu::GpuPipelineError> {
//         todo!()
//     }

//     fn reason(
//         &mut self,
//         _self_: Resource<webgpu::GpuPipelineError>,
//     ) -> webgpu::GpuPipelineErrorReason {
//         todo!()
//     }

//     fn drop(&mut self, error: Resource<webgpu::GpuPipelineError>) -> wasmtime::Result<()> {
//         self.table.delete(error)?;
//         Ok(())
//     }
// }
impl<'a> webgpu::HostGpuCompilationMessage for WasiWebGpuCtx<'a> {
    fn message(
        &mut self,
        _self_: Resource<webgpu::GpuCompilationMessage>,
    ) -> wasmtime::Result<String> {
        todo!()
    }

    fn type_(
        &mut self,
        _self_: Resource<webgpu::GpuCompilationMessage>,
    ) -> wasmtime::Result<webgpu::GpuCompilationMessageType> {
        todo!()
    }

    fn line_num(
        &mut self,
        _self_: Resource<webgpu::GpuCompilationMessage>,
    ) -> wasmtime::Result<u64> {
        todo!()
    }

    fn line_pos(
        &mut self,
        _self_: Resource<webgpu::GpuCompilationMessage>,
    ) -> wasmtime::Result<u64> {
        todo!()
    }

    fn offset(&mut self, _self_: Resource<webgpu::GpuCompilationMessage>) -> wasmtime::Result<u64> {
        todo!()
    }

    fn length(&mut self, _self_: Resource<webgpu::GpuCompilationMessage>) -> wasmtime::Result<u64> {
        todo!()
    }

    fn drop(&mut self, _cm: Resource<webgpu::GpuCompilationMessage>) -> wasmtime::Result<()> {
        todo!()
    }
}
impl<'a> webgpu::HostGpuCompilationInfo for WasiWebGpuCtx<'a> {
    fn messages(
        &mut self,
        _self_: Resource<webgpu::GpuCompilationInfo>,
    ) -> wasmtime::Result<Vec<Resource<webgpu::GpuCompilationMessage>>> {
        todo!()
    }

    fn drop(&mut self, _info: Resource<webgpu::GpuCompilationInfo>) -> wasmtime::Result<()> {
        todo!()
    }
}
impl<'a> webgpu::HostGpuQuerySet for WasiWebGpuCtx<'a> {
    fn destroy(&mut self, _self_: Resource<webgpu::GpuQuerySet>) -> wasmtime::Result<()> {
        // https://github.com/gfx-rs/wgpu/issues/6495
        todo!()
    }

    fn type_(
        &mut self,
        _self_: Resource<webgpu::GpuQuerySet>,
    ) -> wasmtime::Result<webgpu::GpuQueryType> {
        todo!()
    }

    fn count(
        &mut self,
        _self_: Resource<webgpu::GpuQuerySet>,
    ) -> wasmtime::Result<webgpu::GpuSize32Out> {
        todo!()
    }

    fn label(&mut self, _self_: Resource<webgpu::GpuQuerySet>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuQuerySet>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, _query_set: Resource<webgpu::GpuQuerySet>) -> wasmtime::Result<()> {
        let query_set_id = self.table.delete(_query_set)?;
        self.instance.query_set_drop(query_set_id);
        Ok(())
    }
}
impl<'a> webgpu::HostGpuRenderBundleEncoder for WasiWebGpuCtx<'a> {
    fn finish(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        descriptor: Option<webgpu::GpuRenderBundleDescriptor>,
    ) -> wasmtime::Result<Resource<webgpu::GpuRenderBundle>> {
        let descriptor = descriptor
            .map(|d| d.to_core(self.table))
            .unwrap_or(wgpu_types::RenderBundleDescriptor::default());
        let mut bundle_encoder_lock = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder_lock.take().unwrap();
        drop(bundle_encoder_lock);
        let (render_bundle, err) = self.instance.render_bundle_encoder_finish(
            bundle_encoder.render_bundle_encoder,
            &descriptor,
            None,
        );
        bundle_encoder.error_handler.handle_possible_error(err);
        Ok(self.table.push(render_bundle)?)
    }

    fn label(
        &mut self,
        _self_: Resource<webgpu::GpuRenderBundleEncoder>,
    ) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuRenderBundleEncoder>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn push_debug_group(
        &mut self,
        _bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        _group_label: String,
    ) -> wasmtime::Result<()> {
        todo!("Debug markers for RenderBundleEncoder not yet implemented in wgpu")
    }

    fn pop_debug_group(
        &mut self,
        _bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
    ) -> wasmtime::Result<()> {
        todo!("Debug markers for RenderBundleEncoder not yet implemented in wgpu")
    }

    fn insert_debug_marker(
        &mut self,
        _bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        _marker_label: String,
    ) -> wasmtime::Result<()> {
        todo!("Debug markers for RenderBundleEncoder not yet implemented in wgpu")
    }

    fn set_bind_group(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        index: webgpu::GpuIndex32,
        bind_group: Option<Resource<webgpu::GpuBindGroup>>,
        dynamic_offsets_data: Option<Vec<webgpu::GpuBufferDynamicOffset>>,
        dynamic_offsets_data_start: Option<webgpu::GpuSize64>,
        dynamic_offsets_data_length: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<Result<(), webgpu::SetBindGroupError>> {
        let bind_group_id = bind_group.map(|bind_group| *self.table.get(&bind_group).unwrap());
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();

        // https://www.w3.org/TR/webgpu/#programmable-passes
        let dynamic_offsets = &dynamic_offsets_data.unwrap_or(vec![]);
        let mut dynamic_offsets = &dynamic_offsets[..];
        if let Some(dynamic_offsets_data_start) = dynamic_offsets_data_start {
            let dynamic_offsets_data_start = dynamic_offsets_data_start as usize;
            dynamic_offsets = &dynamic_offsets[dynamic_offsets_data_start..];
        }
        if let Some(dynamic_offsets_data_length) = dynamic_offsets_data_length {
            let dynamic_offsets_data_length = dynamic_offsets_data_length as usize;
            dynamic_offsets = &dynamic_offsets[..dynamic_offsets_data_length];
        }

        unsafe {
            wgpu_core::command::bundle_ffi::wgpu_render_bundle_set_bind_group(
                &mut bundle_encoder.render_bundle_encoder,
                index,
                bind_group_id,
                dynamic_offsets.as_ptr(),
                dynamic_offsets.len(),
            )
        };
        Ok(Ok(()))
    }

    fn set_pipeline(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        pipeline: Resource<RenderPipeline>,
    ) -> wasmtime::Result<()> {
        let pipeline = self.table.get(&pipeline)?;
        let pipeline_id = pipeline.render_pipeline_id;
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_set_pipeline(
            &mut bundle_encoder.render_bundle_encoder,
            pipeline_id,
        );
        Ok(())
    }

    fn set_index_buffer(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        buffer: Resource<webgpu::GpuBuffer>,
        index_format: webgpu::GpuIndexFormat,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let buffer_id = self.table.get(&buffer)?.buffer_id;
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_set_index_buffer(
            &mut bundle_encoder.render_bundle_encoder,
            buffer_id,
            index_format.into(),
            offset.unwrap_or(0),
            size.map(|s| NonZeroU64::new(s).expect("Size can't be zero")),
        );
        Ok(())
    }

    fn set_vertex_buffer(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        slot: webgpu::GpuIndex32,
        buffer: Option<Resource<webgpu::GpuBuffer>>,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<()> {
        let buffer = buffer.expect("TODO: Null buffers not yet supported in wgpu");
        let buffer_id = self.table.get(&buffer)?.buffer_id;
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_set_vertex_buffer(
            &mut bundle_encoder.render_bundle_encoder,
            slot,
            buffer_id,
            offset.unwrap_or(0),
            size.map(|s| NonZeroU64::new(s).expect("Size can't be zero")),
        );
        Ok(())
    }

    fn draw(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        vertex_count: webgpu::GpuSize32,
        instance_count: Option<webgpu::GpuSize32>,
        first_vertex: Option<webgpu::GpuSize32>,
        first_instance: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<()> {
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_draw(
            &mut bundle_encoder.render_bundle_encoder,
            vertex_count,
            instance_count.unwrap_or(1),
            first_vertex.unwrap_or(0),
            first_instance.unwrap_or(0),
        );
        Ok(())
    }

    fn draw_indexed(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        index_count: webgpu::GpuSize32,
        instance_count: Option<webgpu::GpuSize32>,
        first_index: Option<webgpu::GpuSize32>,
        base_vertex: Option<webgpu::GpuSignedOffset32>,
        first_instance: Option<webgpu::GpuSize32>,
    ) -> wasmtime::Result<()> {
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        // https://www.w3.org/TR/webgpu/#gpurendercommandsmixin
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_draw_indexed(
            &mut bundle_encoder.render_bundle_encoder,
            index_count,
            instance_count.unwrap_or(1),
            first_index.unwrap_or(0),
            base_vertex.unwrap_or(0),
            first_instance.unwrap_or(0),
        );
        Ok(())
    }

    fn draw_indirect(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        indirect_buffer: Resource<webgpu::GpuBuffer>,
        indirect_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let indirect_buffer = self.table.get(&indirect_buffer)?.buffer_id;
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_draw_indirect(
            &mut bundle_encoder.render_bundle_encoder,
            indirect_buffer,
            indirect_offset,
        );
        Ok(())
    }

    fn draw_indexed_indirect(
        &mut self,
        bundle_encoder: Resource<webgpu::GpuRenderBundleEncoder>,
        indirect_buffer: Resource<webgpu::GpuBuffer>,
        indirect_offset: webgpu::GpuSize64,
    ) -> wasmtime::Result<()> {
        let indirect_buffer = self.table.get(&indirect_buffer)?.buffer_id;
        let mut bundle_encoder = self.table.get_mut(&bundle_encoder)?.lock();
        let bundle_encoder = bundle_encoder.as_mut().unwrap();
        wgpu_core::command::bundle_ffi::wgpu_render_bundle_draw_indexed_indirect(
            &mut bundle_encoder.render_bundle_encoder,
            indirect_buffer,
            indirect_offset,
        );
        Ok(())
    }

    fn set_immediates(
        &mut self,
        _render_pass: Resource<webgpu::GpuRenderBundleEncoder>,
        _range_offset: u32,
        _data: Vec<u8>,
        _data_offset: Option<u64>,
        _data_size: Option<u64>,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, encoder: Resource<webgpu::GpuRenderBundleEncoder>) -> wasmtime::Result<()> {
        self.table.delete(encoder)?;
        Ok(())
    }
}
impl<'a> webgpu::HostGpuComputePipeline for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuComputePipeline>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuComputePipeline>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn get_bind_group_layout(
        &mut self,
        compute_pipeline: Resource<webgpu::GpuComputePipeline>,
        index: u32,
    ) -> wasmtime::Result<Resource<webgpu::GpuBindGroupLayout>> {
        let pipeline = self.table.get(&compute_pipeline)?;
        let pipeline_id = pipeline.compute_pipeline_id;
        let error_handler = Arc::clone(&pipeline.error_handler);
        let (bind_group_layout, err) =
            self.instance
                .compute_pipeline_get_bind_group_layout(pipeline_id, index, None);
        error_handler.handle_possible_error(err);
        Ok(self.table.push(bind_group_layout)?)
    }

    fn drop(&mut self, pipeline: Resource<webgpu::GpuComputePipeline>) -> wasmtime::Result<()> {
        let pipeline = self.table.delete(pipeline)?;
        self.instance
            .compute_pipeline_drop(pipeline.compute_pipeline_id);
        Ok(())
    }
}
impl<'a> webgpu::HostGpuBindGroup for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuBindGroup>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuBindGroup>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, bind_group: Resource<webgpu::GpuBindGroup>) -> wasmtime::Result<()> {
        let bind_group_id = self.table.delete(bind_group)?;
        self.instance.bind_group_drop(bind_group_id);
        Ok(())
    }
}
impl<'a> webgpu::HostGpuPipelineLayout for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuPipelineLayout>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuPipelineLayout>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, layout: Resource<webgpu::GpuPipelineLayout>) -> wasmtime::Result<()> {
        let layout_id = self.table.delete(layout)?;
        self.instance.pipeline_layout_drop(layout_id);
        Ok(())
    }
}
impl<'a> webgpu::HostGpuBindGroupLayout for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuBindGroupLayout>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuBindGroupLayout>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, layout: Resource<webgpu::GpuBindGroupLayout>) -> wasmtime::Result<()> {
        let layout_id = self.table.delete(layout)?;
        self.instance.bind_group_layout_drop(layout_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuSampler for WasiWebGpuCtx<'a> {
    fn label(&mut self, _self_: Resource<webgpu::GpuSampler>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuSampler>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, sampler: Resource<webgpu::GpuSampler>) -> wasmtime::Result<()> {
        let sampler_id = self.table.delete(sampler)?;
        self.instance.sampler_drop(sampler_id);
        Ok(())
    }
}

impl<'a> webgpu::HostGpuBuffer for WasiWebGpuCtx<'a> {
    fn size(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
    ) -> wasmtime::Result<webgpu::GpuSize64Out> {
        let buffer = self.table.get(&buffer)?;
        Ok(buffer.size)
    }

    fn usage(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
    ) -> wasmtime::Result<webgpu::GpuBufferUsage> {
        let buffer = self.table.get(&buffer)?;
        Ok(buffer.usage.try_into().unwrap())
    }

    fn map_state(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
    ) -> wasmtime::Result<webgpu::GpuBufferMapState> {
        let buffer = self.table.get(&buffer)?;
        Ok(buffer.map_state())
    }

    fn get_mapped_range_get_with_copy(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<Result<Vec<u8>, webgpu::GetMappedRangeError>> {
        let buffer = self.table.get(&buffer)?;
        if buffer.map_state() != webgpu::GpuBufferMapState::Mapped {
            todo!("Throw buffer not mapped error");
        }
        let buffer_id = buffer.buffer_id;
        let (ptr, len) = self
            .instance
            // https://www.w3.org/TR/webgpu/#gpubuffer
            .buffer_get_mapped_range(buffer_id, offset.unwrap_or(0), size)?;
        let data = unsafe { slice::from_raw_parts(ptr.as_ptr(), len as usize) };
        let mut output = vec![0; len as usize];
        output.copy_from_slice(data);
        Ok(Ok(output))
    }

    fn get_mapped_range_set_with_copy(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
        data: Vec<u8>,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<Result<(), webgpu::GetMappedRangeError>> {
        let buffer = self.table.get(&buffer)?;
        if buffer.map_state() != webgpu::GpuBufferMapState::Mapped {
            todo!("Throw buffer not mapped error");
        }
        let buffer_id = buffer.buffer_id;
        let (ptr, len) = self
            .instance
            // https://www.w3.org/TR/webgpu/#gpubuffer
            .buffer_get_mapped_range(buffer_id, offset.unwrap_or(0), size)?;
        let buffer = unsafe { slice::from_raw_parts_mut(ptr.as_ptr(), len as usize) };
        buffer.copy_from_slice(&data);
        Ok(Ok(()))
    }

    fn unmap(
        &mut self,
        buffer: Resource<webgpu::GpuBuffer>,
    ) -> wasmtime::Result<Result<(), webgpu::UnmapError>> {
        let buffer = self.table.get(&buffer)?;
        self.instance.buffer_unmap(buffer.buffer_id)?;
        buffer.set_map_state(webgpu::GpuBufferMapState::Unmapped);
        Ok(Ok(()))
    }

    fn destroy(&mut self, buffer: Resource<webgpu::GpuBuffer>) -> wasmtime::Result<()> {
        let buffer_id = self.table.get_mut(&buffer)?.buffer_id;
        self.instance.buffer_destroy(buffer_id);
        Ok(())
    }

    fn label(&mut self, _self_: Resource<webgpu::GpuBuffer>) -> wasmtime::Result<String> {
        todo!()
    }

    fn set_label(
        &mut self,
        _self_: Resource<webgpu::GpuBuffer>,
        _label: String,
    ) -> wasmtime::Result<()> {
        todo!()
    }

    fn drop(&mut self, buffer: Resource<webgpu::GpuBuffer>) -> wasmtime::Result<()> {
        let buffer = self.table.delete(buffer)?;
        self.instance.buffer_drop(buffer.buffer_id);
        Ok(())
    }
}

impl<T: Send> webgpu::HostGpuBufferWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn map_async(
        accessor: &Accessor<T, Self>,
        buffer: Resource<webgpu::GpuBuffer>,
        mode: webgpu::GpuMapMode,
        offset: Option<webgpu::GpuSize64>,
        size: Option<webgpu::GpuSize64>,
    ) -> wasmtime::Result<Result<(), webgpu::MapAsyncError>> {
        // source: https://www.w3.org/TR/webgpu/#typedefdef-gpumapmodeflags
        // from the spec
        // > 3. If any of the following conditions are unsatisfied:
        // >     - mode contains exactly one of READ or WRITE.
        // >   Then:
        // >     3. Generate a validation error.

        let mode = if mode == webgpu::GpuMapMode::READ {
            wgpu_core::device::HostMap::Read
        } else if mode == webgpu::GpuMapMode::WRITE {
            wgpu_core::device::HostMap::Write
        } else {
            // > 4. Reject promise with an OperationError.
            return Ok(Err(webgpu::MapAsyncError {
                kind: webgpu::MapAsyncErrorKind::OperationError,
                message: "map mode must be exactly one of READ or WRITE".into(),
            }));
        };

        // https://www.w3.org/TR/webgpu/#gpubuffer
        let offset = offset.unwrap_or(0);

        let mapping = accessor.with(|mut access| -> wasmtime::Result<_> {
            let buffer = access.get().table.get(&buffer)?;
            let mut map_state = buffer.map_state.lock().unwrap();
            // > If this.[[mapping]] is not null or this.[[pending_map]] is not
            // > null, reject promise with an OperationError.
            // A rejected map leaves the earlier map's state alone.
            if *map_state != webgpu::GpuBufferMapState::Unmapped {
                return Ok(Err(webgpu::MapAsyncError {
                    kind: webgpu::MapAsyncErrorKind::OperationError,
                    message: "buffer is already mapped or has a pending map".into(),
                }));
            }
            *map_state = webgpu::GpuBufferMapState::Pending;
            Ok(Ok((
                Arc::clone(&buffer.device),
                buffer.buffer_id,
                Arc::clone(&buffer.map_state),
            )))
        })?;
        let (handle, buffer_id, state) = match mapping {
            Ok(mapping) => mapping,
            Err(err) => return Ok(Err(err)),
        };

        // The map settles the buffer's state, so it stays right if the guest
        // stops waiting.
        let mapped = handle.map_async(buffer_id, mode, offset, size, Arc::clone(&state));
        match mapped.await {
            Ok(mapped) => Ok(mapped),
            Err(_) => {
                *state.lock().unwrap() = webgpu::GpuBufferMapState::Unmapped;
                Ok(Err(aborted("the map callback was dropped")))
            }
        }
    }
}

impl<'a> webgpu::HostGpu for WasiWebGpuCtx<'a> {
    fn get_preferred_canvas_format(
        &mut self,
        _gpu: Resource<webgpu::Gpu>,
    ) -> wasmtime::Result<webgpu::GpuTextureFormat> {
        Ok(PREFERRED_CANVAS_FORMAT)
    }

    fn wgsl_language_features(
        &mut self,
        _self_: Resource<webgpu::Gpu>,
    ) -> wasmtime::Result<Resource<webgpu::WgslLanguageFeatures>> {
        Ok(self.table.push(WgslLanguageFeatures::new())?)
    }

    fn drop(&mut self, _gpu: Resource<webgpu::Gpu>) -> wasmtime::Result<()> {
        // not actually a resource in the table
        Ok(())
    }
}

impl<T: Send> webgpu::HostGpuWithStore<T> for crate::HasWasiWebGpuCtx {
    async fn request_adapter(
        accessor: &Accessor<T, Self>,
        _gpu: Resource<webgpu::Gpu>,
        options: Option<webgpu::GpuRequestAdapterOptions>,
    ) -> wasmtime::Result<Option<Resource<webgpu::GpuAdapter>>> {
        accessor.with(|mut access: Access<'_, T, crate::HasWasiWebGpuCtx>| {
            let ctx = access.get();
            let adapter = ctx.instance.request_adapter(
                &options
                    .map(|o| o.to_core(ctx.table))
                    .unwrap_or(wgpu_types::RequestAdapterOptions::default()),
                wgpu_types::Backends::all(),
                None,
            );
            Ok(match adapter {
                Ok(adapter) => {
                    let adapter = Arc::new(adapter);
                    let adapter = ctx.table.push(adapter).unwrap();
                    Some(adapter)
                }
                Err(wgpu_types::RequestAdapterError::NotFound { .. }) => {
                    log::warn!("GPU adapter not found");
                    None
                }
                Err(e) => {
                    log::warn!("Failed to get gpu adapter: {e:?}");
                    panic!("Error when trying to get GPU adapter");
                }
            })
        })
    }
}

impl<'a> webgpu::HostGpuAdapterInfo for WasiWebGpuCtx<'a> {
    // TODO: more real values here
    // take ideas from https://bugzilla.mozilla.org/show_bug.cgi?id=1831994
    // keep an eye on https://github.com/gfx-rs/wgpu/issues/8649
    fn vendor(
        &mut self,
        adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<String> {
        let adapter_info = self.table.get(&adapter_info)?;
        Ok(adapter_info.vendor.to_string())
    }

    fn architecture(
        &mut self,
        _adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<String> {
        // TODO: implement real architecture
        Ok(String::new())
    }

    fn device(
        &mut self,
        adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<String> {
        let adapter_info = self.table.get(&adapter_info)?;
        Ok(adapter_info.device.to_string())
    }

    fn description(
        &mut self,
        _adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<String> {
        // TODO: implement real description
        Ok(String::new())
    }

    fn subgroup_min_size(
        &mut self,
        adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<u32> {
        let adapter_info = self.table.get(&adapter_info)?;
        Ok(adapter_info.subgroup_min_size)
    }

    fn subgroup_max_size(
        &mut self,
        adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<u32> {
        let adapter_info = self.table.get(&adapter_info)?;
        Ok(adapter_info.subgroup_max_size)
    }

    fn is_fallback_adapter(
        &mut self,
        adapter_info: Resource<webgpu::GpuAdapterInfo>,
    ) -> wasmtime::Result<bool> {
        let adapter_info = self.table.get(&adapter_info)?;
        // wgpu in browser treats only cpu as fallback
        // https://github.com/gfx-rs/wgpu/blob/0d32f7e75604feeff976445576c234da377fa3df/wgpu/src/backend/webgpu.rs#L889-L893
        let is_fallback = match adapter_info.device_type {
            wgpu_types::DeviceType::IntegratedGpu
            | wgpu_types::DeviceType::DiscreteGpu
            | wgpu_types::DeviceType::VirtualGpu
            | wgpu_types::DeviceType::Other => false,
            wgpu_types::DeviceType::Cpu => true,
        };
        Ok(is_fallback)
    }

    fn drop(&mut self, info: Resource<webgpu::GpuAdapterInfo>) -> wasmtime::Result<()> {
        self.table.delete(info)?;
        Ok(())
    }
}
impl<'a> webgpu::HostWgslLanguageFeatures for WasiWebGpuCtx<'a> {
    fn has(
        &mut self,
        features: Resource<webgpu::WgslLanguageFeatures>,
        key: String,
    ) -> wasmtime::Result<bool> {
        let features = self.table.get(&features)?;
        Ok(features.has(&key))
    }

    fn drop(&mut self, features: Resource<webgpu::WgslLanguageFeatures>) -> wasmtime::Result<()> {
        self.table.delete(features)?;
        Ok(())
    }
}
impl<'a> webgpu::HostGpuSupportedFeatures for WasiWebGpuCtx<'a> {
    fn has(
        &mut self,
        features: Resource<webgpu::GpuSupportedFeatures>,
        query: String,
    ) -> wasmtime::Result<bool> {
        let features = self.table.get(&features)?;
        // TODO: disable the ones not present in the wgpu yet.
        Ok(match query.as_str() {
            "core-features-and-limits" => {
                // TODO: enable once wgpu does
                // features.contains(wgpu_types::Features::CORE_FEATURES_AND_LIMITS)
                false
            }
            "depth-clip-control" => features.contains(wgpu_types::Features::DEPTH_CLIP_CONTROL),
            "depth32float-stencil8" => {
                features.contains(wgpu_types::Features::DEPTH32FLOAT_STENCIL8)
            }
            "texture-compression-bc" => {
                features.contains(wgpu_types::Features::TEXTURE_COMPRESSION_BC)
            }
            "texture-compression-bc-sliced-3d" => {
                features.contains(wgpu_types::Features::TEXTURE_COMPRESSION_BC_SLICED_3D)
            }
            "texture-compression-etc2" => {
                features.contains(wgpu_types::Features::TEXTURE_COMPRESSION_ETC2)
            }
            "texture-compression-astc" => {
                features.contains(wgpu_types::Features::TEXTURE_COMPRESSION_ASTC)
            }
            "texture-compression-astc-sliced-3d" => {
                features.contains(wgpu_types::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D)
            }
            "timestamp-query" => features.contains(wgpu_types::Features::TIMESTAMP_QUERY),
            "indirect-first-instance" => {
                features.contains(wgpu_types::Features::INDIRECT_FIRST_INSTANCE)
            }
            "shader-f16" => features.contains(wgpu_types::Features::SHADER_F16),
            "rg11b10ufloat-renderable" => {
                features.contains(wgpu_types::Features::RG11B10UFLOAT_RENDERABLE)
            }
            "bgra8unorm-storage" => features.contains(wgpu_types::Features::BGRA8UNORM_STORAGE),
            "float32-filterable" => features.contains(wgpu_types::Features::FLOAT32_FILTERABLE),
            "float32-blendable" => features.contains(wgpu_types::Features::FLOAT32_BLENDABLE),
            "clip-distances" => features.contains(wgpu_types::Features::CLIP_DISTANCES),
            "dual-source-blending" => features.contains(wgpu_types::Features::DUAL_SOURCE_BLENDING),
            "subgroups" => {
                // TODO: enable once wgpu does
                // features.contains(wgpu_types::Features::SUBGROUPS)
                false
            }
            "texture-formats-tier1" => {
                // TODO: enable once wgpu does
                // features.contains(wgpu_types::Features::TEXTURE_FORMATS_TIER1)
                false
            }
            "texture-formats-tier2" => {
                // TODO: enable once wgpu does
                // features.contains(wgpu_types::Features::TEXTURE_FORMATS_TIER2)
                false
            }
            "primitive-index" => features.contains(wgpu_types::Features::PRIMITIVE_INDEX),
            "texture-component-swizzle" => {
                // TODO: enable once wgpu does
                // features.contains(wgpu_types::Features::TEXTURE_COMPONENT_SWIZZLE)
                false
            }
            name => {
                log::warn!("unknown feature name: {}", name);
                false
            }
        })
    }

    fn drop(&mut self, features: Resource<webgpu::GpuSupportedFeatures>) -> wasmtime::Result<()> {
        self.table.delete(features)?;
        Ok(())
    }
}
impl<'a> webgpu::HostGpuSupportedLimits for WasiWebGpuCtx<'a> {
    fn max_texture_dimension1_d(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_texture_dimension_1d)
    }

    fn max_texture_dimension2_d(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_texture_dimension_2d)
    }

    fn max_texture_dimension3_d(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_texture_dimension_3d)
    }

    fn max_texture_array_layers(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_texture_array_layers)
    }

    fn max_bind_groups(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_bind_groups)
    }

    fn max_bind_groups_plus_vertex_buffers(
        &mut self,
        _limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        // Not present in wgpu yet so rely on spec default
        // https://www.w3.org/TR/webgpu/#dom-supported-limits-maxbindgroupsplusvertexbuffers
        // TODO: take value from wgpu once implemented there
        Ok(24)
    }

    fn max_bindings_per_bind_group(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_bindings_per_bind_group)
    }

    fn max_dynamic_uniform_buffers_per_pipeline_layout(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_dynamic_uniform_buffers_per_pipeline_layout)
    }

    fn max_dynamic_storage_buffers_per_pipeline_layout(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_dynamic_storage_buffers_per_pipeline_layout)
    }

    fn max_sampled_textures_per_shader_stage(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_sampled_textures_per_shader_stage)
    }

    fn max_samplers_per_shader_stage(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_samplers_per_shader_stage)
    }

    fn max_storage_buffers_per_shader_stage(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_storage_buffers_per_shader_stage)
    }

    fn max_storage_textures_per_shader_stage(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_storage_textures_per_shader_stage)
    }

    fn max_uniform_buffers_per_shader_stage(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_uniform_buffers_per_shader_stage)
    }

    fn max_uniform_buffer_binding_size(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u64> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_uniform_buffer_binding_size)
    }

    fn max_storage_buffer_binding_size(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u64> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_storage_buffer_binding_size)
    }

    fn min_uniform_buffer_offset_alignment(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.min_uniform_buffer_offset_alignment)
    }

    fn min_storage_buffer_offset_alignment(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.min_storage_buffer_offset_alignment)
    }

    fn max_vertex_buffers(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_vertex_buffers)
    }

    fn max_buffer_size(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u64> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_buffer_size)
    }

    fn max_vertex_attributes(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_vertex_attributes)
    }

    fn max_vertex_buffer_array_stride(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_vertex_buffer_array_stride)
    }

    fn max_inter_stage_shader_variables(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_inter_stage_shader_variables)
    }

    fn max_color_attachments(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_color_attachments)
    }

    fn max_color_attachment_bytes_per_sample(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_color_attachment_bytes_per_sample)
    }

    fn max_compute_workgroup_storage_size(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_workgroup_storage_size)
    }

    fn max_compute_invocations_per_workgroup(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_invocations_per_workgroup)
    }

    fn max_compute_workgroup_size_x(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_workgroup_size_x)
    }

    fn max_compute_workgroup_size_y(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_workgroup_size_y)
    }

    fn max_compute_workgroup_size_z(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_workgroup_size_z)
    }

    fn max_compute_workgroups_per_dimension(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_compute_workgroups_per_dimension)
    }

    fn max_immediate_size(
        &mut self,
        limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        let limits = self.table.get(&limits)?;
        Ok(limits.max_immediate_size)
    }

    fn max_storage_buffers_in_vertex_stage(
        &mut self,
        _limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        // let limits = self.table().get(&limits)?;
        // Ok(limits.max_storage_buffers_in_vertex_stage)
        todo!()
    }

    fn max_storage_buffers_in_fragment_stage(
        &mut self,
        _limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        // let limits = self.table().get(&limits)?;
        // Ok(limits.max_storage_buffers_in_fragment_stage)
        todo!()
    }

    fn max_storage_textures_in_vertex_stage(
        &mut self,
        _limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        // let limits = self.table().get(&limits)?;
        // Ok(limits.max_storage_textures_in_vertex_stage)
        todo!()
    }

    fn max_storage_textures_in_fragment_stage(
        &mut self,
        _limits: Resource<webgpu::GpuSupportedLimits>,
    ) -> wasmtime::Result<u32> {
        // let limits = self.table().get(&limits)?;
        // Ok(limits.max_storage_textures_in_fragment_stage)
        todo!()
    }

    fn drop(&mut self, limits: Resource<webgpu::GpuSupportedLimits>) -> wasmtime::Result<()> {
        self.table.delete(limits)?;
        Ok(())
    }
}
