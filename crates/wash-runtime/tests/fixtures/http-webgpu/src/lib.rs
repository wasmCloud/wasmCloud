use anyhow::Context;
use bindings::wasi::webgpu::webgpu;
use std::{future::Future, task::Poll};

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

        let body = if path == "/lost" {
            report(destroy_and_await_lost().await)
        } else if let Some(iterations) = path.strip_prefix("/runaway/") {
            match iterations.parse::<u32>() {
                Ok(iterations) => report(run_away(iterations).await),
                Err(e) => format!("bad iteration count: {e}"),
            }
        } else if let Some(iterations) = path.strip_prefix("/destroy-during-map/") {
            match iterations.parse::<u32>() {
                Ok(iterations) => report(destroy_during_map(iterations).await),
                Err(e) => format!("bad iteration count: {e}"),
            }
        } else {
            double_numbers(&path).await
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

fn report(result: anyhow::Result<String>) -> String {
    result.unwrap_or_else(|e| format!("gpu error: {e:?}"))
}

async fn request_device() -> anyhow::Result<webgpu::GpuDevice> {
    let adapter = webgpu::get_gpu()
        .request_adapter(None)
        .await
        .context("failed to request adapter")?;
    Ok(adapter.request_device(None).await?)
}

fn describe_lost(info: &webgpu::GpuDeviceLostInfo) -> String {
    let reason = match info.reason() {
        webgpu::GpuDeviceLostReason::Unknown => "unknown",
        webgpu::GpuDeviceLostReason::Destroyed => "destroyed",
    };
    format!("lost: {reason}: {}", info.message())
}

async fn destroy_and_await_lost() -> anyhow::Result<String> {
    let device = request_device().await?;
    let lost = device.lost();
    device.destroy();
    Ok(describe_lost(&lost.await))
}

const RUNAWAY_SHADER: &str = r#"
@group(0)
@binding(0)
var<storage, read_write> data: array<u32>;

@compute
@workgroup_size(64)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    var i = 0u;
    loop {
        if (i >= data[0]) { break; }
        data[global_id.x + 1u] = data[global_id.x + 1u] ^ i;
        i = i + 1u;
    }
}
"#;

const RUNAWAY_INVOCATIONS: u32 = 64 * 64;

/// Submits a shader that loops `iterations` times per invocation, copying its
/// output to the returned staging buffer.
fn submit_runaway(
    device: &webgpu::GpuDevice,
    queue: &webgpu::GpuQueue,
    iterations: u32,
) -> anyhow::Result<webgpu::GpuBuffer> {
    let size = u64::from(RUNAWAY_INVOCATIONS + 1) * 4;
    let storage = device.create_buffer(&webgpu::GpuBufferDescriptor {
        label: None,
        size,
        usage: webgpu::GpuBufferUsage::STORAGE | webgpu::GpuBufferUsage::COPY_SRC,
        mapped_at_creation: Some(true),
    });
    let mut contents = vec![0u32; RUNAWAY_INVOCATIONS as usize + 1];
    contents[0] = iterations;
    storage.get_mapped_range_set_with_copy(bytemuck::cast_slice(&contents), None, None)?;
    storage.unmap()?;
    let staging = device.create_buffer(&webgpu::GpuBufferDescriptor {
        label: None,
        size: 4,
        usage: webgpu::GpuBufferUsage::MAP_READ | webgpu::GpuBufferUsage::COPY_DST,
        mapped_at_creation: None,
    });

    let module = device.create_shader_module(&webgpu::GpuShaderModuleDescriptor {
        label: None,
        code: RUNAWAY_SHADER.to_string(),
        compilation_hints: None,
    });
    let pipeline = device.create_compute_pipeline(webgpu::GpuComputePipelineDescriptor {
        label: None,
        layout: webgpu::GpuLayoutMode::Auto,
        compute: webgpu::GpuProgrammableStage {
            module: &module,
            entry_point: Some("main".to_string()),
            constants: None,
        },
    });
    let bind_group = device.create_bind_group(&webgpu::GpuBindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: vec![webgpu::GpuBindGroupEntry {
            binding: 0,
            resource: webgpu::GpuBindingResource::GpuBuffer(&storage),
        }],
    });

    let encoder = device.create_command_encoder(None);
    let pass = encoder.begin_compute_pass(None);
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, Some(&bind_group), None, None, None)?;
    pass.dispatch_workgroups(RUNAWAY_INVOCATIONS / 64, Some(1), Some(1));
    pass.end();
    encoder.copy_buffer_to_buffer(&storage, Some(4), &staging, Some(0), Some(4));
    queue.submit(&[&encoder.finish(None)]);
    Ok(staging)
}

fn describe_map(mapped: Result<(), webgpu::MapAsyncError>) -> String {
    match mapped {
        Ok(()) => "map: ok".to_string(),
        Err(e) => {
            let kind = match e.kind {
                webgpu::MapAsyncErrorKind::OperationError => "OperationError",
                webgpu::MapAsyncErrorKind::RangeError => "RangeError",
                webgpu::MapAsyncErrorKind::AbortError => "AbortError",
            };
            format!("map: {kind}: {}", e.message)
        }
    }
}

/// Reads a runaway shader's output back, the way a guest waits on compute
/// results.
async fn run_away(iterations: u32) -> anyhow::Result<String> {
    let device = request_device().await?;
    let lost = device.lost();
    let queue = device.queue();
    let staging = submit_runaway(&device, &queue, iterations)?;

    let mapped = staging
        .map_async(webgpu::GpuMapMode::READ, None, None)
        .await;
    if mapped.is_ok() {
        return Ok(describe_map(mapped));
    }
    let mapped = describe_map(mapped);
    let lost = describe_lost(&lost.await);

    // A lost device refuses new work without trapping the guest.
    queue.submit(&[&device.create_command_encoder(None).finish(None)]);
    queue.on_submitted_work_done().await;
    Ok(format!("{mapped}\n{lost}\nafter: ok"))
}

/// Destroys the device while a map waits on a runaway shader.
async fn destroy_during_map(iterations: u32) -> anyhow::Result<String> {
    let device = request_device().await?;
    let queue = device.queue();
    let staging = submit_runaway(&device, &queue, iterations)?;

    let mut map = std::pin::pin!(staging.map_async(webgpu::GpuMapMode::READ, None, None));
    // The first poll starts the call.
    let started = std::future::poll_fn(|cx| Poll::Ready(map.as_mut().poll(cx))).await;
    if let Poll::Ready(mapped) = started {
        return Ok(format!("finished before destroy: {}", describe_map(mapped)));
    }
    device.destroy();
    Ok(describe_map(map.await))
}

async fn double_numbers(path: &str) -> String {
    let numbers = path
        .trim_start_matches('/')
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect::<Vec<u32>>();

    if numbers.is_empty() {
        "No numbers provided\ntry `/1,2,3,4`".to_string()
    } else {
        match double_numbers_on_gpu(&numbers).await {
            Ok(results) => format!("results: {results:?}"),
            Err(e) => format!("gpu error: {e:?}"),
        }
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
