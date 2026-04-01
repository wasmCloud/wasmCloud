//! Integration test for http-counter component with blobstore-filesystem plugin
//!
//! This test demonstrates component-to-component linking by:
//! 1. Running the blobstore-filesystem plugin as a component that exports wasi:blobstore
//! 2. Running the http-counter component that imports wasi:blobstore
//! 3. Verifying that the http-counter can use the blobstore-filesystem implementation
//! 4. Testing the component resolution system that links them together

use anyhow::{Context, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Mutex, time::timeout};

use wash_runtime::{
    engine::{
        Engine,
        ctx::{ActiveCtx, SharedCtx, extract_active_ctx},
        workload::{ResolvedWorkload, WorkloadItem},
    },
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::{HostPlugin, wasi_config::DynamicConfig, wasi_keyvalue::InMemoryKeyValue},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        imports: { default: async | trappable },
        inline: "
            package wasmcloud:test@0.1.0;

            world logging {
                import wasi:logging/logging@0.1.0-draft;
            }
        "
    });
}

use bindings::wasi::logging::logging::Level;

const CALLER_WASM: &[u8] = include_bytes!("wasm/inter_component_call_caller.wasm");
const MIDDLEWARE_WASM: &[u8] = include_bytes!("wasm/inter_component_call_middleware.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/inter_component_call_callee.wasm");

#[derive(Clone)]
pub struct PerComponentInfo {
    workload_id: String,
}

#[derive(Default)]
pub struct CustomLogging {
    tracker: Mutex<HashMap<String, PerComponentInfo>>,
    prev_ctx_id: Mutex<Option<String>>,
}

impl<'a> bindings::wasi::logging::logging::Host for ActiveCtx<'a> {
    async fn log(
        &mut self,
        level: Level,
        context: String,
        message: String,
    ) -> wasmtime::Result<()> {
        let plugin = self
            .get_plugin::<CustomLogging>("logging")
            .ok_or_else(|| wasmtime::format_err!("failed to get plugin"))?;

        let per_component_info = plugin
            .tracker
            .lock()
            .await
            .get(&*self.component_id)
            .cloned();

        if !per_component_info.is_some_and(|info| info.workload_id == &*self.workload_id) {
            return Err(wasmtime::format_err!("workload ID mismatch"));
        }

        let prev_ctx_id = plugin.prev_ctx_id.lock().await.clone();
        match (prev_ctx_id, &self.id) {
            (Some(prev_ctx_id), ctx_id) => {
                if prev_ctx_id == *ctx_id {
                    panic!("same context");
                }
            }
            (_, _) => {}
        }

        *plugin.prev_ctx_id.lock().await = Some(self.id.clone());

        match level {
            Level::Critical => tracing::error!(id = &self.id, context, "{message}"),
            Level::Error => tracing::error!(id = &self.id, context, "{message}"),
            Level::Warn => tracing::warn!(id = &self.id, context, "{message}"),
            Level::Info => tracing::info!(id = &self.id, context, "{message}"),
            Level::Debug => tracing::debug!(id = &self.id, context, "{message}"),
            Level::Trace => tracing::trace!(id = &self.id, context, "{message}"),
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostPlugin for CustomLogging {
    fn id(&self) -> &'static str {
        "logging"
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:logging/logging")]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        workload_handle: &mut WorkloadItem<'a>,
        interfaces: std::collections::HashSet<wash_runtime::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Ensure exactly one interface: "wasi:logging/logging"
        let mut iter = interfaces.iter();
        let Some(interface) = iter.next() else {
            anyhow::bail!("No interfaces provided; expected wasi:logging/logging");
        };
        if iter.next().is_some()
            || interface.namespace != "wasi"
            || interface.package != "logging"
            || !interface.interfaces.contains("logging")
        {
            anyhow::bail!(
                "Expected exactly one interface: wasi:logging/logging, got: {:?}",
                interfaces
            );
        }

        // Add `wasi:logging/logging` to the workload's linker
        bindings::wasi::logging::logging::add_to_linker::<_, SharedCtx>(
            workload_handle.linker(),
            extract_active_ctx,
        )?;

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        self.tracker.lock().await.insert(
            component_id.to_string(),
            PerComponentInfo {
                workload_id: workload.id().to_string(),
            },
        );
        Ok(())
    }
}

#[tokio::test]
async fn test_inter_component_call() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Create engine
    let engine = Engine::builder().build()?;

    // Create HTTP server plugin on a dynamically allocated port
    let http_plugin = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_plugin.addr();

    // Create keyvalue plugin for counter persistence (still using built-in)
    let keyvalue_plugin = InMemoryKeyValue::new();

    // Create logging plugin
    let logging_plugin = CustomLogging::default();

    // Create config plugin
    let config_plugin = DynamicConfig::default();

    // Build host WITHOUT the built-in blobstore plugin
    // We'll use the blobstore-filesystem component instead
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(keyvalue_plugin))?
        .with_plugin(Arc::new(logging_plugin))?
        .with_plugin(Arc::new(config_plugin))?
        .build()?;

    // Start the host (which starts all plugins)
    let host = host.start().await.context("Failed to start host")?;
    println!("Host started, HTTP server listening on {addr}");

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "caller".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: "caller".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CALLER_WASM),
                    local_resources: LocalResources {
                        memory_limit_mb: 128,
                        cpu_limit: 1,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: Default::default(),
                    },
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: "middleware".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(MIDDLEWARE_WASM),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 2,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: Default::default(),
                    },
                    pool_size: 2,
                    max_invocations: 100,
                },
                Component {
                    name: "callee".to_string(),
                    digest: None,
                    bytes: bytes::Bytes::from_static(CALLEE_WASM),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 2,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: Default::default(),
                    },
                    pool_size: 2,
                    max_invocations: 100,
                },
            ],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: None,
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "test".to_string());
                        config
                    },
                    name: None,
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "logging".to_string(),
                    interfaces: ["logging".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
                    config: HashMap::new(),
                    name: None,
                },
            ],
            volumes: vec![],
        },
    };

    let _ = host
        .workload_start(req)
        .await
        .context("Failed to start workload with component linking")?;

    let client = reqwest::Client::new();

    println!("Testing inter-component call");
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("First request timed out")?
    .context("Failed to make first request")?;

    let status = response.status();
    println!("First Response Status: {}", status);

    assert!(
        status.is_success(),
        "First request failed with status {}",
        status,
    );

    Ok(())
}
