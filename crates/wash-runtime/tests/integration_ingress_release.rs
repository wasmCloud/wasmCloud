//! An ingress holds every routable workload, and everything reachable from a
//! workload can reach the ingress back. Held strongly both ways, nothing frees
//! either — so each way back is weak, and each gets a test here, because no one
//! of them reaches the others: a workload's own handle and its stores' egress
//! hooks, an ephemeral linked call, and a bound host component plugin.
//!
//! The rest cover teardown: a stop must serve its drain out, and must let go of
//! what it routed once that drain ends.

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

/// Build an ingress and a started host around it.
///
/// `DevRouter`, so a plain GET reaches the workload without these tests also
/// having to arrange hostname routing.
async fn host_with_ingress() -> Result<(std::net::SocketAddr, Arc<Host>, Arc<Ingress<DevRouter>>)> {
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .build()
        .context("failed to build the engine")?;
    let ingress = Arc::new(
        Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?)
            .await
            .context("failed to bind an ingress")?,
    );
    let addr = ingress.addr();
    let host = Host::builder()
        .with_engine(engine)
        .with_http_handler(Arc::clone(&ingress) as Arc<dyn HostHandler>)
        .build()
        .context("failed to build the host")?;
    let host = host.start().await.context("failed to start the host")?;
    Ok((addr, host, ingress))
}

/// [`host_with_ingress`], with the ingress handed back only weakly and no
/// strong one left but the host's — so what the `Weak` measures afterwards is
/// the host's own reachability.
async fn host_with_weak_ingress() -> Result<(std::net::SocketAddr, Arc<Host>, Weak<dyn HostHandler>)>
{
    let (addr, host, ingress) = host_with_ingress().await?;
    let weak = Arc::downgrade(&(ingress as Arc<dyn HostHandler>));
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
async fn a_dropped_host_releases_its_ingress() -> Result<()> {
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

        // Dropped rather than stopped: letting a host go without stopping it
        // is the case a strong back-reference strands.
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

        // Dropped rather than stopped: letting a host go without stopping it
        // is the case a strong back-reference strands.
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

        if stop_first {
            Arc::clone(&host)
                .stop()
                .await
                .context("failed to stop the host")?;
        }
        drop(host);
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

/// Stopping before dropping must release just as thoroughly, and this measures
/// the workload graph rather than the table that held it: the plugin is
/// reachable only through the workload that bound it, so a live plugin means a
/// live `InstancePre` and the components it compiled.
#[cfg(feature = "host-component-plugins")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_then_dropped_host_releases_the_plugin_its_workload_bound() -> Result<()> {
    let (ingress, plugin) = plugin_host_release(true).await?;
    assert!(
        plugin.upgrade().is_none(),
        "the host component plugin outlived its host, so the workload that \
         bound it and everything that workload compiled are still alive too"
    );
    assert!(ingress.upgrade().is_none(), "the ingress outlived its host");
    Ok(())
}

/// The far end of the drain: once it finishes, a host that is stopped but still
/// held lets go of the workloads it routed. Asserted while the host and its
/// ingress are both deliberately still alive — freeing them would prove nothing
/// about what they held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drained_host_releases_the_workloads_it_routed() -> Result<()> {
    let (addr, host, ingress) = host_with_ingress().await?;

    host.workload_start(component_workload_request(
        "http-handler-p3.wasm",
        "ingress-drained",
        HTTP_HANDLER_P3_WASM,
        LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            ..Default::default()
        },
        http_only_host_interfaces("ingress-drained"),
    ))
    .await
    .context("failed to start the workload")?;
    serve_one(&addr).await?;
    assert_eq!(
        ingress.routed_workloads().await,
        1,
        "the workload must be routable before the drain"
    );

    Arc::clone(&host)
        .stop()
        .await
        .context("failed to stop the host")?;

    // The release is detached, because a drain outlasts the call that starts
    // it; `reqwest` holds its pooled connection until the client is dropped.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while ingress.routed_workloads().await != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .context("the routing tables were still populated long after the drain")?;

    // The host and its ingress are both still bound here, so what the wait
    // above measured was release and not teardown.
    Ok(())
}

/// Stopping a host ends its accept loop but must not withdraw the routes under
/// connections it already has: a client holding a keep-alive connection goes on
/// sending, and answering those 404 hands an upstream proxy a response to
/// forward rather than a reason to try another replica. That drain window is
/// what clearing the routing tables in `stop()` closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_host_still_serves_an_established_connection() -> Result<()> {
    let (addr, host, ingress) = host_with_ingress().await?;

    host.workload_start(component_workload_request(
        "http-handler-p3.wasm",
        "ingress-drain",
        HTTP_HANDLER_P3_WASM,
        LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            ..Default::default()
        },
        http_only_host_interfaces("ingress-drain"),
    ))
    .await
    .context("failed to start the workload")?;

    // One client, reused, so the second request rides the connection the first
    // opened rather than dialing a listener that has stopped accepting.
    let client = reqwest::Client::builder()
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()?;
    let first = client.get(format!("http://{addr}/")).send().await?;
    assert!(
        first.status().is_success(),
        "the workload must serve before the drain"
    );
    first.bytes().await?;

    Arc::clone(&host)
        .stop()
        .await
        .context("failed to stop the host")?;
    // Separates the two ways the request below can fail: a withdrawn route, or
    // a connection the client did not reuse.
    assert_eq!(
        ingress.routed_workloads().await,
        1,
        "stop must leave the route in place for the drain"
    );

    let during_drain = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .context("a request on an established connection must still be answered")?;
    assert!(
        during_drain.status().is_success(),
        "a draining host withdrew the route under an established connection and \
         answered {} — an upstream proxy forwards that to the client rather than \
         retrying another replica",
        during_drain.status()
    );
    Ok(())
}
