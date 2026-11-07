// Wrappers around `wgpu_*` types
// Every type here should have an explanation as to why we can't use the type directly.

use std::{
    collections::HashMap,
    fmt::{self, Debug},
    sync::{Arc, Mutex},
};

use crate::{device_handle::DeviceHandle, wasi::webgpu::webgpu};

pub struct WgslLanguageFeatures;
impl WgslLanguageFeatures {
    pub fn new() -> Self {
        Self
    }
    pub fn has(&self, feature: &str) -> bool {
        use wgpu_core::naga::front::wgsl::LanguageExtension;
        matches!(
            LanguageExtension::from_ident(feature),
            Some(LanguageExtension::Implemented(_))
        )
    }
}

// can't pass generics to `wasmtime::component::bindgen`
// TODO: these should be unit-structs instead of `types` so that the internals are private to the crate
pub type RecordGpuPipelineConstantValue = HashMap<String, webgpu::GpuPipelineConstantValue>;
pub type RecordOptionGpuSize64 = HashMap<String, Option<webgpu::GpuSize64>>;

// RenderPassEncoder, ComputePassEncoder, and RenderBundleEncoder need to be dropped when calling `.end`/`.finish` on them, but we can't guarantee that they'll be dropped in time by GC languages. Takeable lets you take the value and leaves None in place, so that RenderPass/ComputePass get dropped from Rust's point of view, but the wasm module can keep it's reference.
// this is caused by the same underlying issue as this one https://github.com/gfx-rs/wgpu-native/issues/412
// TODO: these should be unit-structs instead of `types` so that the internals are private to the crate
pub type RenderPassEncoder = Takeable<wgpu_core::command::RenderPass>;
pub type ComputePassEncoder = Takeable<wgpu_core::command::ComputePass>;
pub type RenderBundleEncoder = Takeable<RenderBundleEncoderInner>;

#[derive(Clone, Debug)]
pub struct Takeable<T: std::fmt::Debug>(Arc<std::sync::Mutex<Option<T>>>);
impl<T> Takeable<T>
where
    T: std::fmt::Debug,
{
    pub(crate) fn new(id: T) -> Self {
        Takeable(Arc::new(std::sync::Mutex::new(Some(id))))
    }
    pub(crate) fn lock<'a>(&'a self) -> std::sync::MutexGuard<'a, Option<T>> {
        self.0.lock().unwrap()
    }
    // pub(crate) fn take(&self) -> Option<T> {
    //     self.0.lock().unwrap().take()
    // }
}

// size needed in `GpuBuffer.size`, `RenderPass.set_index_buffer`, `RenderPass.set_vertex_buffer`.
// usage needed in `GpuBuffer.usage`
pub struct Buffer {
    pub(crate) buffer_id: wgpu_core::id::BufferId,
    pub(crate) device: Arc<DeviceHandle>,
    pub(crate) size: u64,
    pub(crate) usage: wgpu_types::BufferUsages,
    /// Shared with a pending map, which settles it.
    pub(crate) map_state: Arc<Mutex<webgpu::GpuBufferMapState>>,
}

impl Buffer {
    /// Destroying the device unmaps its buffers.
    /// https://www.w3.org/TR/webgpu/#dom-gpudevice-destroy
    pub(crate) fn map_state(&self) -> webgpu::GpuBufferMapState {
        if self.device.is_destroyed() {
            return webgpu::GpuBufferMapState::Unmapped;
        }
        *self.map_state.lock().unwrap()
    }

    pub(crate) fn set_map_state(&self, map_state: webgpu::GpuBufferMapState) {
        *self.map_state.lock().unwrap() = map_state;
    }
}

// references to adapter are also saved in device.
// TODO: these should be unit-structs instead of `types` so that the internals are private to the crate
pub type Adapter = Arc<wgpu_core::id::AdapterId>;

// adapter needed for surface_get_capabilities in connect_graphics_context
// keeping adapter as Arc for reference counting while dropping.
#[derive(Clone)]
pub struct Device {
    pub(crate) handle: Arc<DeviceHandle>,
    pub(crate) adapter: Arc<wgpu_core::id::AdapterId>,
    pub(crate) error_handler: Arc<ErrorHandler>,
}

/// A device's queue. It keeps the device alive, so the queue can poll it.
pub struct Queue {
    pub(crate) handle: Arc<DeviceHandle>,
    pub(crate) error_handler: Arc<ErrorHandler>,
}

impl Queue {
    pub fn queue_id(&self) -> &wgpu_core::id::QueueId {
        self.handle.queue_id()
    }

    pub fn device_id(&self) -> &wgpu_core::id::DeviceId {
        self.handle.device_id()
    }
}

impl Device {
    pub fn device_id(&self) -> &wgpu_core::id::DeviceId {
        self.handle.device_id()
    }

    /// Create a `Texture` from a `TextureId` that is connected to this device.
    /// Useful in cases where an external crate get a texture through get_current_texture
    /// and needs to connect it to a device.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the TextureId is actually connected to this device
    #[doc(hidden)]
    pub unsafe fn connect_texture(&self, texture_id: wgpu_core::id::TextureId) -> Texture {
        Texture {
            texture_id,
            error_handler: Arc::clone(&self.error_handler),
        }
    }
}

#[derive(Clone)]
pub struct CommandEncoder {
    pub(crate) command_encoder_id: wgpu_core::id::CommandEncoderId,
    pub(crate) error_handler: Arc<ErrorHandler>,
}

pub struct Texture {
    pub(crate) texture_id: wgpu_core::id::TextureId,
    pub(crate) error_handler: Arc<ErrorHandler>,
}
pub struct RenderPipeline {
    pub(crate) render_pipeline_id: wgpu_core::id::RenderPipelineId,
    pub(crate) error_handler: Arc<ErrorHandler>,
}

#[derive(Debug)]
pub struct RenderBundleEncoderInner {
    pub(crate) render_bundle_encoder: wgpu_core::command::RenderBundleEncoder,
    pub(crate) error_handler: Arc<ErrorHandler>,
}
pub struct ComputePipeline {
    pub(crate) compute_pipeline_id: wgpu_core::id::ComputePipelineId,
    pub(crate) error_handler: Arc<ErrorHandler>,
}

#[derive(Debug, Clone)]
pub struct GpuError {
    pub(crate) message: String,
    pub(crate) kind: webgpu::GpuErrorKind,
}

// Device level error handler
#[derive(Debug)]
pub(crate) struct ErrorHandler(Mutex<ErrorHandlerInner>);

#[derive(Debug)]
pub(crate) struct ErrorHandlerInner {
    scopes: Vec<ErrorScope>,
    uncaptured_error_sender: async_broadcast::Sender<webgpu::GpuError>,
    // Keeping inactive receiver to keep channel open.
    // See https://docs.rs/async-broadcast/0.7.1/async_broadcast/struct.InactiveReceiver.html
    _uncaptured_error_receiver: async_broadcast::InactiveReceiver<webgpu::GpuError>,
}

impl Default for ErrorHandler {
    fn default() -> Self {
        let (sender, receiver) = async_broadcast::broadcast(5);
        let receiver = receiver.deactivate();
        let inner = ErrorHandlerInner {
            scopes: Default::default(),
            uncaptured_error_sender: sender,
            _uncaptured_error_receiver: receiver,
        };
        Self(Mutex::new(inner))
    }
}

#[derive(Debug)]
struct ErrorScope {
    error: Option<webgpu::GpuError>,
    filter: webgpu::GpuErrorFilter,
}

impl ErrorHandler {
    pub fn push_scope(&self, filter: webgpu::GpuErrorFilter) {
        self.0.lock().unwrap().scopes.push(ErrorScope {
            filter,
            error: None,
        });
    }

    pub fn pop_scope(&self) -> Result<Option<webgpu::GpuError>, webgpu::PopErrorScopeError> {
        let scopes = self.0.lock().unwrap().scopes.pop();
        match scopes {
            Some(scope) => Ok(scope.error),
            None => {
                // From the spec:
                // > If any of the following requirements are unmet:
                // >  - this.[[errorScopeStack]].size must be > 0.
                // > Then issue the following steps on contentTimeline and return:
                // >  1. Reject promise with an OperationError.
                // https://www.w3.org/TR/webgpu/#dom-gpudevice-poperrorscope
                Err(webgpu::PopErrorScopeError {
                    kind: webgpu::PopErrorScopeErrorKind::OperationError,
                    message: "pop-error-scope on empty stack".to_string(),
                })
            }
        }
    }

    pub fn handle_possible_error<E: wgpu_types::error::WebGpuError + fmt::Display>(
        &self,
        error: Option<E>,
    ) {
        if let Some(error) = error {
            // > If device is lost, return; no errors are generated after this.
            // https://www.w3.org/TR/webgpu/#abstract-opdef-generate-an-error
            let error_kind = match error.webgpu_error_type() {
                wgpu_types::error::ErrorType::DeviceLost => return,
                error_type => error_type.into(),
            };
            let error = GpuError {
                message: error.to_string(),
                kind: error_kind,
            };

            let error_filter = error_kind.into();
            let mut inner = self.0.lock().unwrap();
            match &mut inner
                .scopes
                .iter_mut()
                .rev()
                .find(|scope| scope.filter == error_filter)
            {
                Some(scope) => {
                    // Only return one error per scope.
                    // From the spec:
                    // > 4. Let error be any one of the items in scope.[[errors]], or null if there are none.
                    // >   For any two errors E1 and E2 in the list, if E2 was caused by E1, E2 should not be the one selected.
                    // https://www.w3.org/TR/webgpu/#dom-gpudevice-poperrorscope
                    // Here we're assuming that the first error is the one that caused the others, so only set the first error.
                    if scope.error.is_none() {
                        scope.error = Some(error);
                    }
                }
                None => {
                    shared::unwrap_unless_inactive_or_full(
                        inner.uncaptured_error_sender.try_broadcast(error),
                    );
                }
            }
        }
    }

    pub(crate) fn new_error_receiver(&self) -> async_broadcast::Receiver<webgpu::GpuError> {
        self.0
            .lock()
            .unwrap()
            .uncaptured_error_sender
            .new_receiver()
    }
}

// For now, GpuErrorFilter and GpuErrorKind are effectively the same, just with different names.
impl From<webgpu::GpuErrorFilter> for webgpu::GpuErrorKind {
    fn from(filter: webgpu::GpuErrorFilter) -> Self {
        match filter {
            webgpu::GpuErrorFilter::Validation => webgpu::GpuErrorKind::ValidationError,
            webgpu::GpuErrorFilter::OutOfMemory => webgpu::GpuErrorKind::OutOfMemoryError,
            webgpu::GpuErrorFilter::Internal => webgpu::GpuErrorKind::InternalError,
        }
    }
}
impl From<webgpu::GpuErrorKind> for webgpu::GpuErrorFilter {
    fn from(kind: webgpu::GpuErrorKind) -> Self {
        match kind {
            webgpu::GpuErrorKind::ValidationError => webgpu::GpuErrorFilter::Validation,
            webgpu::GpuErrorKind::OutOfMemoryError => webgpu::GpuErrorFilter::OutOfMemory,
            webgpu::GpuErrorKind::InternalError => webgpu::GpuErrorFilter::Internal,
        }
    }
}
