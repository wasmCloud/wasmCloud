use super::contract_tests::{API, EXPORTER, component, engine, resolve, service};
use super::*;
use crate::engine::pool_share::{PoolCost, PoolShare, Share};
use crate::observability::{MeterKind, Meters, WorkloadIdentity};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const MIDDLE: &str = "test:workload/middle@0.1.0";
const ROOT: &str = "test:workload/root@0.1.0";

fn forwarder(name: &str, import: &str, export: &str) -> WorkloadComponent {
    component(
        name,
        &format!(
            r#"(component
        (import "{import}" (instance $api (export "f" (func (result u32)))))
        (alias export $api "f" (func $f))
        (core func $f (canon lower (func $f)))
        (core module $m
            (import "host" "f" (func $f (result i32)))
            (memory 1)
            (func (export "f") (result i32) call $f))
        (core instance $m (instantiate $m (with "host" (instance (export "f" (func $f))))))
        (func $forward (result u32) (canon lift (core func $m "f")))
        (instance $forwarded (export "f" (func $forward)))
        (export "{export}" (instance $forwarded)))"#
        ),
    )
}

async fn linked_workload() -> anyhow::Result<(ResolvedWorkload, [String; 3])> {
    let leaf = component(
        "leaf",
        &EXPORTER.replace("(core module $m", "(core module $m (memory 1)"),
    );
    let middle = forwarder("middle", API, MIDDLE);
    let root = forwarder("root", MIDDLE, ROOT);
    let ids = [
        root.id().to_string(),
        middle.id().to_string(),
        leaf.id().to_string(),
    ];
    Ok((resolve(None, [root, leaf, middle]).await?, ids))
}

fn pool_share() -> PoolShare {
    pool_share_with_memories(64)
}

fn pool_share_with_memories(memories: u32) -> PoolShare {
    PoolShare::new(
        super::super::pool_usage::PoolTotals {
            memories,
            tables: 64,
            core_instances: 320,
            component_instances: 64,
            stacks: 64,
            gc_heaps: 64,
        },
        Share {
            numerator: 1,
            denominator: 1,
            floor: 0,
            cap: 64,
        },
        "workload tests",
    )
}

fn warm_policy() -> InstancePolicy {
    InstancePolicy::Warm {
        pool_size: NonZeroUsize::new(2).unwrap(),
        max_invocations: None,
        max_concurrency: NonZeroUsize::new(3).unwrap(),
        reclaim: None,
    }
}

async fn call_export(
    pre: &InstancePre<SharedCtx>,
    store: &mut wasmtime::Store<SharedCtx>,
    interface: &str,
) -> anyhow::Result<u32> {
    let instance = pre.instantiate_async(&mut *store).await?;
    let (_, interface) = instance
        .get_export(&mut *store, None, interface)
        .context("interface missing")?;
    let (_, index) = instance
        .get_export(&mut *store, Some(&interface), "f")
        .context("function missing")?;
    let function = instance
        .get_func(&mut *store, index)
        .context("function missing")?
        .typed::<(), (u32,)>(&*store)?;
    let (value,) = function.call_async(&mut *store, ()).await?;
    Ok(value)
}

#[tokio::test]
async fn admission_charges_the_transitive_store_once_and_shares_its_gc_heap() -> anyhow::Result<()>
{
    let (resolved, ids) = linked_workload().await?;
    for gc_component in [None, Some(0), Some(1), Some(2)] {
        let mut components = resolved.components.write().await;
        for (index, id) in ids.iter().enumerate() {
            components.get_mut(id.as_str()).unwrap().metadata.may_use_gc =
                gc_component == Some(index);
        }
        drop(components);
        let cost = resolved.admit_store(&ids[0], &pool_share()).await?;
        assert_eq!(
            cost,
            PoolCost {
                memories: 3 + u32::from(gc_component.is_some()),
                tables: 0,
                core_instances: 15,
                component_instances: 3,
                stacks: 1,
                gc_heaps: u32::from(gc_component.is_some()),
            }
        );
    }
    for component in resolved.components.write().await.values_mut() {
        component.metadata.may_use_gc = true;
    }
    assert_eq!(
        resolved.admit_store(&ids[0], &pool_share()).await?.gc_heaps,
        1
    );
    let error = resolved
        .admit_store(&ids[0], &pool_share_with_memories(2))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("memories"), "{error:#}");
    assert!(
        resolved
            .admit_store("unknown", &pool_share())
            .await
            .unwrap_err()
            .to_string()
            .contains("component ID not found")
    );
    Ok(())
}

#[tokio::test]
async fn stores_install_transitive_contexts_and_execute_the_linked_export() -> anyhow::Result<()> {
    let (resolved, ids) = linked_workload().await?;
    let mut store = resolved.new_store(&ids[0]).await?;
    assert_eq!(store.data().active_ctx.component_id.as_ref(), ids[0]);
    for id in &ids[1..] {
        let context = &store.data().contexts[id.as_str()];
        assert_eq!(context.workload_id.as_ref(), "contracts");
        assert_eq!(context.store_id, store.data().active_ctx.store_id);
        assert!(store.data().exporter_instances.contains_key(id.as_str()));
    }
    assert_eq!(store.data().exporter_instances.len(), 2);
    let pre = resolved.instantiate_pre(&ids[0]).await?;
    assert_eq!(call_export(&pre, &mut store, ROOT).await?, 42);
    assert_eq!(store.data().active_ctx.component_id.as_ref(), ids[0]);
    assert!(
        resolved
            .new_store("unknown")
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("component ID not found")
    );
    Ok(())
}

#[tokio::test]
async fn store_metering_uses_the_host_setting_and_the_supplied_or_manifest_identity()
-> anyhow::Result<()> {
    let (mut resolved, ids) = linked_workload().await?;
    let supplied = WorkloadIdentity::new("override-ns", "override-workload", "override-item");
    for kind in [MeterKind::Off, MeterKind::Duration] {
        resolved.invocation = Meters::new(kind).invocation;
        for identity in [None, Some(&supplied)] {
            let store = resolved.new_store_as(&ids[0], identity).await?;
            let stamp = store.data().executed.identity();
            if kind == MeterKind::Off {
                assert!(stamp.is_none());
            } else {
                let stamp = stamp.unwrap();
                let expected = identity
                    .cloned()
                    .unwrap_or_else(|| WorkloadIdentity::new("test", "contracts", "root"));
                assert_eq!(stamp.namespace, expected.namespace);
                assert_eq!(stamp.name, expected.name);
                assert_eq!(stamp.component, expected.component);
            }
        }
    }
    let service = service("(component)");
    let service_id = service.id().to_string();
    let resolved = resolve(Some(service), []).await?;
    assert_eq!(
        resolved
            .component_identity(&service_id)
            .await
            .component
            .as_ref(),
        "service"
    );
    assert_eq!(
        resolved
            .component_identity("unknown")
            .await
            .component
            .as_ref(),
        "unknown"
    );
    Ok(())
}

#[tokio::test]
async fn pool_and_dispatch_selection_require_every_linked_component_to_opt_in() -> anyhow::Result<()>
{
    let (resolved, ids) = linked_workload().await?;
    for ephemeral in [None, Some(0), Some(1), Some(2)] {
        let mut components = resolved.components.write().await;
        for (index, id) in ids.iter().enumerate() {
            components.get_mut(id.as_str()).unwrap().instances =
                Arc::new(InstancePool::new(if ephemeral == Some(index) {
                    InstancePolicy::Ephemeral
                } else {
                    warm_policy()
                }));
        }
        let root_pool = Arc::clone(&components[ids[0].as_str()].instances);
        drop(components);
        let pool = resolved.instance_pool_for_component(&ids[0]).await;
        assert_eq!(pool.is_some(), ephemeral.is_none());
        if let Some(pool) = pool {
            assert!(Arc::ptr_eq(&pool, &root_pool));
        }
        let (_, _, dispatch_pool) = resolved.component_dispatch(&ids[0]).await?;
        assert_eq!(dispatch_pool.is_some(), ephemeral.is_none());
        assert_eq!(
            resolved
                .warm_instance_policy(&ids[0])
                .await
                .keeps_instances_warm(),
            ephemeral.is_none()
        );
        assert_eq!(
            resolved.call_concurrency(&ids[0]).await,
            if ephemeral == Some(0) { 1 } else { 6 }
        );
        resolved.dispatch_target(&ids[0], "test-dispatch").await?;
    }
    assert!(
        resolved
            .instance_pool_for_component("unknown")
            .await
            .is_none()
    );
    assert!(
        !resolved
            .warm_instance_policy("unknown")
            .await
            .keeps_instances_warm()
    );
    assert_eq!(resolved.call_concurrency("unknown").await, 1);
    let error = resolved
        .dispatch_target("unknown", "test-dispatch")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no item 'unknown'"));
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Callback {
    Bind,
    Item,
    Resolved,
}

struct RecordingPlugin {
    id: &'static str,
    interface: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    fail: Option<Callback>,
    unbind_failures: AtomicUsize,
    unbind_gate: Option<Arc<tokio::sync::Semaphore>>,
    entered: tokio::sync::Notify,
    overrides: WitWorld,
}

impl RecordingPlugin {
    fn new(id: &'static str, interface: &'static str, events: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id,
            interface,
            events: Arc::clone(events),
            fail: None,
            unbind_failures: AtomicUsize::new(0),
            unbind_gate: None,
            entered: tokio::sync::Notify::new(),
            overrides: WitWorld::default(),
        }
    }

    fn event(&self, event: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("{}:{event}", self.id));
    }
    fn callback(&self, callback: Callback) -> anyhow::Result<()> {
        self.event(&format!("{callback:?}"));
        ensure!(
            self.fail != Some(callback),
            "{} refused {callback:?}",
            self.id
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostPlugin for RecordingPlugin {
    fn id(&self) -> &'static str {
        self.id
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::new(),
            exports: HashSet::from([WitInterface::from(self.interface)]),
        }
    }
    fn wasi_overrides(&self) -> WitWorld {
        self.overrides.clone()
    }
    async fn on_workload_bind(
        &self,
        _: &UnresolvedWorkload,
        _: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        self.callback(Callback::Bind)
    }
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        self.callback(Callback::Item)?;
        item.linker()?
            .instance(self.interface)?
            .func_wrap("f", |_, (): ()| Ok(()))?;
        Ok(())
    }
    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        id: &str,
    ) -> anyhow::Result<()> {
        if workload.is_service_item(id) {
            assert!(workload.service.as_ref().unwrap().metadata.is_sealed());
        } else {
            assert!(workload.components.read().await[id].metadata.is_sealed());
        }
        self.callback(Callback::Resolved)
    }
    async fn on_workload_unbind(&self, _: &str, _: WitInterfaces<'_>) -> anyhow::Result<()> {
        self.entered.notify_one();
        if let Some(gate) = &self.unbind_gate {
            gate.acquire().await?.forget();
        }
        self.event("Unbind");
        let failed = self
            .unbind_failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        ensure!(!failed, "{} cleanup failed", self.id);
        Ok(())
    }
}

fn requesting(interfaces: &[&str], as_service: bool) -> UnresolvedWorkload {
    let imports = interfaces
        .iter()
        .map(|interface| format!(r#"(import "{interface}" (instance (export "f" (func))))"#))
        .collect::<String>();
    let wat = format!("(component {imports})");
    UnresolvedWorkload::new(
        "contracts",
        "contracts",
        "test",
        as_service.then(|| service(&wat)),
        if as_service {
            vec![]
        } else {
            vec![component("caller", &wat)]
        },
        interfaces
            .iter()
            .map(|interface| WitInterface::from(*interface))
            .collect(),
    )
}

#[tokio::test]
async fn every_failed_plugin_callback_releases_component_and_service_bindings() -> anyhow::Result<()>
{
    for callback in [Callback::Bind, Callback::Item, Callback::Resolved] {
        for as_service in [false, true] {
            for journaled in [false, true] {
                let events = Arc::default();
                let mut plugin = RecordingPlugin::new("recording", API, &events);
                plugin.fail = Some(callback);
                let plugins =
                    HashMap::from([("recording", Arc::new(plugin) as Arc<dyn HostPlugin>)]);
                let resources = WorkloadResources::default();
                let error = requesting(&[API], as_service)
                    .resolve_for_start(
                        Some(&plugins),
                        &Default::default(),
                        &crate::host::HostRef::detached(),
                        &Default::default(),
                        journaled.then_some(&resources),
                    )
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("refused"), "{error:#}");
                if journaled {
                    assert!(
                        !events
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|e| e.ends_with(":Unbind"))
                    );
                    resources.release("contracts").await?;
                    resources.release("contracts").await?;
                }
                assert_eq!(
                    events
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|e| e.ends_with(":Unbind"))
                        .count(),
                    1,
                    "{callback:?}, service={as_service}, journaled={journaled}"
                );
            }
        }
    }
    Ok(())
}

async fn journaled_workload(
    plugins: &[Arc<RecordingPlugin>],
) -> anyhow::Result<(ResolvedWorkload, WorkloadResources)> {
    let registered = plugins
        .iter()
        .map(|p| (p.id, Arc::clone(p) as Arc<dyn HostPlugin>))
        .collect();
    let interfaces = plugins.iter().map(|p| p.interface).collect::<Vec<_>>();
    let resources = WorkloadResources::default();
    let mut resolved = requesting(&interfaces, false)
        .resolve_for_start(
            Some(&registered),
            &Default::default(),
            &crate::host::HostRef::detached(),
            &Default::default(),
            Some(&resources),
        )
        .await?;
    resources.commit(&mut resolved);
    Ok((resolved, resources))
}

#[tokio::test]
async fn concurrent_cleanup_retries_only_the_failed_plugin_in_reverse_bind_order()
-> anyhow::Result<()> {
    let events = Arc::default();
    let first = Arc::new(RecordingPlugin::new("first", API, &events));
    let last = Arc::new(RecordingPlugin::new("last", MIDDLE, &events));
    last.unbind_failures.store(1, Ordering::SeqCst);
    let (resolved, _) = journaled_workload(&[first, last]).await?;
    assert_eq!(resolved.teardown_bindings.len(), 2);
    let resources = WorkloadResources::for_teardown(&resolved);
    events.lock().unwrap().clear();
    let (first, second) = tokio::join!(
        resources.release("contracts"),
        resources.release("contracts")
    );
    assert!(first.is_err());
    second?;
    resources.release("contracts").await?;
    assert_eq!(
        *events.lock().unwrap(),
        ["last:Unbind", "first:Unbind", "last:Unbind"]
    );
    assert!(resolved.released());
    Ok(())
}

#[tokio::test]
async fn cancelled_cleanup_retains_the_unfinished_binding() -> anyhow::Result<()> {
    let events = Arc::default();
    let first = Arc::new(RecordingPlugin::new("first", API, &events));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut last = RecordingPlugin::new("last", MIDDLE, &events);
    last.unbind_gate = Some(Arc::clone(&gate));
    let last = Arc::new(last);
    let (resolved, resources) = journaled_workload(&[first, Arc::clone(&last)]).await?;
    events.lock().unwrap().clear();
    let task = tokio::spawn({
        let resources = resources.clone();
        async move { resources.release("contracts").await }
    });
    tokio::time::timeout(Duration::from_secs(5), last.entered.notified()).await?;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.add_permits(1);
    resources.release("contracts").await?;
    assert_eq!(*events.lock().unwrap(), ["last:Unbind", "first:Unbind"]);
    assert!(resolved.released());
    Ok(())
}

#[test]
fn trigger_ingresses_match_the_claims_and_receive_fresh_channels() -> anyhow::Result<()> {
    use crate::host::trigger_service::Ingress;
    for http in [false, true] {
        for messaging in [false, true] {
            for guest in [false, true] {
                let calls = Arc::new(ServiceCalls::default());
                let _claim = guest.then(|| calls.claim()).transpose()?;
                calls.mark_started();
                let (first, http_before, messaging_before) =
                    build_trigger_ingresses(http, messaging, &calls);
                assert_eq!(
                    first
                        .iter()
                        .filter(|i| matches!(i, Ingress::Http(_)))
                        .count(),
                    usize::from(http)
                );
                assert_eq!(
                    first
                        .iter()
                        .filter(|i| matches!(i, Ingress::Messaging(_)))
                        .count(),
                    usize::from(messaging)
                );
                assert_eq!(
                    first
                        .iter()
                        .filter(|i| matches!(i, Ingress::Guest(_)))
                        .count(),
                    usize::from(guest)
                );
                let (second, http_after, messaging_after) =
                    build_trigger_ingresses(http, messaging, &calls);
                assert_eq!(second.len(), first.len());
                drop(first);
                if http {
                    let before = http_before.unwrap();
                    let after = http_after.unwrap();
                    assert!(before.is_closed());
                    assert!(!after.is_closed());
                    assert!(!before.same_channel(&after));
                    assert_eq!(after.capacity(), INGRESS_BACKLOG);
                } else {
                    assert!(http_before.is_none() && http_after.is_none());
                }
                if messaging {
                    let before = messaging_before.unwrap();
                    let after = messaging_after.unwrap();
                    assert!(before.is_closed());
                    assert!(!after.is_closed());
                    assert!(!before.same_channel(&after));
                    assert_eq!(after.capacity(), INGRESS_BACKLOG);
                } else {
                    assert!(messaging_before.is_none() && messaging_after.is_none());
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Outcome {
    Success,
    Error,
    Trap,
}

type ServiceRuns = Arc<Mutex<Vec<(String, u32, String)>>>;

fn p3_service(
    outcome: Outcome,
    restarts: u64,
) -> (WorkloadService, ServiceRuns, Arc<tokio::sync::Notify>) {
    let mut service = service(
        r#"(component
        (import "test:workload/probe@0.1.0" (instance $host
            (export "probe" (func (param "count" u32) (result u32)))))
        (alias export $host "probe" (func $probe))
        (core func $probe (canon lower (func $probe)))
        (core func $return (canon task.return (result (result))))
        (core func $reply (canon task.return (result u32)))
        (core module $m
            (import "host" "probe" (func $probe (param i32) (result i32)))
            (import "host" "return" (func $return (param i32)))
            (import "host" "reply" (func $reply (param i32)))
            (global $count (mut i32) i32.const 0)
            (func $next (result i32)
                global.get $count i32.const 1 i32.add global.set $count global.get $count)
            (func (export "run") (result i32)
                call $next call $probe call $return i32.const 0)
            (func (export "f") (result i32)
                call $next call $reply i32.const 0)
            (func (export "callback") (param i32 i32 i32) (result i32) unreachable))
        (core instance $m (instantiate $m (with "host" (instance
            (export "probe" (func $probe)) (export "return" (func $return)) (export "reply" (func $reply))))))
        (func $run async (result (result)) (canon lift (core func $m "run")
            async (callback (core func $m "callback"))))
        (instance $cli (export "run" (func $run)))
        (export "wasi:cli/run@0.3.0" (instance $cli))
        (func $f async (result u32) (canon lift (core func $m "f")
            async (callback (core func $m "callback"))))
        (instance $api (export "f" (func $f)))
        (export "test:workload/api@0.1.0" (instance $api)))"#,
    );
    service.max_restarts = restarts;
    let runs: ServiceRuns = Arc::default();
    let entered = Arc::new(tokio::sync::Notify::new());
    service
        .metadata
        .linker()
        .unwrap()
        .instance("test:workload/probe@0.1.0")
        .unwrap()
        .func_wrap("probe", {
            let runs = Arc::clone(&runs);
            let entered = Arc::clone(&entered);
            move |ctx, (count,): (u32,)| -> wasmtime::Result<(u32,)> {
                let identity = ctx
                    .data()
                    .executed
                    .identity()
                    .map(|i| i.component.to_string())
                    .unwrap_or_default();
                runs.lock().unwrap().push((
                    ctx.data().active_ctx.store_id.to_string(),
                    count,
                    identity,
                ));
                entered.notify_one();
                match outcome {
                    Outcome::Success => Ok((0,)),
                    Outcome::Error => Ok((1,)),
                    Outcome::Trap => Err(wasmtime::format_err!("service probe trapped")),
                }
            }
        })
        .unwrap();
    (service, runs, entered)
}

async fn join_service(
    resolved: &mut ResolvedWorkload,
    handle: Arc<JoinHandle<()>>,
) -> anyhow::Result<Result<(), tokio::task::JoinError>> {
    resolved.service_mut().unwrap().handle = None;
    let handle =
        Arc::try_unwrap(handle).map_err(|_| anyhow::anyhow!("service task is still shared"))?;
    Ok(tokio::time::timeout(Duration::from_secs(5), handle).await?)
}

#[tokio::test]
async fn p3_errors_and_traps_spend_the_restart_budget_on_fresh_metered_stores() -> anyhow::Result<()>
{
    for outcome in [Outcome::Error, Outcome::Trap] {
        for trigger in [false, true] {
            for budget in [0, 1, 3] {
                let (service, runs, _) = p3_service(outcome, budget);
                let mut resolved = resolve(Some(service), []).await?;
                let _claim = trigger
                    .then(|| resolved.claim_service_dispatch())
                    .transpose()?;
                let handle = resolved
                    .execute_service()
                    .await?
                    .context("service task missing")?;
                join_service(&mut resolved, handle).await??;
                let runs = runs.lock().unwrap();
                assert_eq!(runs.len(), budget as usize + 1);
                let stores = runs.iter().map(|(id, _, _)| id).collect::<HashSet<_>>();
                assert_eq!(stores.len(), runs.len());
                assert!(
                    runs.iter()
                        .all(|(_, count, identity)| *count == 1 && identity == "service")
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn successful_p3_commands_finish_without_restarting() -> anyhow::Result<()> {
    let (service, runs, _) = p3_service(Outcome::Success, 3);
    let mut resolved = resolve(Some(service), []).await?;
    let handle = resolved
        .execute_service()
        .await?
        .context("service task missing")?;
    join_service(&mut resolved, handle).await??;
    assert_eq!(runs.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_successful_claimed_service_stays_alive_until_teardown() -> anyhow::Result<()> {
    let (service, runs, entered) = p3_service(Outcome::Success, 3);
    let id = service.id().to_string();
    let mut resolved = resolve(Some(service), []).await?;
    let claim = resolved.claim_service_dispatch()?;
    let handle = resolved
        .execute_service()
        .await?
        .context("service task missing")?;
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await?;
    assert!(!handle.is_finished());
    assert!(resolved.dispatch_target(&id, "late-plugin").await.is_err());
    drop(claim);
    assert!(!handle.is_finished());
    resolved.begin_teardown();
    assert!(
        join_service(&mut resolved, handle)
            .await?
            .unwrap_err()
            .is_cancelled()
    );
    assert!(resolved.released());
    assert_eq!(runs.lock().unwrap().len(), 1);
    Ok(())
}

#[tokio::test]
async fn absent_and_p2_services_refuse_dispatch_claims() -> anyhow::Result<()> {
    let mut absent = resolve(None, []).await?;
    assert!(absent.execute_service().await?.is_none());
    assert!(
        absent
            .claim_service_dispatch()
            .err()
            .unwrap()
            .to_string()
            .contains("no service")
    );
    let p2 = service("(component)");
    let id = p2.id().to_string();
    let resolved = resolve(Some(p2), []).await?;
    assert!(resolved.is_service_item(&id));
    let error = resolved
        .dispatch_target(&id, "p2-plugin")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("p2 service"));
    Ok(())
}

#[cfg(feature = "host-component-plugins")]
fn actual_function(component: &Component) -> wasmtime::component::types::ComponentFunc {
    let (ComponentItem::ComponentInstance(instance), _) = component.get_export(None, API).unwrap()
    else {
        panic!("interface missing")
    };
    instance
        .exports(engine())
        .find_map(|(name, item)| match item.ty {
            ComponentItem::ComponentFunc(function) if name == "f" => Some(function),
            _ => None,
        })
        .unwrap()
}

#[cfg(feature = "host-component-plugins")]
#[tokio::test]
async fn component_export_routes_execute_the_function_the_callee_declares() -> anyhow::Result<()> {
    use wasmtime::component::Val;
    let callee = component("callee", EXPORTER);
    let id = callee.id().to_string();
    let function = actual_function(callee.component());
    let params = function.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let results = function.results().collect::<Vec<_>>();
    let resolved = resolve(None, [callee, component("caller", "(component)")]).await?;
    let caller = resolved
        .components
        .read()
        .await
        .values()
        .find(|c| c.name() == "caller")
        .unwrap()
        .id()
        .to_string();
    let funcs = [ExternalCallFunc {
        name: "f",
        param_tys: &params,
        result_tys: &results,
    }];
    let routes = resolved
        .external_export_invocations(&id, API, &funcs)
        .await?;
    let route = &routes["f"];
    let mut store = resolved.new_store(&caller).await?;
    let values = tokio::time::timeout(
        Duration::from_secs(5),
        store.run_concurrent(async |accessor| {
            let mut values = [Val::U32(0)];
            invoke_linked_async_export(accessor, &[], &mut values, route).await?;
            Ok::<_, wasmtime::Error>(values)
        }),
    )
    .await???;
    assert!(matches!(values, [Val::U32(42)]));
    match resolved
        .item_exporting(&id, &WitInterface::from("test:workload/api"))
        .await
    {
        ItemExport::Component { name, export } => {
            assert_eq!(name.as_ref(), "callee");
            assert_eq!(export.as_ref(), API);
        }
        _ => panic!("component export missing"),
    }
    Ok(())
}

#[cfg(feature = "host-component-plugins")]
#[tokio::test]
async fn service_export_routes_share_the_running_instance() -> anyhow::Result<()> {
    use wasmtime::component::Val;
    let (service, runs, entered) = p3_service(Outcome::Success, 0);
    let id = service.id().to_string();
    let function = actual_function(service.component());
    let results = function.results().collect::<Vec<_>>();
    let caller = component("caller", "(component)");
    let caller_id = caller.id().to_string();
    let mut resolved = resolve(Some(service), [caller]).await?;
    assert!(
        matches!(resolved.item_exporting(&id, &WitInterface::from("test:workload/api")).await,
        ItemExport::Service { export } if export.as_ref() == API)
    );
    let claim = resolved.claim_service_dispatch()?;
    let funcs = [ExternalCallFunc {
        name: "f",
        param_tys: &[],
        result_tys: &results,
    }];
    let routes = resolved.service_export_invocations(&claim, API, &funcs)?;
    let route = &routes["f"];
    let handle = resolved
        .execute_service()
        .await?
        .context("service task missing")?;
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await?;
    let mut store = resolved.new_store(&caller_id).await?;
    for expected in [2, 3] {
        let values = tokio::time::timeout(
            Duration::from_secs(5),
            store.run_concurrent(async |accessor| {
                let mut values = [Val::U32(0)];
                invoke_linked_async_export(accessor, &[], &mut values, route).await?;
                Ok::<_, wasmtime::Error>(values)
            }),
        )
        .await???;
        assert!(matches!(values, [Val::U32(value)] if value == expected));
    }
    assert_eq!(runs.lock().unwrap().len(), 1);
    resolved.begin_teardown();
    assert!(
        join_service(&mut resolved, handle)
            .await?
            .unwrap_err()
            .is_cancelled()
    );
    Ok(())
}

#[tokio::test]
async fn plugin_selection_is_stable_across_registration_orders() -> anyhow::Result<()> {
    for reverse in [false, true] {
        let events = Arc::default();
        let first = Arc::new(RecordingPlugin::new("a-provider", API, &events));
        let last = Arc::new(RecordingPlugin::new("z-provider", API, &events));
        let mut registered = HashMap::new();
        let order = if reverse {
            [last, first]
        } else {
            [first, last]
        };
        for plugin in order {
            registered.insert(plugin.id, plugin as Arc<dyn HostPlugin>);
        }
        requesting(&[API], false)
            .resolve(
                Some(&registered),
                &Default::default(),
                &crate::host::HostRef::detached(),
                &Default::default(),
            )
            .await?;
        assert_eq!(
            *events.lock().unwrap(),
            ["a-provider:Bind", "a-provider:Item", "a-provider:Resolved"]
        );
    }
    Ok(())
}

#[tokio::test]
async fn wasi_overrides_bind_last_and_restore_shadowing_after_an_error() -> anyhow::Result<()> {
    let events = Arc::default();
    let native = Arc::new(RecordingPlugin::new("z-native", API, &events));
    let mut overriding = RecordingPlugin::new("a-override", MIDDLE, &events);
    overriding
        .overrides
        .imports
        .insert(WitInterface::from("wasi:clocks/monotonic-clock@0.2.0"));
    overriding.fail = Some(Callback::Item);
    let registered = HashMap::from([
        ("a-override", Arc::new(overriding) as Arc<dyn HostPlugin>),
        ("z-native", native as Arc<dyn HostPlugin>),
    ]);
    let mut unresolved = requesting(&[API, MIDDLE], false);
    assert!(
        unresolved
            .bind_plugins(&registered, &Default::default())
            .await
            .is_err()
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "z-native:Bind",
            "z-native:Item",
            "a-override:Bind",
            "a-override:Item",
            "a-override:Unbind",
            "z-native:Unbind"
        ]
    );
    let metadata = &mut unresolved.components.values_mut().next().unwrap().metadata;
    let duplicate = metadata
        .linker()?
        .instance(API)?
        .func_wrap("f", |_, (): ()| Ok(()));
    assert!(
        duplicate.is_err(),
        "override shadowing leaked into the next bind"
    );
    Ok(())
}

#[test]
fn wasi_override_requests_validate_names_and_versions_before_opening_the_linker() {
    for interface in [
        "test:workload/api@0.2.0",
        "wasi:clocks/monotonic-clock,wall-clock@0.2.0",
        "wasi:clocks/monotonic-clock",
        "wasi:clocks/monotonic-clock@9.0.0",
    ] {
        let mut component = component("override", "(component)");
        assert!(
            component.wasi_override_instance(interface).is_err(),
            "{interface}"
        );
        assert!(matches!(
            component.link,
            LinkState::Open { written: false, .. }
        ));
    }
}

#[test]
fn compatible_wasi_overrides_reopen_the_host_instance_and_preserve_other_definitions()
-> anyhow::Result<()> {
    let mut component = component("override", "(component)");
    let version = super::super::WASI_LINKER_VERSIONS
        .into_iter()
        .find(|v| v.major == 0 && v.minor == 2)
        .unwrap();
    let exact = format!("wasi:clocks/monotonic-clock@{version}");
    component
        .linker()?
        .instance(&exact)?
        .func_wrap("preserved", |_, (): ()| Ok(()))?;
    component
        .wasi_override_instance("wasi:clocks/monotonic-clock@0.2.0")?
        .func_wrap("replacement", |_, (): ()| Ok(()))?;
    assert!(
        component
            .linker()?
            .instance(&exact)?
            .func_wrap("preserved", |_, (): ()| Ok(()))
            .is_err()
    );
    assert!(
        component
            .linker()?
            .instance(&exact)?
            .func_wrap("replacement", |_, (): ()| Ok(()))
            .is_err()
    );
    component.seal_linker();
    assert!(
        component
            .wasi_override_instance("wasi:clocks/monotonic-clock@0.2.0")
            .is_err()
    );
    Ok(())
}

#[derive(Default)]
struct RecordingHttp {
    resolved: Mutex<Vec<String>>,
    unbinds: AtomicUsize,
    fail_resolved: bool,
}

#[async_trait::async_trait]
impl crate::host::http::HostHandler for RecordingHttp {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn port(&self) -> u16 {
        0
    }
    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        id: &str,
    ) -> anyhow::Result<()> {
        let components = workload.components.read().await;
        let component = &components[id];
        assert!(component.is_sealed());
        self.resolved
            .lock()
            .unwrap()
            .push(component.name().to_string());
        ensure!(!self.fail_resolved, "HTTP registration refused");
        Ok(())
    }
    async fn on_workload_unbind(&self, _: &str) -> anyhow::Result<()> {
        self.unbinds.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn outgoing_request(
        &self,
        workload: &str,
        request: hyper::Request<wasmtime_wasi_http::WasiBody>,
        options: Option<wasmtime_wasi_http::RequestOptions>,
        future: crate::host::http::RequestIoFuture,
        allowed_hosts: &[crate::host::allowed_hosts::AllowedHost],
    ) -> crate::host::http::SendFuture {
        crate::host::http::HostHandler::outgoing_request(
            &crate::host::http::NullServer::default(),
            workload,
            request,
            options,
            future,
            allowed_hosts,
        )
    }
}

#[tokio::test]
async fn http_registration_uses_manifest_order_and_failed_registration_rolls_back_plugins()
-> anyhow::Result<()> {
    for fail in [false, true] {
        let events = Arc::default();
        let plugin = Arc::new(RecordingPlugin::new("recording", API, &events));
        let plugins = HashMap::from([("recording", plugin as Arc<dyn HostPlugin>)]);
        let wat = r#"(component
            (import "test:workload/api@0.1.0" (instance (export "f" (func))))
            (instance $http)
            (export "wasi:http/incoming-handler@0.2.0" (instance $http)))"#;
        let mut first = component("manifest-first", wat);
        first.metadata.id = Arc::from("z-first");
        let mut second = component("map-first", wat);
        second.metadata.id = Arc::from("a-second");
        let recording = Arc::new(RecordingHttp {
            fail_resolved: fail,
            ..Default::default()
        });
        let handler: Arc<dyn crate::host::http::HostHandler> = recording.clone();
        let resources = WorkloadResources::default();
        let result = UnresolvedWorkload::new(
            "contracts",
            "contracts",
            "test",
            None,
            [first, second],
            vec![
                WitInterface::from(API),
                WitInterface::from("wasi:http/incoming-handler@0.2.0"),
            ],
        )
        .resolve_for_start(
            Some(&plugins),
            &Default::default(),
            &crate::host::HostRef::from_handler(&handler),
            &Default::default(),
            Some(&resources),
        )
        .await;
        assert_eq!(*recording.resolved.lock().unwrap(), ["manifest-first"]);
        if fail {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("HTTP registration refused")
            );
        } else {
            assert_eq!(result?.component_count(), 2);
        }
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.ends_with(":Unbind"))
        );
        resources.release("contracts").await?;
        resources.release("contracts").await?;
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.ends_with(":Unbind"))
                .count(),
            1
        );
        assert_eq!(recording.unbinds.load(Ordering::SeqCst), 1);
    }
    Ok(())
}

#[test]
fn dependency_order_is_stable_and_ignores_dependencies_outside_the_workload() -> anyhow::Result<()>
{
    let entries = [("a", vec!["outside"]), ("b", vec![]), ("c", vec!["a"])];
    for reverse in [false, true] {
        let mut graph = HashMap::new();
        let mut entries = entries.clone();
        if reverse {
            entries.reverse();
        }
        for (id, dependencies) in entries {
            graph.insert(
                Arc::from(id),
                dependencies.into_iter().map(Arc::from).collect(),
            );
        }
        assert_eq!(
            topological_sort_components(&graph)?,
            [Arc::<str>::from("b"), Arc::from("a"), Arc::from("c")]
        );
    }
    assert!(topological_sort_components(&HashMap::new())?.is_empty());
    let graph = HashMap::from([(Arc::from("self"), HashSet::from([Arc::from("self")]))]);
    assert!(topological_sort_components(&graph).is_err());
    Ok(())
}

#[tokio::test]
async fn resource_links_install_guest_types_and_preserve_reserved_host_types() -> anyhow::Result<()>
{
    for name in [
        "value",
        "output-stream",
        "input-stream",
        "pollable",
        "tcp-socket",
        "incoming-value-async-body",
    ] {
        let exporter = component(
            "resource",
            &format!(
                r#"(component
            (type $resource (resource (rep i32)))
            (instance $api (export "{name}" (type $resource)))
            (export "test:workload/api@0.1.0" (instance $api)))"#
            ),
        );
        let exports = build_export_map(&[(
            ComponentExporter {
                name: Arc::from(exporter.name()),
                id: Arc::from(exporter.id()),
            },
            vec![API.to_string()],
        )]);
        let resolved = resolve(None, [exporter]).await?;
        let mut importer = component(
            "importer",
            &format!(
                r#"(component
            (import "test:workload/api@0.1.0" (instance $api (export "{name}" (type (sub resource)))))
            (alias export $api "{name}" (type $resource))
            (export "linked-resource" (type $resource)))"#
            ),
        );
        let reserved = name != "value";
        if reserved {
            importer.linker()?.instance(API)?.resource(
                name,
                ResourceType::host::<u32>(),
                |_, _| Ok(()),
            )?;
        }
        let compiled = importer.component().clone();
        let id = importer.id().to_string();
        resolved
            .resolve_component_imports(
                &compiled,
                &mut importer.metadata,
                &exports,
                &HashSet::new(),
                Some(&id),
            )
            .await?;
        let pre = importer.pre_instantiate_ref()?;
        let ctx = crate::engine::ctx::Ctx::builder("contracts", importer.id()).build();
        let mut store = wasmtime::Store::new(engine(), SharedCtx::new(ctx));
        let instance = pre.instantiate_async(&mut store).await?;
        let expected = if reserved {
            ResourceType::host::<u32>()
        } else {
            ResourceType::host::<ResourceAny>()
        };
        assert_eq!(
            instance.get_resource(&mut store, "linked-resource"),
            Some(expected)
        );
        assert!(
            importer
                .linker()?
                .instance(API)?
                .resource(name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                .is_err(),
            "resource {name} was not retained"
        );
    }
    Ok(())
}
