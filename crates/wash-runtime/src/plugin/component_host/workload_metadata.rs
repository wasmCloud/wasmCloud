//! Logical owner access scoped to this plugin's bound workloads.

use std::sync::Arc;

use wasmtime::component::{Linker, Val};

use crate::engine::ctx::SharedCtx;

use super::ComponentHostPluginState;

const HOST_WORKLOAD_METADATA_INTERFACE: &str = "wasmcloud:host/workload-metadata@0.1.7";

pub(super) fn install_host_workload_metadata(
    linker: &mut Linker<SharedCtx>,
    state: &Arc<ComponentHostPluginState>,
) -> anyhow::Result<()> {
    let state = Arc::clone(state);
    linker
        .instance(HOST_WORKLOAD_METADATA_INTERFACE)?
        .func_new("owner", move |_store, _ty, params, results| {
            let [Val::String(id)] = params else {
                wasmtime::bail!("owner requires a workload id");
            };
            let owner = state
                .bound
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id.as_str())
                .filter(|info| info.generation.is_active())
                .map(|workload| Arc::clone(&workload.owner));
            let owner = owner.map(|owner| {
                use crate::engine::workload::OwnerKind;
                let kind = match owner.kind {
                    OwnerKind::Deployment => "deployment",
                    OwnerKind::DaemonSet => "daemon-set",
                    OwnerKind::ReplicaSet => "replica-set",
                    OwnerKind::Workload => "workload",
                };
                Box::new(Val::Record(vec![
                    ("namespace".into(), Val::String(owner.namespace.to_string())),
                    ("kind".into(), Val::Enum(kind.into())),
                    ("name".into(), Val::String(owner.name.to_string())),
                ]))
            });
            let [result] = results else {
                wasmtime::bail!("owner requires one result");
            };
            *result = Val::Option(owner);
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use wasmtime::Store;
    use wasmtime::component::{Func, Instance};

    use super::*;
    use crate::engine::Engine;
    use crate::engine::workload::{
        OwnerKind, UnresolvedWorkload, WorkloadComponent, WorkloadOwner,
    };
    use crate::host::trigger_service::CapabilityJob;
    use crate::plugin::{HostPlugin, WitInterfaces};

    use super::super::tests::one_func_interface;
    use super::super::{
        ComponentHostPlugin, LifecycleFuncs, Relocated, evict_workload, replay_snapshot,
    };

    mod contract {
        wasmtime::component::bindgen!({
            path: ["../../wit/secrets/wit", "../../wit/host/wit"],
            inline: "package test:metadata; world metadata { import wasmcloud:host/workload-metadata@0.1.7; }",
        });
    }

    const PROBE: &str = r#"
        (component
            (import "wasmcloud:host/workload-metadata@0.1.7" (instance $metadata
                (type $k (enum "deployment" "daemon-set" "replica-set" "workload"))
                (export "owner-kind" (type $kind (eq $k)))
                (type $o (record
                    (field "namespace" string) (field "kind" $kind) (field "name" string)))
                (export "workload-owner" (type $identity (eq $o)))
                (export "owner" (func (param "id" string) (result (option $identity))))
            ))
            (alias export $metadata "owner-kind" (type $kind))
            (alias export $metadata "workload-owner" (type $identity))
            (type $owner (func (param "id" string) (result (option $identity))))
            (alias export $metadata "owner" (func $owner))
            (core module $memory
                (memory (export "memory") 1)
                (global $heap (mut i32) (i32.const 4096))
                (func (export "realloc") (param i32 i32 i32 i32) (result i32)
                    (local $ptr i32)
                    global.get $heap
                    local.tee $ptr
                    local.get 3
                    i32.add
                    i32.const 15
                    i32.add
                    i32.const -16
                    i32.and
                    global.set $heap
                    local.get $ptr)
            )
            (core instance $memory (instantiate $memory))
            (alias core export $memory "memory" (core memory $memory))
            (alias core export $memory "realloc" (core func $realloc))
            (core func $owner (canon lower (func $owner)
                (memory $memory) (realloc $realloc)))
            (core module $probe
                (import "host" "owner" (func $owner (param i32 i32 i32)))
                (func (export "owner") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.const 1024
                    call $owner
                    i32.const 1024)
            )
            (core instance $host (export "owner" (func $owner)))
            (core instance $probe (instantiate $probe (with "host" (instance $host))))
            (func $probe (type $owner)
                (canon lift (core func $probe "owner")
                    (memory $memory) (realloc $realloc)))
            (instance $probe
                (export "owner-kind" (type $kind))
                (export "workload-owner" (type $identity))
                (export "owner" (func $probe)))
            (export "test:metadata/probe@0.1.0" (instance $probe))
        )
    "#;

    async fn new_plugin() -> ComponentHostPlugin {
        let engine = Engine::builder().build().unwrap();
        let wasm = wat::parse_str(PROBE).unwrap();
        ComponentHostPlugin::builder()
            .id("metadata-test")
            .wasm(&wasm)
            .engine(engine)
            .build()
            .await
            .unwrap()
    }

    async fn probe(plugin: &ComponentHostPlugin) -> (Store<SharedCtx>, Func) {
        let mut store = super::super::build_plugin_store(
            &plugin.runtime.engine,
            plugin.id,
            &HashMap::new(),
            &plugin.runtime.store,
        );
        let instance: Instance = plugin
            .runtime
            .pre
            .instantiate_async(&mut store)
            .await
            .unwrap();
        let (_, interface) = instance
            .get_export(&mut store, None, "test:metadata/probe@0.1.0")
            .unwrap();
        let (_, function) = instance
            .get_export(&mut store, Some(&interface), "owner")
            .unwrap();
        let function = instance.get_func(&mut store, function).unwrap();
        (store, function)
    }

    async fn owner(
        store: &mut Store<SharedCtx>,
        function: &Func,
        id: &str,
    ) -> Option<WorkloadOwner> {
        let mut results = [Val::Option(None)];
        function
            .call_async(store, &[Val::String(id.into())], &mut results)
            .await
            .unwrap();
        let [Val::Option(result)] = results else {
            panic!("expected optional owner");
        };
        result.map(|result| {
            let Val::Record(fields) = *result else {
                panic!("expected owner record");
            };
            let [
                (_, Val::String(namespace)),
                (_, Val::Enum(kind)),
                (_, Val::String(name)),
            ]: [_; 3] = fields.try_into().unwrap()
            else {
                panic!("expected owner fields");
            };
            let kind = match kind.as_str() {
                "deployment" => OwnerKind::Deployment,
                "daemon-set" => OwnerKind::DaemonSet,
                "replica-set" => OwnerKind::ReplicaSet,
                "workload" => OwnerKind::Workload,
                _ => panic!("unknown owner kind"),
            };
            WorkloadOwner::new(namespace, kind, name)
        })
    }

    fn workload(id: &str, owner: Option<(OwnerKind, &str)>) -> UnresolvedWorkload {
        let workload = UnresolvedWorkload::new(
            id,
            "replica",
            "test",
            None,
            std::iter::empty::<WorkloadComponent>(),
            Vec::new(),
        );
        match owner {
            Some((kind, name)) => workload.with_owner(kind, name),
            None => workload,
        }
    }

    fn with_lifecycle(plugin: &mut ComponentHostPlugin) {
        plugin.lifecycle = Some(Arc::new(LifecycleFuncs {
            interface: Arc::from("wasmcloud:host/workload-lifecycle@0.1.6"),
            takes_secrets: false,
            bind: one_func_interface("lifecycle", "on-workload-bind", true)
                .funcs
                .remove(0),
            unbind: one_func_interface("lifecycle", "on-workload-unbind", true)
                .funcs
                .remove(0),
        }));
    }

    #[test]
    fn initialized_workload_resolves_the_owner_once() {
        let engine = Engine::builder().build().unwrap();
        let annotations = HashMap::from([(
            "runtime.wasmcloud.dev/workload-deployment".to_string(),
            "deployment".to_string(),
        )]);
        let workload = engine
            .initialize_workload(
                "a",
                crate::types::Workload {
                    name: "replica".into(),
                    namespace: "test".into(),
                    annotations: annotations.clone(),
                    service: None,
                    components: vec![],
                    host_interfaces: vec![],
                    volumes: vec![],
                },
            )
            .unwrap();
        assert_eq!(
            workload.owner(),
            &WorkloadOwner::new("test", OwnerKind::Deployment, "deployment")
        );
    }

    #[tokio::test]
    async fn metadata_is_scoped_to_bound_workloads_without_lifecycle_hooks() {
        let plugin = new_plugin().await;
        let other = new_plugin().await;
        let (mut store, function) = probe(&plugin).await;
        let (mut other_store, other_function) = probe(&other).await;
        let interfaces = HashSet::new();
        assert!(owner(&mut store, &function, "a").await.is_none());
        plugin
            .on_workload_bind(
                &workload("a", Some((OwnerKind::Deployment, "deployment-1"))),
                WitInterfaces::new(&interfaces),
            )
            .await
            .unwrap();
        plugin
            .on_workload_bind(
                &workload("rs", Some((OwnerKind::ReplicaSet, "deployment-1"))),
                WitInterfaces::new(&interfaces),
            )
            .await
            .unwrap();
        assert_eq!(
            owner(&mut store, &function, "rs").await,
            Some(WorkloadOwner::new(
                "test",
                OwnerKind::ReplicaSet,
                "deployment-1"
            ))
        );
        plugin
            .on_workload_bind(
                &workload("ds", Some((OwnerKind::DaemonSet, "deployment-1"))),
                WitInterfaces::new(&interfaces),
            )
            .await
            .unwrap();
        assert_eq!(
            owner(&mut store, &function, "ds").await,
            Some(WorkloadOwner::new(
                "test",
                OwnerKind::DaemonSet,
                "deployment-1"
            ))
        );
        plugin
            .on_workload_bind(&workload("empty", None), WitInterfaces::new(&interfaces))
            .await
            .unwrap();
        assert_eq!(
            owner(&mut store, &function, "a").await,
            Some(WorkloadOwner::new(
                "test",
                OwnerKind::Deployment,
                "deployment-1"
            ))
        );
        assert_eq!(
            owner(&mut store, &function, "empty").await,
            Some(WorkloadOwner::new("test", OwnerKind::Workload, "replica"))
        );
        plugin
            .state
            .bound
            .lock()
            .unwrap()
            .get("rs")
            .unwrap()
            .generation
            .revoke();
        assert!(owner(&mut store, &function, "rs").await.is_none());
        assert!(owner(&mut store, &function, "unknown").await.is_none());
        assert!(
            owner(&mut other_store, &other_function, "a")
                .await
                .is_none()
        );
        plugin
            .on_workload_unbind("a", WitInterfaces::new(&interfaces))
            .await
            .unwrap();
        assert!(owner(&mut store, &function, "a").await.is_none());
        assert_eq!(
            owner(&mut store, &function, "empty").await,
            Some(WorkloadOwner::new("test", OwnerKind::Workload, "replica"))
        );
    }

    #[tokio::test]
    async fn metadata_is_available_during_bind_and_survives_a_fresh_store() {
        let mut plugin = new_plugin().await;
        with_lifecycle(&mut plugin);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        plugin.state.tx.store(Some(Arc::new(tx)));
        let interfaces = HashSet::new();
        let (mut store, function) = probe(&plugin).await;
        let workload = workload("a", Some((OwnerKind::Deployment, "deployment")));
        let bind = plugin.on_workload_bind(&workload, WitInterfaces::new(&interfaces));
        let serve = async {
            let Some(CapabilityJob::Call(call)) = rx.recv().await else {
                panic!("expected bind");
            };
            assert_eq!(
                owner(&mut store, &function, "a").await,
                Some(WorkloadOwner::new(
                    "test",
                    OwnerKind::Deployment,
                    "deployment"
                )),
            );
            call.reply
                .send(Ok(vec![Relocated::Val(Val::Result(Ok(None)))]))
                .ok()
                .expect("bind receiver must be live");
        };
        let (bound, ()) = tokio::join!(bind, serve);
        bound.unwrap();
        let replay = replay_snapshot(
            &plugin.state.bound.lock().unwrap(),
            plugin.lifecycle.as_deref(),
        );
        assert_eq!(replay.len(), 1);
        assert_eq!(&*replay[0].caller.workload_id, "a");
        let (mut fresh_store, fresh_function) = probe(&plugin).await;
        assert_eq!(
            owner(&mut fresh_store, &fresh_function, "a").await,
            Some(WorkloadOwner::new(
                "test",
                OwnerKind::Deployment,
                "deployment"
            )),
        );
        evict_workload(&plugin.state, &Arc::from("a"));
        assert!(
            owner(&mut fresh_store, &fresh_function, "a")
                .await
                .is_none()
        );
    }
    #[tokio::test]
    async fn failed_bind_delivery_revokes_metadata() {
        let mut plugin = new_plugin().await;
        with_lifecycle(&mut plugin);
        let (mut store, function) = probe(&plugin).await;
        let interfaces = HashSet::new();
        let workload = workload("a", Some((OwnerKind::Deployment, "deployment")));
        assert!(
            plugin
                .on_workload_bind(&workload, WitInterfaces::new(&interfaces))
                .await
                .is_err()
        );
        assert!(owner(&mut store, &function, "a").await.is_none());

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        plugin.state.tx.store(Some(Arc::new(tx)));
        assert!(
            plugin
                .on_workload_bind(&workload, WitInterfaces::new(&interfaces))
                .await
                .is_err()
        );
        assert!(owner(&mut store, &function, "a").await.is_none());
    }

    #[tokio::test]
    async fn rejected_bind_revokes_metadata_before_rollback_unbind() {
        let mut plugin = new_plugin().await;
        with_lifecycle(&mut plugin);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        plugin.state.tx.store(Some(Arc::new(tx)));
        let (mut store, function) = probe(&plugin).await;
        let interfaces = HashSet::new();
        let workload = workload("a", Some((OwnerKind::Deployment, "deployment")));
        let bind = plugin.on_workload_bind(&workload, WitInterfaces::new(&interfaces));
        let serve = async {
            let Some(CapabilityJob::Call(call)) = rx.recv().await else {
                panic!("expected bind");
            };
            call.reply
                .send(Ok(vec![Relocated::Val(Val::Result(Err(Some(Box::new(
                    Val::String("rejected".into()),
                )))))]))
                .ok()
                .expect("bind receiver must be live");
            let Some(CapabilityJob::Call(call)) = rx.recv().await else {
                panic!("expected rollback unbind");
            };
            assert_eq!(&*call.func, "on-workload-unbind");
            assert!(owner(&mut store, &function, "a").await.is_none());
            call.reply
                .send(Ok(vec![]))
                .ok()
                .expect("unbind receiver must be live");
        };
        let (bound, ()) = tokio::join!(bind, serve);
        assert!(bound.unwrap_err().to_string().contains("rejected"));
        assert!(owner(&mut store, &function, "a").await.is_none());
    }
    #[tokio::test]
    async fn failed_item_bind_revokes_metadata_without_lifecycle_hooks() {
        let plugin = new_plugin().await;
        let engine = plugin.runtime.engine.inner();
        let wasm = wat::parse_str(
            r#"(component
            (import "test:metadata/probe@0.1.0" (instance
                (type $k (enum "deployment" "daemon-set" "replica-set" "workload"))
                (export "owner-kind" (type $kind (eq $k)))
                (type $o (record (field "namespace" string)
                    (field "kind" $kind) (field "name" string)))
                (export "workload-owner" (type $identity (eq $o)))
                (export "owner" (func (param "id" string) (result (option $identity))))
            ))
        )"#,
        )
        .unwrap();
        let mut component = WorkloadComponent::new(
            "a",
            "replica",
            "test",
            "caller",
            wasmtime::component::Component::new(engine, &wasm).unwrap(),
            Linker::new(engine),
            vec![],
            crate::types::LocalResources::default(),
            Arc::default(),
            crate::engine::instance_pool::InstancePolicy::Ephemeral,
        );
        let mut interface = crate::wit::WitInterface::from("test:metadata/probe@0.1.0");
        interface.name = Some("named".into());
        let interfaces = HashSet::from([interface]);
        plugin
            .on_workload_bind(
                &workload("a", Some((OwnerKind::Deployment, "deployment"))),
                WitInterfaces::new(&interfaces),
            )
            .await
            .unwrap();
        let (mut store, function) = probe(&plugin).await;
        assert!(owner(&mut store, &function, "a").await.is_some());
        let error = plugin
            .on_workload_item_bind(
                &mut crate::engine::workload::WorkloadItem::Component(&mut component),
                WitInterfaces::new(&interfaces),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("named"));
        assert!(owner(&mut store, &function, "a").await.is_none());
    }
    #[tokio::test]
    async fn timed_out_bind_revokes_metadata_before_deferred_unbind() {
        let mut plugin = new_plugin()
            .await
            .with_lifecycle_call_timeout(std::time::Duration::from_millis(20));
        with_lifecycle(&mut plugin);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        plugin.state.tx.store(Some(Arc::new(tx)));
        let interfaces = HashSet::new();
        let workload = workload("a", Some((OwnerKind::Deployment, "deployment")));
        let (bound, pending) = tokio::join!(
            plugin.on_workload_bind(&workload, WitInterfaces::new(&interfaces)),
            rx.recv(),
        );
        assert!(bound.unwrap_err().to_string().contains("timed out"));
        let (mut store, function) = probe(&plugin).await;
        assert!(owner(&mut store, &function, "a").await.is_none());
        let Some(CapabilityJob::Call(call)) = pending else {
            panic!("expected bind");
        };
        call.reply
            .send(Ok(vec![Relocated::Val(Val::Result(Ok(None)))]))
            .ok()
            .expect("deferred cleanup must retain the bind receiver");
        let pending = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap();
        let Some(CapabilityJob::Call(call)) = pending else {
            panic!("expected deferred unbind");
        };
        assert_eq!(&*call.func, "on-workload-unbind");
        assert!(owner(&mut store, &function, "a").await.is_none());
        call.reply
            .send(Ok(vec![]))
            .ok()
            .expect("unbind receiver must be live");
    }
}
