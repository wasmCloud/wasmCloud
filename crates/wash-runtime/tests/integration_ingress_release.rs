//! A host that is stopped and dropped releases its ingress.
//!
//! The ingress keeps every routable workload in its handle map so an inbound
//! request can find one, and everything reachable from a workload can reach the
//! ingress back. Held strongly, those make a loop nothing can break: stopping
//! the host would free neither, and the ingress would go on holding each
//! workload's `InstancePre`, its compiled components and the engine behind them
//! for the life of the process.
//!
//! There are three ways back, and each test here pins one that the others do
//! not reach: a workload's own handle and its stores' egress hooks, an
//! ephemeral linked call (which lives in a linker closure, hence in the
//! caller's `InstancePre`), and a bound host component plugin.
//!
//! Its own binary, so the assertions are about these hosts and not about
//! whatever another test in the same process left running.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use anyhow::{Context, Result};

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, HostHandler, Ingress};
use wash_runtime::host::{Host, HostApi};
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};

mod common;
use common::{component_workload_request, http_only_host_interfaces};

const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");
const EPHEMERAL_CALLER_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_caller_p3.wasm");
const EPHEMERAL_CALLEE_P3_WASM: &[u8] = include_bytes!("wasm/ephemeral_callee_p3.wasm");

/// Build an ingress and a host around it, handing back the address, the started
/// host, and a `Weak` on the ingress. The only strong handle left is the host's
/// own, so what the `Weak` measures afterwards is the host's reachability.
async fn host_with_weak_ingress() -> Result<(std::net::SocketAddr, Arc<Host>, Weak<dyn HostHandler>)>
{
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .build()
        .context("failed to build the engine")?;
    // `DevRouter`, so a plain GET reaches the workload without these tests also
    // having to arrange hostname routing.
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?)
        .await
        .context("failed to bind an ingress")?;
    let addr = ingress.addr();
    let ingress: Arc<dyn HostHandler> = Arc::new(ingress);
    let weak = Arc::downgrade(&ingress);

    let host = Host::builder()
        .with_engine(engine)
        .with_http_handler(Arc::clone(&ingress))
        .build()
        .context("failed to build the host")?;
    let host = host.start().await.context("failed to start the host")?;
    drop(ingress);

    Ok((addr, host, weak))
}

/// Serve one GET and drain the body, so the store the call ran on is dropped
/// before the host is — a request still in flight legitimately holds the
/// ingress.
async fn serve_one(addr: &std::net::SocketAddr) -> Result<()> {
    let response = reqwest::get(format!("http://{addr}/"))
        .await
        .context("the workload must serve a request")?;
    assert!(
        response.status().is_success(),
        "the workload must serve a request, got {}",
        response.status()
    );
    response.bytes().await.context("failed to read the body")?;
    Ok(())
}

/// The workload is deliberately left running in each of these: a host torn down
/// without each of its workloads stopped first is exactly the case a strong
/// back-reference strands, and it is the case an embedder building hosts
/// repeatedly hits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_host_releases_its_ingress() -> Result<()> {
    let ingress = {
        let (addr, host, weak) = host_with_weak_ingress().await?;

        host.workload_start(component_workload_request(
            "http-handler-p3.wasm",
            "ingress-release",
            HTTP_HANDLER_P3_WASM,
            LocalResources {
                memory_limit_mb: 128,
                cpu_limit: 1,
                ..Default::default()
            },
            http_only_host_interfaces("ingress-release"),
        ))
        .await
        .context("failed to start the workload")?;

        // The handle map is populated at bind, but routing a request is what
        // builds the store whose egress hooks are the second way back.
        serve_one(&addr).await?;

        // Dropped, not stopped: `stop` clears the routing tables, which would
        // break the loop by itself. The weak handles are what has to hold for a
        // host that is simply let go.
        drop(host);
        weak
    };

    assert!(
        ingress.upgrade().is_none(),
        "the ingress outlived the host that owned it: a running workload still \
         holds it, so nothing it routes is ever freed"
    );
    Ok(())
}

/// A component that calls another component in the same workload gets an
/// `EphemeralLinkedCall`, which lives in a linker closure and so in the
/// caller's `InstancePre` — which the ingress keeps. The single-component test
/// above builds none, so only this one pins that edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workload_with_an_ephemeral_linked_call_releases_its_ingress() -> Result<()> {
    let ingress = {
        let (addr, host, weak) = host_with_weak_ingress().await?;

        let request = WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: "ingress-release-linked".to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![
                    Component {
                        name: "ephemeral-caller".to_string(),
                        digest: None,
                        bytes: bytes::Bytes::from_static(EPHEMERAL_CALLER_P3_WASM),
                        local_resources: LocalResources::default(),
                        pool_size: 1,
                        max_invocations: 100,
                        max_concurrency: 1,
                        ..Default::default()
                    },
                    Component {
                        name: "ephemeral-callee".to_string(),
                        digest: None,
                        bytes: bytes::Bytes::from_static(EPHEMERAL_CALLEE_P3_WASM),
                        local_resources: LocalResources::default(),
                        pool_size: 1,
                        max_invocations: 100,
                        max_concurrency: 1,
                        ..Default::default()
                    },
                ],
                host_interfaces: http_only_host_interfaces("ingress-release-linked"),
                volumes: vec![],
            },
        };
        host.workload_start(request)
            .await
            .context("failed to start the linked workload")?;

        // Drives the linked call, so the ephemeral path has actually run.
        serve_one(&addr).await?;

        // Dropped, not stopped: `stop` clears the routing tables, which would
        // break the loop by itself. The weak handles are what has to hold for a
        // host that is simply let go.
        drop(host);
        weak
    };

    assert!(
        ingress.upgrade().is_none(),
        "the ingress outlived its host: a linked call in a component's linker \
         still holds it"
    );
    Ok(())
}

/// A bound host component plugin is reached from every workload that binds it,
/// and the ingress holds those, so the plugin's own handle on the ingress is a
/// third way back. Neither test above loads a plugin, so only this one pins it.
///
/// `stop_first` decides which half is under test. `false` drops the host
/// outright, which is what the weak handle has to survive. `true` stops it
/// first, which is the other half of the teardown: an ingress that has stopped
/// lets go of its routing tables, so the workloads it routed — and the plugin
/// they bound — are released rather than held until the last handle on the
/// ingress goes. Dropping alone cannot show that, because the detached accept
/// loop still holds a handle on those tables.
#[cfg(feature = "host-component-plugins")]
async fn plugin_host_release(
    stop_first: bool,
) -> Result<(Weak<dyn HostHandler>, Weak<impl Sized>)> {
    use wash_runtime::host::HostBuilder;
    use wash_runtime::plugin::component_host::ComponentHostPlugin;
    use wash_runtime::wit::WitInterface;

    const EGRESS_PLUGIN_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin.wasm");
    const CALLER_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin_caller.wasm");

    let handles = {
        let engine = Engine::builder().with_pooling_allocator(false).build()?;
        let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
        let ingress: Arc<dyn HostHandler> = Arc::new(ingress);
        let weak_ingress = Arc::downgrade(&ingress);

        let builder = HostBuilder::new()
            .with_engine(engine.clone())
            .with_http_handler(Arc::clone(&ingress));
        let native_plugins = builder.native_plugins();
        let http_handler = builder.http_handler();

        let plugin = ComponentHostPlugin::builder()
            .id("http-egress-plugin")
            .wasm(EGRESS_PLUGIN_WASM)
            .engine(engine)
            .native_plugins(native_plugins)
            .allowed_hosts(vec!["example.com".parse()?].into())
            .maybe_http_handler(http_handler.as_ref().map(Arc::downgrade))
            .build()
            .await
            .context("the egress plugin should link cleanly")?;
        let plugin = Arc::new(plugin);
        let weak_plugin = Arc::downgrade(&plugin);

        let host = builder.with_plugin(plugin)?.build()?;
        let host = host.start().await.context("failed to start the host")?;
        drop(ingress);

        // Binding is what puts the plugin in the workload's component
        // metadata, which is what closes the loop; the call itself is not
        // needed and would reach the network.
        host.workload_start(component_workload_request(
            "http-egress-plugin-caller",
            "caller",
            CALLER_WASM,
            LocalResources::default(),
            vec![
                common::http_incoming_handler_interface("caller", None),
                WitInterface {
                    namespace: "acme".to_string(),
                    package: "httpegress".to_string(),
                    interfaces: ["fetch".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.1.0")?),
                    config: HashMap::new(),
                    name: None,
                },
            ],
        ))
        .await
        .context("the caller workload should bind the plugin")?;

        match stop_first {
            true => host.stop().await.context("failed to stop the host")?,
            false => drop(host),
        }
        (weak_ingress, weak_plugin)
    };

    Ok(handles)
}

/// Dropped without being stopped, the host still releases its ingress: the
/// plugin's handle on it is weak.
#[cfg(feature = "host-component-plugins")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_host_with_a_component_plugin_releases_its_ingress() -> Result<()> {
    let (ingress, _plugin) = plugin_host_release(false).await?;
    assert!(
        ingress.upgrade().is_none(),
        "the ingress outlived its host: a bound component plugin still holds it"
    );
    Ok(())
}

/// Stopping the host first releases the workload graph too — the plugin the
/// workload bound is freed, which freeing the ingress alone would not show.
#[cfg(feature = "host-component-plugins")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_host_releases_the_workloads_it_routed() -> Result<()> {
    let (ingress, plugin) = plugin_host_release(true).await?;
    assert!(
        plugin.upgrade().is_none(),
        "a stopped host kept the plugin its workload bound, so the workload, \
         its `InstancePre` and its compiled components are still alive too"
    );
    assert!(
        ingress.upgrade().is_none(),
        "the ingress outlived its stopped host"
    );
    Ok(())
}
