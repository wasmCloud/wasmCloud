//! End-to-end: an unlabelled import a host plugin serves is not ambiguous just
//! because two sibling components in the same workload export the interface.
//!
//! `served_within_workload` leaves such an import to the plugin (two exporters
//! is the ambiguity `build_export_map` declines to resolve), the plugin binds
//! it, and the link step skips it. The import-resolution pass must agree and
//! not refuse the workload. These tests start real workloads on a real host,
//! drive the importer over HTTP, and check the call reaches the plugin and
//! neither sibling.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, Result};

use wash_runtime::engine::Engine;
use wash_runtime::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use wash_runtime::engine::workload::WorkloadItem;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::{HostPlugin, WitInterfaces};
use wash_runtime::types::{
    Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState, WorkloadStatus,
};
use wash_runtime::wit::{WitInterface, WitWorld};

mod common;

const CALLER_WASM: &[u8] = include_bytes!("wasm/inter_component_call_caller.wasm");
const MIDDLEWARE: &str = "wasmcloud:example/middleware@0.0.1";

mod bindings {
    wasmtime::component::bindgen!({
        imports: { default: async | trappable },
        inline: "
            package wasmcloud:example@0.0.1;

            interface middleware {
                invoke: func() -> result<_, string>;
            }

            world serve {
                import middleware;
            }
        "
    });
}

/// A native plugin serving `wasmcloud:example/middleware` and counting calls.
#[derive(Default)]
struct MiddlewarePlugin {
    calls: AtomicUsize,
    bound_importers: std::sync::Mutex<HashSet<String>>,
}

const PLUGIN_ID: &str = "middleware-plugin";

impl<'a> bindings::wasmcloud::example::middleware::Host for ActiveCtx<'a> {
    async fn invoke(&mut self) -> wasmtime::Result<Result<(), String>> {
        let plugin = self.try_get_plugin::<MiddlewarePlugin>(PLUGIN_ID)?;
        plugin.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Ok(()))
    }
}

#[async_trait::async_trait]
impl HostPlugin for MiddlewarePlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(MIDDLEWARE)]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // Exporters match on their export too; only an importer needs the
        // instance in its linker.
        let imports_it = item
            .world()
            .imports
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "example");
        if imports_it {
            self.bound_importers
                .lock()
                .unwrap()
                .insert(item.id().to_string());
            bindings::wasmcloud::example::middleware::add_to_linker::<_, SharedCtx>(
                item.linker(),
                extract_active_ctx,
            )?;
        }
        Ok(())
    }
}

/// A sibling exporting the very same `middleware` type as the plugin, whose
/// `invoke` traps. Were the importer linked to it, the request would fail.
fn trapping_middleware_exporter() -> Vec<u8> {
    wat::parse_str(format!(
        r#"(component
            (core module $m
                (memory (export "memory") 1)
                (func (export "invoke") (result i32) unreachable))
            (core instance $i (instantiate $m))
            (func $f (result (result (error string)))
                (canon lift (core func $i "invoke") (memory (core memory $i "memory"))))
            (instance $inst (export "invoke" (func $f)))
            (export "{MIDDLEWARE}" (instance $inst)))"#
    ))
    .expect("sibling WAT parses")
}

fn component(name: &str, bytes: bytes::Bytes) -> Component {
    Component {
        name: name.to_string(),
        digest: None,
        bytes,
        local_resources: LocalResources::default(),
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    }
}

fn middleware_interface() -> WitInterface {
    WitInterface::from(MIDDLEWARE)
}

/// Caller plus two trapping siblings that both export `middleware`.
fn workload(host: &str, serve_middleware: bool) -> WorkloadStartRequest {
    let sibling = bytes::Bytes::from(trapping_middleware_exporter());
    let mut host_interfaces = vec![common::http_incoming_handler_interface(host, None)];
    if serve_middleware {
        host_interfaces.push(middleware_interface());
    }
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("caller", bytes::Bytes::from_static(CALLER_WASM)),
                component("exporter-a", sibling.clone()),
                component("exporter-b", sibling),
            ],
            host_interfaces,
            volumes: vec![],
        },
    }
}

/// Start `req` and return its status. A refused workload still answers `Ok`
/// from `workload_start`, with the reason in the status, so callers must look.
async fn start(host: &impl HostApi, req: WorkloadStartRequest) -> Result<WorkloadStatus> {
    Ok(host.workload_start(req).await?.workload_status)
}

fn assert_running(status: &WorkloadStatus) {
    assert_eq!(
        status.workload_state,
        WorkloadState::Running,
        "the plugin-bound import must not be refused as ambiguous: {}",
        status.message
    );
}

fn assert_ambiguous(status: &WorkloadStatus) {
    assert_ne!(status.workload_state, WorkloadState::Running);
    assert!(
        status.message.contains("exported by multiple components"),
        "unexpected status: {status:?}"
    );
}

async fn start_host(
    plugin: Option<Arc<MiddlewarePlugin>>,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let mut builder = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress));
    if let Some(plugin) = plugin {
        builder = builder.with_plugin(plugin)?;
    }
    let host = builder.build()?.start().await.context("host start")?;
    Ok((addr, host))
}

/// The plugin serves the caller's unlabelled `middleware` import while two
/// siblings export it: the workload starts, every call lands on the plugin,
/// and neither (trapping) sibling is ever called.
#[tokio::test]
async fn native_plugin_serves_an_import_two_siblings_export() -> Result<()> {
    let plugin = Arc::new(MiddlewarePlugin::default());
    let (addr, host) = start_host(Some(plugin.clone())).await?;
    assert_running(&start(&host, workload("native-plugin", true)).await?);

    assert_eq!(
        plugin.bound_importers.lock().unwrap().len(),
        1,
        "the plugin bound exactly the caller's import"
    );

    let client = reqwest::Client::new();
    for n in 1..=3 {
        let (status, body) = common::req(&client, &addr, "native-plugin", "/").await?;
        assert!(status.is_success(), "call {n}: {status} {body}");
        assert_eq!(
            plugin.calls.load(Ordering::SeqCst),
            n,
            "call {n} reached the plugin"
        );
    }
    Ok(())
}

/// The control: with no plugin serving it, the same workload is refused as
/// ambiguous, so the test above is exercising the plugin path.
#[tokio::test]
async fn without_a_plugin_two_sibling_exporters_are_still_ambiguous() -> Result<()> {
    let (_addr, host) = start_host(None).await?;
    assert_ambiguous(&start(&host, workload("no-plugin", false)).await?);
    Ok(())
}

/// A plugin that is registered but not asked for (no `hostInterfaces` entry)
/// binds nothing, so the import is still the workload's and still ambiguous.
#[tokio::test]
async fn an_unrequested_plugin_does_not_lift_the_ambiguity() -> Result<()> {
    let plugin = Arc::new(MiddlewarePlugin::default());
    let (_addr, host) = start_host(Some(plugin.clone())).await?;
    assert_ambiguous(&start(&host, workload("unrequested", false)).await?);
    assert!(plugin.bound_importers.lock().unwrap().is_empty());
    Ok(())
}

/// The same shape with a host component plugin: `ephemeral-callee-p3` serves
/// `wasmcloud:ephemeral-test/compute` from its own persistent store to the p3
/// caller while two copies of the same callee sit in the workload as siblings
/// exporting it. Warm siblings would also count, so they keep no instances:
/// a sibling call always answers `1`, while the plugin's one store climbs.
#[cfg(feature = "host-component-plugins")]
mod component_plugin {
    use super::*;

    const CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
    const CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");

    fn compute_interface() -> WitInterface {
        WitInterface::from("wasmcloud:ephemeral-test/compute@0.1.0")
    }

    fn workload(host: &str, serve_compute: bool) -> WorkloadStartRequest {
        let mut host_interfaces = vec![common::http_incoming_handler_interface(host, None)];
        if serve_compute {
            host_interfaces.push(compute_interface());
        }
        let sibling = |name: &str| Component {
            pool_size: 0,
            ..component(name, bytes::Bytes::from_static(CALLEE_P3_WASM))
        };
        WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: host.to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![
                    component("caller", bytes::Bytes::from_static(CALLER_P3_WASM)),
                    sibling("exporter-a"),
                    sibling("exporter-b"),
                ],
                host_interfaces,
                volumes: vec![],
            },
        }
    }

    #[tokio::test]
    async fn component_plugin_serves_an_import_two_siblings_export() -> Result<()> {
        let (addr, host) = common::start_host_with_component_plugin(
            "127.0.0.1:0",
            "compute-plugin",
            CALLEE_P3_WASM,
        )
        .await?;
        assert_running(&start(&host, workload("component-plugin", true)).await?);

        let client = reqwest::Client::new();
        let (status, body) = common::req(&client, &addr, "component-plugin", "/").await?;
        assert!(status.is_success(), "{status} {body}");
        assert_eq!(body, "43", "run(21) answered");

        // The plugin's single persistent store has now served `run`, so its
        // counter keeps climbing; an ephemeral sibling would say 1 each time.
        let mut seen = Vec::new();
        for _ in 0..3 {
            let (status, body) = common::req(&client, &addr, "component-plugin", "/calls").await?;
            assert!(status.is_success(), "{status} {body}");
            seen.push(body.parse::<u32>().context("calls is a number")?);
        }
        assert!(
            seen.windows(2).all(|w| w[1] > w[0]),
            "calls must climb on the plugin's persistent store, got {seen:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn without_the_plugin_entry_the_siblings_are_ambiguous() -> Result<()> {
        let (_addr, host) = common::start_host_with_component_plugin(
            "127.0.0.1:0",
            "compute-plugin",
            CALLEE_P3_WASM,
        )
        .await?;
        assert_ambiguous(&start(&host, workload("component-unrequested", false)).await?);
        Ok(())
    }
}
