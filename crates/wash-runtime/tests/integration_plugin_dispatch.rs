//! Integration tests for plugin-initiated dispatch: a host-native plugin that
//! *imports* `acme:tasks/runner` and calls into whichever workload item exports
//! it, the way a plugin consuming an external event stream pushes work into a
//! workload.
//!
//! Every reply the fixture sends counts the calls its own instance has served,
//! so the assertions here are about WHERE the host ran each call:
//!
//! - a component with a warm pool serves them all on one instance (`1, 2, 3`),
//!   which is what `poolSize`/`maxConcurrency` are for;
//! - a component that keeps no instances gets a fresh one per call (`1, 1, 1`);
//! - a service serves them on the single instance it already is, concurrently.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::sync::{Mutex, oneshot};
use wasmtime::component::{Accessor, Instance};

use wash_runtime::engine::Engine;
use wash_runtime::engine::ctx::SharedCtx;
use wash_runtime::engine::dispatch::{DispatchTarget, GuestCall, GuestCallFuture};
use wash_runtime::engine::workload::ResolvedWorkload;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::{HostPlugin, WitInterfaces};
use wash_runtime::types::{
    Component, LocalResources, Service, Workload, WorkloadStartRequest, WorkloadStopRequest,
};
use wash_runtime::wit::{WitInterface, WitWorld};

const DISPATCH_TARGET_WASM: &[u8] = include_bytes!("wasm/dispatch_target.wasm");

/// A typed view over the fixture's `acme:tasks/runner` export, built against
/// whichever instance the host picks for a call. The plugin owns the call; the
/// host only owns the instance it runs on.
mod bindings {
    wasmtime::component::bindgen!({
        world: "runner-view",
        inline: "
            package wasmcloud:dispatchtest@0.1.0;

            world runner-view {
                export acme:tasks/runner@0.1.0;
            }

            package acme:tasks@0.1.0 {
                interface runner {
                    run: async func(message: string) -> string;
                }
            }
        ",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

/// The interface the plugin drives and the fixture exports. One manifest entry
/// covers it, because interface matching looks at an item's exports as well as
/// its imports.
fn acme_tasks_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "tasks".to_string(),
        interfaces: ["runner".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// One `runner.run` call, as the plugin hands it to the host: the message to
/// send and the channel its reply comes back on.
struct RunCall {
    message: String,
    reply: oneshot::Sender<Result<String>>,
}

impl GuestCall for RunCall {
    fn describe(&self) -> &str {
        "acme:tasks/runner#run"
    }

    fn call<'a>(
        self: Box<Self>,
        accessor: &'a Accessor<SharedCtx>,
        instance: Instance,
    ) -> GuestCallFuture<'a> {
        Box::pin(async move {
            let view = accessor
                .with(|mut access| bindings::RunnerView::new(&mut access, &instance))
                .map_err(|e| {
                    anyhow::anyhow!(
                        "dispatch target is missing its acme:tasks/runner export: {e:#}"
                    )
                })?;
            // A trap is a call that did not complete, so it is the host's
            // `Err` — which is what retires the instance it ran on. Only an
            // answer the guest actually gave goes back on the plugin's own
            // channel.
            let answer = view
                .acme_tasks_runner()
                .call_run(accessor, self.message)
                .await
                .map_err(|e| anyhow::anyhow!("runner.run trapped: {e:#}"))?;
            let _ = self.reply.send(Ok(answer));
            Ok(None)
        })
    }
}

/// A host-native plugin in the push direction: it serves nothing a workload
/// imports, and exists to call `acme:tasks/runner` on whatever bound it.
#[derive(Default)]
struct TaskPusher {
    /// One dispatch target per workload, resolved while the workload resolved
    /// and held for as long as it runs.
    targets: Mutex<BTreeMap<String, DispatchTarget>>,
}

impl TaskPusher {
    /// Push one unit of work into `workload_id` and wait for the reply.
    async fn run(&self, workload_id: &str, message: &str) -> Result<String> {
        let target = self
            .targets
            .lock()
            .await
            .get(workload_id)
            .cloned()
            .with_context(|| format!("no dispatch target for workload '{workload_id}'"))?;
        let (reply, reply_rx) = oneshot::channel();
        target
            .dispatch(RunCall {
                message: message.to_string(),
                reply,
            })
            .await?;
        reply_rx.await.context("dispatched call sent no reply")?
    }
}

#[async_trait::async_trait]
impl HostPlugin for TaskPusher {
    fn id(&self) -> &'static str {
        "acme-tasks"
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("acme:tasks/runner@0.1.0")]),
            ..Default::default()
        }
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        item_id: &str,
    ) -> anyhow::Result<()> {
        // Resolving the target here, while the workload resolves, is what
        // reserves a service's ingress before it starts running.
        let target = workload.dispatch_target(item_id, "acme-tasks").await?;
        self.targets
            .lock()
            .await
            .insert(workload.id().to_string(), target);
        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        self.targets.lock().await.remove(workload_id);
        Ok(())
    }
}

/// A host running nothing but the plugin under test: the fixture imports only
/// WASI, so no other plugin is involved in reaching it.
async fn start_host() -> Result<(Arc<TaskPusher>, impl HostApi)> {
    let plugin = Arc::new(TaskPusher::default());
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::clone(&plugin) as Arc<dyn HostPlugin>)?
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((plugin, host))
}

/// The fixture deployed as an ordinary component, with the instance limits the
/// caller wants to observe.
fn component_workload(
    workload_id: &str,
    pool_size: i32,
    max_concurrency: i32,
) -> WorkloadStartRequest {
    component_workload_with(workload_id, pool_size, max_concurrency, 0)
}

/// As [`component_workload`], with an invocation budget per instance.
fn component_workload_with(
    workload_id: &str,
    pool_size: i32,
    max_concurrency: i32,
    max_invocations: i32,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: workload_id.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "dispatch-target".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(DISPATCH_TARGET_WASM),
                local_resources: LocalResources::default(),
                pool_size,
                max_invocations,
                max_concurrency,
                ..Default::default()
            }],
            host_interfaces: vec![acme_tasks_interface()],
            volumes: vec![],
        },
    }
}

/// The same fixture deployed as the workload's long-lived service, with nothing
/// in its environment and no restarts.
fn service_workload(workload_id: &str) -> WorkloadStartRequest {
    service_workload_with(workload_id, HashMap::new(), 0)
}

/// The same fixture deployed as the workload's long-lived service, with the
/// environment and restart budget the caller wants.
fn service_workload_with(
    workload_id: &str,
    environment: HashMap<String, String>,
    max_restarts: u64,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: workload_id.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(DISPATCH_TARGET_WASM),
                local_resources: LocalResources {
                    environment,
                    ..LocalResources::default()
                },
                max_restarts,
            }),
            components: vec![],
            host_interfaces: vec![acme_tasks_interface()],
            volumes: vec![],
        },
    }
}

async fn stop(host: &impl HostApi, workload_id: &str) {
    let _ = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.to_string(),
        })
        .await;
}

/// A component that keeps instances warm serves every dispatched call on one of
/// them: the counts climb, which they only can if the instance outlived the
/// call before it. This is what `poolSize` buys a push-mode plugin.
#[tokio::test]
async fn test_dispatch_runs_on_a_warm_component_instance() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload("warm", 1, 4))
        .await?;

    assert_eq!(plugin.run("warm", "a").await?, "a:1");
    assert_eq!(plugin.run("warm", "b").await?, "b:2");
    assert_eq!(plugin.run("warm", "c").await?, "c:3");

    stop(&host, "warm").await;
    Ok(())
}

/// An instance stops taking dispatched calls once it has served
/// `maxInvocations` of them, and the one that replaces it starts over.
///
/// Read through the replacement rather than the retired instance: a retired
/// instance is drained and dropped, so nothing can ask it what it served. The
/// third call answering `1` is the whole proof — the budget was spent, and the
/// component the plugin drives is not one long-lived instance forever.
#[tokio::test]
async fn test_a_dispatched_call_spends_the_instances_invocation_budget() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload_with("budget", 1, 1, 2))
        .await?;

    assert_eq!(plugin.run("budget", "a").await?, "a:1");
    assert_eq!(plugin.run("budget", "b").await?, "b:2");
    assert_eq!(
        plugin.run("budget", "c").await?,
        "c:1",
        "the third call must land on a fresh instance, not the one whose budget is spent"
    );

    stop(&host, "budget").await;
    Ok(())
}

/// A dispatched call that traps takes the warm instance it ran on with it: the
/// next call is served by a fresh one, counting from zero again.
///
/// The trap faults the whole store, so the driver ends and the pool reaps its
/// handle — a `GuestCall` returning `Err` retires the instance for the failures
/// that do *not* fault it, and reporting the trap that way is what keeps the two
/// consistent.
#[tokio::test]
async fn test_a_trapping_dispatch_takes_its_warm_instance_with_it() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload("trapped", 1, 1))
        .await?;

    assert_eq!(plugin.run("trapped", "a").await?, "a:1");
    assert!(
        plugin.run("trapped", "trap").await.is_err(),
        "a trapped call must be reported to the dispatcher, not answered"
    );
    assert_eq!(
        plugin.run("trapped", "b").await?,
        "b:1",
        "the next call must land on a fresh instance, not the one that trapped"
    );

    stop(&host, "trapped").await;
    Ok(())
}

/// A component that asked for no warm instances still gets its calls, each on
/// an instance built and dropped for it — the count restarts every time.
#[tokio::test]
async fn test_dispatch_to_an_unpooled_component_builds_an_instance_per_call() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload("cold", 0, 0))
        .await?;

    assert_eq!(plugin.run("cold", "a").await?, "a:1");
    assert_eq!(plugin.run("cold", "b").await?, "b:1");

    stop(&host, "cold").await;
    Ok(())
}

/// A workload whose SERVICE exports the interface is dispatched to on the
/// instance already running — not silently skipped, and not instantiated a
/// second time. The climbing counts are the proof: a service has no per-call
/// instantiation to fall back on, so a fresh instance would answer `1` forever.
#[tokio::test]
async fn test_dispatch_reaches_a_service() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(service_workload("svc")).await?;

    assert_eq!(plugin.run("svc", "a").await?, "a:1");
    assert_eq!(plugin.run("svc", "b").await?, "b:2");
    assert_eq!(plugin.run("svc", "c").await?, "c:3");

    stop(&host, "svc").await;
    Ok(())
}

/// Calls dispatched at once to a service are served concurrently on its one
/// instance, so the counts they report are exactly `1..=n` — one instance, n
/// calls, no duplicates.
#[tokio::test]
async fn test_concurrent_dispatches_share_the_service_instance() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(service_workload("svc-concurrent"))
        .await?;

    let messages: Vec<String> = (0..4).map(|i| format!("m{i}")).collect();
    let replies =
        futures::future::join_all(messages.iter().map(|m| plugin.run("svc-concurrent", m))).await;

    let mut counts = replies
        .into_iter()
        .map(|reply| {
            let reply = reply?;
            let (_, count) = reply
                .rsplit_once(':')
                .context("reply is not `{message}:{count}`")?;
            count.parse::<u32>().context("reply count is not a number")
        })
        .collect::<Result<Vec<_>>>()?;
    counts.sort_unstable();
    assert_eq!(counts, vec![1, 2, 3, 4]);

    stop(&host, "svc-concurrent").await;
    Ok(())
}

/// A service reached only by dispatched calls is still supervised as the plain
/// p3 service it would otherwise be: its `cli/run` failing spends the workload's
/// restart budget and, at zero, ends the service. Binding a plugin that pushes
/// into it must not quietly turn `maxRestarts` into "run forever with dead
/// `cli/run` work".
#[tokio::test]
async fn test_a_dispatch_only_service_still_spends_its_restart_budget() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(service_workload_with(
        "svc-run-fails",
        HashMap::from([("RUN_EXIT".to_string(), "error".to_string())]),
        0,
    ))
    .await?;

    // The first dispatch may still land — `cli/run` and the ingress start
    // together — so wait for the incarnation to end rather than asserting on a
    // single call.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match plugin.run("svc-run-fails", "x").await {
            Err(_) => break,
            Ok(_) if std::time::Instant::now() >= deadline => {
                panic!(
                    "service kept serving dispatched calls after cli/run failed with no restarts left"
                )
            }
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }

    stop(&host, "svc-run-fails").await;
    Ok(())
}

/// The same, for a `cli/run` that *traps* rather than answering with an error.
/// A plain p3 service spends a restart on either, so a dispatch-only one — which
/// reaches the trigger driver only because a plugin claimed it — has to as well.
#[tokio::test]
async fn test_a_dispatch_only_service_spends_its_budget_on_a_cli_run_trap() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(service_workload_with(
        "svc-run-traps",
        HashMap::from([("RUN_EXIT".to_string(), "trap".to_string())]),
        0,
    ))
    .await?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match plugin.run("svc-run-traps", "x").await {
            Err(_) => break,
            Ok(_) if std::time::Instant::now() >= deadline => {
                panic!(
                    "service kept serving dispatched calls after cli/run trapped with no \
                     restarts left"
                )
            }
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }

    stop(&host, "svc-run-traps").await;
    Ok(())
}

/// A target outlives the workload it names — a plugin holds one until it is
/// unbound, which happens after teardown starts — so a dispatch that arrives
/// once the workload has stopped is refused. Otherwise it would build (and, for
/// a pooled component, park) a fresh instance in a workload the host has already
/// torn down.
#[tokio::test]
async fn test_dispatch_after_the_workload_stops_is_refused() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload("stopped", 1, 1))
        .await?;
    assert_eq!(plugin.run("stopped", "a").await?, "a:1");

    // Reach past the plugin's own unbind bookkeeping to the target it held
    // while the workload ran: the host must refuse the call on its own, not
    // rely on a plugin having tidied up first.
    let target = plugin
        .targets
        .lock()
        .await
        .get("stopped")
        .cloned()
        .expect("plugin bound the workload");
    stop(&host, "stopped").await;

    let (reply, _reply_rx) = oneshot::channel();
    let err = target
        .dispatch(RunCall {
            message: "after".to_string(),
            reply,
        })
        .await
        .expect_err("a stopped workload must take no more dispatched calls");
    assert!(
        err.to_string().contains("has stopped"),
        "unexpected error: {err:#}"
    );

    Ok(())
}

/// An item the workload does not have is refused when the target is resolved,
/// rather than accepted and dispatched into nothing.
#[tokio::test]
async fn test_an_unknown_item_is_refused() -> Result<()> {
    let (plugin, host) = start_host().await?;
    host.workload_start(component_workload("unknown-item", 1, 1))
        .await?;

    let target = plugin.targets.lock().await;
    let workload = target
        .get("unknown-item")
        .expect("plugin bound the workload")
        .workload()
        .clone();
    drop(target);

    let err = workload
        .dispatch_target("no-such-item", "acme-tasks")
        .await
        .expect_err("a missing item must not resolve as a dispatch target");
    assert!(
        err.to_string().contains("no item 'no-such-item'"),
        "unexpected error: {err:#}"
    );

    stop(&host, "unknown-item").await;
    Ok(())
}
