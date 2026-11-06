//! wgpu-core objects owned by the resource table.

use std::{fmt, ops::Deref, sync::Arc};

use wgpu_core::{global::Global, id};

/// A wgpu-core object that a resource table entry owns. Dropping it releases
/// the object, so a store torn down while the guest still holds handles frees
/// them too. Each object must have exactly one owner: wgpu-core panics on a
/// second release.
pub struct Owned<I: Release> {
    id: I,
    instance: Arc<Global>,
}

impl<I: Release> Owned<I> {
    pub(crate) fn new(instance: &Arc<Global>, id: I) -> Self {
        Self {
            id,
            instance: Arc::clone(instance),
        }
    }
}

impl<I: Release> Deref for Owned<I> {
    type Target = I;

    fn deref(&self) -> &I {
        &self.id
    }
}

impl<I: Release> Drop for Owned<I> {
    fn drop(&mut self) {
        self.id.release(&self.instance);
    }
}

impl<I: Release + fmt::Debug> fmt::Debug for Owned<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.id.fmt(f)
    }
}

/// Removes a wgpu-core object from the instance's hub.
pub trait Release: Copy {
    fn release(self, instance: &Global);
}

macro_rules! release {
    ($($id:ty => $drop:ident,)*) => {$(
        impl Release for $id {
            fn release(self, instance: &Global) {
                instance.$drop(self);
            }
        }
    )*};
}

release! {
    id::AdapterId => adapter_drop,
    id::BindGroupId => bind_group_drop,
    id::BindGroupLayoutId => bind_group_layout_drop,
    id::BufferId => buffer_drop,
    id::CommandBufferId => command_buffer_drop,
    id::CommandEncoderId => command_encoder_drop,
    id::ComputePipelineId => compute_pipeline_drop,
    id::PipelineLayoutId => pipeline_layout_drop,
    id::QuerySetId => query_set_drop,
    id::RenderBundleId => render_bundle_drop,
    id::RenderPipelineId => render_pipeline_drop,
    id::SamplerId => sampler_drop,
    id::ShaderModuleId => shader_module_drop,
    id::TextureId => texture_drop,
    id::TextureViewId => texture_view_drop,
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use wasmtime::component::ResourceTable;
    use wgpu_core::{binding_model, pipeline};

    use super::*;
    use crate::{device_handle::tests::noop_device, types::Queue};

    /// A store torn down without the guest dropping anything releases every
    /// object, so its device can be destroyed.
    #[test]
    fn dropping_the_table_releases_every_object() {
        let handle = noop_device();
        let instance = Arc::clone(handle.instance());
        let device = *handle.device_id();
        let mut table = ResourceTable::new();

        let (buffer, err) = instance.device_create_buffer(
            device,
            &wgpu_types::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu_types::BufferUsages::STORAGE,
                mapped_at_creation: false,
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (module, err) = instance.device_create_shader_module(
            device,
            &pipeline::ShaderModuleDescriptor {
                label: None,
                runtime_checks: Default::default(),
            },
            pipeline::ShaderModuleSource::Wgsl(Cow::Borrowed(
                "@compute @workgroup_size(1) fn main() {}",
            )),
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (bind_group_layout, err) = instance.device_create_bind_group_layout(
            device,
            &binding_model::BindGroupLayoutDescriptor {
                label: None,
                entries: Cow::Borrowed(&[]),
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (pipeline_layout, err) = instance.device_create_pipeline_layout(
            device,
            &binding_model::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: Cow::Owned(vec![Some(bind_group_layout)]),
                immediate_size: 0,
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (compute_pipeline, err) = instance.device_create_compute_pipeline(
            device,
            &pipeline::ComputePipelineDescriptor {
                label: None,
                layout: Some(pipeline_layout),
                stage: pipeline::ProgrammableStageDescriptor {
                    module,
                    entry_point: Some(Cow::Borrowed("main")),
                    constants: Default::default(),
                    zero_initialize_workgroup_memory: true,
                },
                cache: None,
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (bind_group, err) = instance.device_create_bind_group(
            device,
            &binding_model::BindGroupDescriptor {
                label: None,
                layout: bind_group_layout,
                entries: Cow::Borrowed(&[]),
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (encoder, err) = instance.device_create_command_encoder(
            device,
            &wgpu_types::CommandEncoderDescriptor::default(),
            None,
        );
        assert!(err.is_none(), "{err:?}");
        let (command_buffer, err) = instance.command_encoder_finish(
            encoder,
            &wgpu_types::CommandBufferDescriptor::default(),
            None,
        );
        assert!(err.is_none(), "{err:?}");

        table.push(Owned::new(&instance, buffer)).unwrap();
        table.push(Owned::new(&instance, module)).unwrap();
        table
            .push(Owned::new(&instance, bind_group_layout))
            .unwrap();
        table.push(Owned::new(&instance, pipeline_layout)).unwrap();
        table.push(Owned::new(&instance, compute_pipeline)).unwrap();
        table.push(Owned::new(&instance, bind_group)).unwrap();
        table.push(Owned::new(&instance, encoder)).unwrap();
        table.push(Owned::new(&instance, command_buffer)).unwrap();
        table.push(Queue { handle }).unwrap();
        assert_eq!(instance.generate_report().hub.devices.num_allocated, 1);

        drop(table);

        let hub = instance.generate_report().hub;
        for (kind, report) in [
            ("devices", hub.devices),
            ("queues", hub.queues),
            ("buffers", hub.buffers),
            ("shader modules", hub.shader_modules),
            ("bind group layouts", hub.bind_group_layouts),
            ("pipeline layouts", hub.pipeline_layouts),
            ("compute pipelines", hub.compute_pipelines),
            ("bind groups", hub.bind_groups),
            ("command encoders", hub.command_encoders),
            ("command buffers", hub.command_buffers),
        ] {
            assert_eq!(report.num_allocated, 0, "{kind} left in the hub");
        }
    }
}
