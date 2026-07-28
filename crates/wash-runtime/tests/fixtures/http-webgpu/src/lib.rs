use anyhow::Context;
use bindings::wasi::webgpu::webgpu;

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        // get the path from the request
        let path = request.get_path_with_query().unwrap_or_default();
        // parse the numbers from the path

        let numbers = path
            .trim_start_matches('/')
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .collect::<Vec<u32>>();

        let body = if numbers.is_empty() {
            "No numbers provided\ntry `/1,2,3,4`".to_string()
        } else {
            match double_numbers_on_gpu(&numbers).await {
                Ok(results) => format!("results: {results:?}"),
                Err(e) => format!("gpu error: {e:?}"),
            }
        };

        let (mut tx, rx) = bindings::wit_stream::new();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
        wit_bindgen::spawn_local(async move {
            tx.write_all(body.into_bytes()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);

const SHADER: &str = r#"
@group(0)
@binding(0)
var<storage, read_write> v_indices: array<u32>; // this is used as both input and output for convenience

@compute
@workgroup_size(1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    v_indices[global_id.x] = v_indices[global_id.x] * 2;
}
"#;

async fn double_numbers_on_gpu(numbers: &[u32]) -> anyhow::Result<Vec<u32>> {
    let gpu = webgpu::get_gpu();

    // `request_adapter` instantiates the general connection to the GPU
    let adapter = gpu
        .request_adapter(None)
        .await
        .context("failed to request adapter")?;

    // `request_device` instantiates the feature specific connection to the GPU, defining some parameters,
    //  `features` being the available features.
    let device = adapter.request_device(None).await?;
    let queue = device.queue();

    // Loads the shader from WGSL
    let cs_module = device.create_shader_module(&webgpu::GpuShaderModuleDescriptor {
        label: None,
        code: SHADER.to_string(),
        compilation_hints: None,
    });

    // Gets the size in bytes of the buffer.
    let size = std::mem::size_of_val(numbers) as u64;

    // Instantiates buffer without data.
    // `usage` of buffer specifies how it can be used:
    //   `BufferUsages::MAP_READ` allows it to be read (outside the shader).
    //   `BufferUsages::COPY_DST` allows it to be the destination of the copy.
    let staging_buffer = device.create_buffer(&webgpu::GpuBufferDescriptor {
        label: None,
        size,
        usage: webgpu::GpuBufferUsage::MAP_READ | webgpu::GpuBufferUsage::COPY_DST,
        mapped_at_creation: None,
    });

    // Instantiates buffer with data (`numbers`).
    // Usage allowing the buffer to be:
    //   A storage buffer (can be bound within a bind group and thus available to a shader).
    //   The destination of a copy.
    //   The source of a copy.
    let storage_buffer_contents = bytemuck::cast_slice(numbers);
    let storage_buffer = device.create_buffer(&webgpu::GpuBufferDescriptor {
        label: Some("Storage Buffer".to_string()),
        size: storage_buffer_contents.len() as _,
        usage: webgpu::GpuBufferUsage::STORAGE
            | webgpu::GpuBufferUsage::COPY_DST
            | webgpu::GpuBufferUsage::COPY_SRC,

        mapped_at_creation: Some(true),
    });
    storage_buffer.get_mapped_range_set_with_copy(storage_buffer_contents, None, None)?;
    storage_buffer.unmap()?;

    // A bind group defines how buffers are accessed by shaders.
    // It is to WebGPU what a descriptor set is to Vulkan.
    // `binding` here refers to the `binding` of a buffer in the shader (`layout(set = 0, binding = 0) buffer`).

    // A pipeline specifies the operation of a shader

    // Instantiates the pipeline.
    let compute_pipeline = device.create_compute_pipeline(webgpu::GpuComputePipelineDescriptor {
        label: None,
        layout: webgpu::GpuLayoutMode::Auto,
        compute: webgpu::GpuProgrammableStage {
            module: &cs_module,
            entry_point: Some("main".to_string()),
            constants: None,
        },
    });

    // Instantiates the bind group, once again specifying the binding of buffers.
    let bind_group_layout = compute_pipeline.get_bind_group_layout(0);
    let bind_group = device.create_bind_group(&webgpu::GpuBindGroupDescriptor {
        label: None,
        layout: &bind_group_layout,
        entries: vec![webgpu::GpuBindGroupEntry {
            binding: 0,
            resource: webgpu::GpuBindingResource::GpuBuffer(&storage_buffer),
        }],
    });

    // A command encoder executes one or many pipelines.
    // It is to WebGPU what a command buffer is to Vulkan.
    let encoder = device.create_command_encoder(None);
    let cpass = encoder.begin_compute_pass(None);
    cpass.set_pipeline(&compute_pipeline);
    cpass.set_bind_group(0, Some(&bind_group), None, None, None)?;
    cpass.insert_debug_marker("double numbers on gpu");
    cpass.dispatch_workgroups(numbers.len() as u32, Some(1), Some(1)); // Number of cells to run, the (x,y,z) size of item being processed
    cpass.end();

    // Sets adds copy operation to command encoder.
    // Will copy data from storage buffer on GPU to staging buffer on CPU.
    encoder.copy_buffer_to_buffer(&storage_buffer, None, &staging_buffer, None, None);

    // Submits command encoder for processing
    queue.submit(&[&encoder.finish(None)]);

    staging_buffer
        .map_async(webgpu::GpuMapMode::READ, None, None)
        .await?;

    // Gets contents of buffer
    let data = staging_buffer.get_mapped_range_get_with_copy(None, None)?;
    // Since contents are got in bytes, this converts these bytes back to u32
    let result = bytemuck::cast_slice(&data).to_vec();

    // With the current interface, we have to make sure all mapped views are
    // dropped before we unmap the buffer.
    drop(data);
    // Unmaps buffer from memory
    // If you are familiar with C++ these 2 lines can be thought of similarly to:
    //   delete myPointer;
    //   myPointer = NULL;
    // It effectively frees the memory
    staging_buffer.unmap()?;

    // Returns data from buffer
    Ok(result)
}
