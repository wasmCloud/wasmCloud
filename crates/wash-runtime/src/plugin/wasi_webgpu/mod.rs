//! # WASI Webgpu Plugin
//!
//! This module implements a webgpu plugin for the wasmCloud runtime,
//! providing the `wasi:webgpu@0.0.1` interfaces.

use std::{collections::HashSet, sync::Arc};

const WASI_WEBGPU_ID: &str = "wasi-webgpu";

use crate::{
    engine::{ctx::SharedCtx, workload::WorkloadItem},
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

/// Webgpu plugin
#[derive(Clone)]
pub struct WebGpu {
    pub gpu: Arc<wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global>,
}

/// Backend options for the WasiWebGpu plugin
#[derive(Clone, Copy)]
pub enum WebGpuBackend {
    /// Backend with all available features
    All,
    /// Noop backend for testing purposes. It does not perform any real GPU operations.
    Noop,
}

impl WebGpu {
    pub fn new(backend: WebGpuBackend) -> Self {
        let (backends, backend_options) = match backend {
            WebGpuBackend::All => (
                wasi_webgpu_wasmtime::reexports::wgpu_types::Backends::all(),
                wasi_webgpu_wasmtime::reexports::wgpu_types::BackendOptions::default(),
            ),
            WebGpuBackend::Noop => (
                wasi_webgpu_wasmtime::reexports::wgpu_types::Backends::NOOP,
                wasi_webgpu_wasmtime::reexports::wgpu_types::BackendOptions {
                    noop: wasi_webgpu_wasmtime::reexports::wgpu_types::NoopBackendOptions {
                        enable: true,
                    },
                    ..Default::default()
                },
            ),
        };

        Self {
            gpu: Arc::new(wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global::new(
                "webgpu",
                wasi_webgpu_wasmtime::reexports::wgpu_types::InstanceDescriptor {
                    backends,
                    backend_options,
                    flags: wasi_webgpu_wasmtime::reexports::wgpu_types::InstanceFlags::from_build_config(),
                    memory_budget_thresholds: Default::default(),
                    display: None,
                },
                None,
            )),
        }
    }
}

impl Default for WebGpu {
    fn default() -> Self {
        Self::new(WebGpuBackend::All)
    }
}

impl wasi_graphics_context_wasmtime::WasiGraphicsContextView for SharedCtx {}

struct UiThreadSpawner;
impl wasi_webgpu_wasmtime::MainThreadSpawner for UiThreadSpawner {
    async fn spawn<F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        f()
    }
}

impl wasi_webgpu_wasmtime::WasiWebGpuView for SharedCtx {
    #[allow(clippy::expect_used)] // Trait doesn't return Result; plugin is registered at startup
    fn instance(&self) -> Arc<wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global> {
        let plugin = self
            .active_ctx
            .get_plugin::<WebGpu>(WASI_WEBGPU_ID)
            .expect("WebGpu plugin should be registered");
        Arc::clone(&plugin.gpu)
    }

    fn ui_thread_spawner(&self) -> Box<impl wasi_webgpu_wasmtime::MainThreadSpawner + 'static> {
        Box::new(UiThreadSpawner)
    }
}

#[async_trait::async_trait]
impl HostPlugin for WebGpu {
    fn id(&self) -> &'static str {
        WASI_WEBGPU_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            exports: HashSet::from([
                WitInterface::from("wasi:graphics-context/graphics-context"),
                WitInterface::from("wasi:webgpu/webgpu"),
            ]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:webgpu related
        let has_webgpu = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "webgpu");

        if !has_webgpu {
            tracing::warn!(
                "WasiWebgpu plugin requested for non-wasi:webgpu interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        tracing::debug!(
            workload_id = component_handle.id(),
            "Adding webgpu interfaces to linker for workload"
        );
        let linker = component_handle.linker();

        wasi_webgpu_wasmtime::add_to_linker(linker)?;
        wasi_graphics_context_wasmtime::add_to_linker(linker)?;

        let id = component_handle.id();
        tracing::debug!(
            workload_id = id,
            "Successfully added webgpu interfaces to linker for workload"
        );

        Ok(())
    }
}
