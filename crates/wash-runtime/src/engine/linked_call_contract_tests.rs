use super::*;
use crate::engine::stream_payload::StreamPayloads;
use wasmtime::component::{Component, types::ComponentItem};

#[tokio::test]
async fn detached_stream_tasks_finish_and_release_their_output() -> anyhow::Result<()> {
    let (finish, wait) = tokio::sync::oneshot::channel();
    let output = Arc::new(());
    let released = Arc::downgrade(&output);
    let task = AbortOnDropHandle::new(tokio::spawn(async move {
        wait.await.expect("stream drain cancelled");
        output
    }));
    detach_after_reply(task);
    finish
        .send(())
        .map_err(|_| anyhow::anyhow!("stream task aborted before draining"))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while released.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Position {
    Param,
    SecondParam,
    Result,
}

const POSITIONS: [Position; 3] = [Position::Param, Position::SecondParam, Position::Result];

impl Position {
    fn func_type(self) -> &'static str {
        match self {
            Position::Param => r#"(param "value" $value) (result u32)"#,
            Position::SecondParam => r#"(param "a" u32) (param "value" $value)"#,
            Position::Result => r#"(param "a" u32) (result $value)"#,
        }
    }
}

fn type_engine() -> &'static wasmtime::Engine {
    static ENGINE: std::sync::OnceLock<wasmtime::Engine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_error_context(true)
            .wasm_component_model_map(true)
            .wasm_component_model_fixed_length_lists(true);
        wasmtime::Engine::new(&config).unwrap()
    })
}

fn signature(payload: &str, aggregate: &str, position: Position) -> ComponentFunc {
    let engine = type_engine();
    let bytes = wat::parse_str(format!(
        r#"(component
        (import "resource" (type $resource (sub resource)))
        (type $bytes-type (stream u8))
        (import "bytes" (type $bytes (eq $bytes-type)))
        (type $payload-type {payload})
        (import "payload" (type $payload (eq $payload-type)))
        (type $value-type {aggregate})
        (import "value" (type $value (eq $value-type)))
        (type $call (func {}))
        (import "call" (func (type $call))))"#,
        position.func_type()
    ))
    .unwrap();
    let component = Component::new(engine, bytes)
        .unwrap_or_else(|error| panic!("{payload} in {aggregate} as {position:?}: {error:#}"));
    component
        .component_type()
        .imports(engine)
        .find_map(|(_, item)| match item.ty {
            ComponentItem::ComponentFunc(function) => Some(function),
            _ => None,
        })
        .unwrap()
}

fn optional_stream_element(function: &ComponentFunc) -> Type {
    let Type::Option(option) = function.params().next().unwrap().1 else {
        unreachable!()
    };
    let Type::Stream(stream) = option.ty() else {
        unreachable!()
    };
    stream.ty().unwrap()
}

#[test]
fn linked_signature_routing_checks_nested_streams_and_store_bound_handles() {
    let payloads = StreamPayloads::default();
    for aggregate in [
        "(list $payload)",
        "(list $payload 2)",
        "(option $payload)",
        "(tuple u32 $payload)",
        "(record (field \"data\" $payload))",
        "(variant (case \"empty\") (case \"value\" $payload))",
        "(result $payload (error string))",
        "(result string (error $payload))",
        "(map string $payload)",
    ] {
        for (payload, ephemeral, bridge) in [
            ("u32", true, true),
            ("(stream u8)", false, true),
            ("(future)", false, true),
            ("(future string)", false, true),
            ("error-context", false, false),
            ("(own $resource)", false, false),
        ] {
            for position in POSITIONS {
                let function = signature(payload, aggregate, position);
                assert_eq!(
                    func_is_ephemeral_safe(&function),
                    ephemeral,
                    "{payload} in {aggregate} as {position:?}"
                );
                assert_eq!(
                    func_is_bridge_safe(&function, &payloads),
                    bridge,
                    "{payload} in {aggregate} as {position:?}"
                );
                #[cfg(feature = "host-component-plugins")]
                {
                    let params = function.params().map(|(_, ty)| ty).collect::<Vec<_>>();
                    let results = function.results().collect::<Vec<_>>();
                    assert_eq!(
                        types_are_ephemeral_safe(&params) && types_are_ephemeral_safe(&results),
                        ephemeral,
                        "{payload} in {aggregate} as {position:?}"
                    );
                    assert_eq!(
                        types_are_bridge_safe(&params, &payloads)
                            && types_are_bridge_safe(&results, &payloads),
                        bridge,
                        "{payload} in {aggregate} as {position:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn bare_streams_pin_the_store_but_bare_futures_bridge() {
    let payloads = StreamPayloads::default();
    for (payload, bridge) in [
        ("(stream)", false),
        ("(future)", true),
        ("(stream $bytes)", false),
        ("(future (own $resource))", false),
        ("(borrow $resource)", false),
    ] {
        for aggregate in ["(option $payload)", "(map $payload string)"] {
            let function = signature(payload, aggregate, Position::Param);
            assert!(
                !func_is_ephemeral_safe(&function),
                "{payload} in {aggregate}"
            );
            assert_eq!(
                func_is_bridge_safe(&function, &payloads),
                bridge,
                "{payload} in {aggregate}"
            );
        }
    }
}

#[test]
fn typed_payload_registration_enables_only_the_registered_stream_shape() -> anyhow::Result<()> {
    let function = signature(
        "(stream (tuple u32 string))",
        "(option $payload)",
        Position::Param,
    );
    let element = optional_stream_element(&function);
    let payloads = StreamPayloads::default();
    assert!(!func_is_bridge_safe(&function, &payloads));
    payloads.register::<(u32, String)>(element.clone())?;
    assert!(func_is_bridge_safe(&function, &payloads));
    assert!(payloads.register::<(u32, String)>(element).is_err());
    assert!(!func_is_ephemeral_safe(&function));
    Ok(())
}

#[test]
fn payload_registration_refuses_store_bound_elements() {
    let function = signature(
        "(stream (own $resource))",
        "(option $payload)",
        Position::Param,
    );
    let element = optional_stream_element(&function);
    let payloads = StreamPayloads::default();
    let error = payloads
        .register::<u32>(element)
        .expect_err("a store-bound element must be refused");
    assert!(
        format!("{error:#}")
            .contains("stream/future payload adapters cannot carry store-bound handles"),
        "{error:#}"
    );
    assert!(!func_is_bridge_safe(&function, &payloads));
}

fn plain_store() -> wasmtime::Store<SharedCtx> {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let ctx = Ctx::builder("contract-workload", "caller").build();
    wasmtime::Store::new(type_engine(), SharedCtx::new(ctx))
}

#[test]
fn extract_all_relocates_plain_values_without_pumps() -> anyhow::Result<()> {
    let mut store = plain_store();
    let mut dones = Vec::new();
    let relocated = extract_all(
        store.as_context_mut(),
        &[Val::U32(1), Val::String("s".into())],
        &[Type::U32, Type::String],
        &mut dones,
    )?;
    assert!(matches!(
        relocated.as_slice(),
        [Relocated::Val(Val::U32(1)), Relocated::Val(Val::String(s))] if s == "s"
    ));
    assert!(dones.is_empty());
    Ok(())
}

#[test]
fn inject_results_rebuilds_values_in_place_and_checks_capacity() -> anyhow::Result<()> {
    let mut store = plain_store();
    let mut results = [Val::Bool(false), Val::Bool(false)];
    inject_results(
        store.as_context_mut(),
        vec![
            Relocated::Val(Val::U32(1)),
            Relocated::Record(vec![("a".into(), Relocated::Val(Val::String("x".into())))]),
        ],
        &mut results,
    )?;
    assert_eq!(
        results,
        [
            Val::U32(1),
            Val::Record(vec![("a".into(), Val::String("x".into()))]),
        ]
    );
    let error = inject_results(
        store.as_context_mut(),
        vec![Relocated::Val(Val::U32(1))],
        &mut [],
    )
    .expect_err("a result slice too short must be refused");
    assert!(
        format!("{error:#}").contains("result index out of bounds"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn write_results_copies_values_in_place_and_checks_capacity() -> anyhow::Result<()> {
    let mut results = [Val::Bool(false), Val::Bool(false)];
    write_results(
        vec![Val::U32(42), Val::String("result".into())],
        &mut results,
    )?;
    assert_eq!(results, [Val::U32(42), Val::String("result".into())]);
    let mut untouched = [Val::U32(9)];
    write_results(vec![], &mut untouched)?;
    assert_eq!(untouched, [Val::U32(9)]);
    let error = write_results(vec![Val::U32(1)], &mut [])
        .expect_err("a result slice too short must be refused");
    assert!(
        format!("{error:#}").contains("result index out of bounds"),
        "{error:#}"
    );
    Ok(())
}

fn engine_builder() -> crate::engine::EngineBuilder {
    crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .without_max_process_memory()
}

fn template(engine: &crate::engine::Engine, id: &str) -> ComponentCtxTemplate {
    ComponentCtxTemplate {
        component_id: id.into(),
        workload_id: "contract-workload".into(),
        local_resources: crate::types::LocalResources::default(),
        volume_mounts: vec![],
        plugins: Some(HashMap::new()),
        loopback: Arc::new(std::sync::Mutex::new(loopback::Network::default())),
        socket_policy: Arc::new(crate::sockets::policy::SocketPolicy::default()),
        guest_memory: Arc::clone(engine.guest_memory()),
        store_settings: engine.store_settings,
        trap_hints: Arc::default(),
        #[cfg(feature = "wasi-tls")]
        tls_provider: None,
    }
}

fn empty_pre(engine: &crate::engine::Engine) -> anyhow::Result<InstancePre<SharedCtx>> {
    let component = Component::new(engine.inner(), wat::parse_str("(component)")?)?;
    Ok(wasmtime::component::Linker::new(engine.inner()).instantiate_pre(&component)?)
}

#[tokio::test]
async fn linked_store_templates_share_identity_and_restore_the_active_component()
-> anyhow::Result<()> {
    let engine = engine_builder().with_fuel_consumption(true).build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let pre = empty_pre(&engine)?;
    let store = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &crate::host::HostRef::detached(),
        &active,
        &[linked],
        &[("callee".into(), pre)],
        false,
    )
    .await?;
    assert_eq!(store.get_fuel()?, u64::MAX);
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");
    let callee = store.data().contexts.get("callee").unwrap();
    assert_eq!(callee.workload_id.as_ref(), "contract-workload");
    assert_eq!(callee.store_id, store.data().active_ctx.store_id);
    assert!(store.data().exporter_instances.contains_key("callee"));
    Ok(())
}

#[tokio::test]
async fn linked_instantiation_runs_under_the_callees_context() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let component = Component::new(
        engine.inner(),
        wat::parse_str(
            r#"(component
            (import "probe" (func $probe))
            (core func $p (canon lower (func $probe)))
            (core module $m
                (import "" "probe" (func $q))
                (func $start (call $q))
                (start $start))
            (core instance (instantiate $m (with "" (instance (export "probe" (func $p)))))))"#,
        )?,
    )?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::<Arc<str>>::new()));
    let probed = Arc::clone(&seen);
    let mut linker = wasmtime::component::Linker::<SharedCtx>::new(engine.inner());
    linker.root().func_wrap(
        "probe",
        move |store: StoreContextMut<'_, SharedCtx>, (): ()| {
            probed
                .lock()
                .unwrap()
                .push(store.data().active_ctx.component_id.clone());
            Ok(())
        },
    )?;
    let pre = linker.instantiate_pre(&component)?;
    let store = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &crate::host::HostRef::detached(),
        &active,
        &[linked],
        &[("callee".into(), pre)],
        false,
    )
    .await?;
    let probed = seen
        .lock()
        .unwrap()
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    assert_eq!(probed, ["callee"]);
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");
    Ok(())
}

#[tokio::test]
async fn linked_instances_need_a_template() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let active = template(&engine, "caller");
    let pre = empty_pre(&engine)?;
    let error = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &crate::host::HostRef::detached(),
        &active,
        &[],
        &[("callee".into(), pre)],
        false,
    )
    .await
    .err()
    .expect("a linked instance without a template must fail");
    assert!(
        format!("{error:#}").contains("Context for component callee not found"),
        "{error:#}"
    );
    Ok(())
}

#[tokio::test]
async fn only_the_active_component_of_a_service_store_may_listen() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let address = "127.0.0.1:8080".parse::<std::net::SocketAddr>()?;
    let may_listen = |ctx: &Ctx| {
        ctx.sockets
            .socket_addr_check
            .check(address, sockets::SocketAddrUse::TcpListen)
            .is_ok()
    };
    for is_service in [false, true] {
        let store = new_store_from_templates(
            engine.inner(),
            engine.stream_payloads(),
            &crate::host::HostRef::detached(),
            &active,
            std::slice::from_ref(&linked),
            &[],
            is_service,
        )
        .await?;
        assert_eq!(may_listen(&store.data().active_ctx), is_service);
        assert!(!may_listen(store.data().contexts.get("callee").unwrap()));
    }
    Ok(())
}

#[tokio::test]
async fn linked_store_instantiation_failures_name_the_callee() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let component = Component::new(
        engine.inner(),
        wat::parse_str(
            "(component (core module (func $start unreachable) (start $start)) (core instance (instantiate 0)))",
        )?,
    )?;
    let pre = wasmtime::component::Linker::new(engine.inner()).instantiate_pre(&component)?;
    // Request and service stores report through the same path, so neither
    // may claim an ephemeral store.
    for is_service in [false, true] {
        let error = new_store_from_templates(
            engine.inner(),
            engine.stream_payloads(),
            &crate::host::HostRef::detached(),
            &active,
            std::slice::from_ref(&linked),
            &[("callee".into(), pre.clone())],
            is_service,
        )
        .await
        .err()
        .expect("a trapping start must fail instantiation");
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("failed to instantiate linked component 'callee'"),
            "{rendered}"
        );
        assert!(!rendered.contains("ephemeral"), "{rendered}");
    }
    Ok(())
}

#[tokio::test]
async fn shared_store_sync_calls_need_a_pre_instantiated_exporter() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let component = Component::new(
        engine.inner(),
        wat::parse_str(
            r#"(component
            (core module $m (func (export "f")))
            (core instance $i (instantiate $m))
            (func (export "f") (canon lift (core func $i "f"))))"#,
        )?,
    )?;
    let pre = wasmtime::component::Linker::new(engine.inner()).instantiate_pre(&component)?;
    let inv = LinkedExportInvocation {
        import_name: "callee-interface".into(),
        export_name: "f".into(),
        plugin_component_id: "callee".into(),
        func_idx: component.get_export_index(None, "f").unwrap(),
        param_tys: Arc::default(),
        target: LinkedTarget::SharedStore,
    };
    let host = crate::host::HostRef::detached();
    let mut store = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &host,
        &active,
        std::slice::from_ref(&linked),
        &[],
        false,
    )
    .await?;
    let error = invoke_linked_sync_export(store.as_context_mut(), &[], &mut [], &inv)
        .await
        .expect_err("a callee that was never instantiated must be refused");
    assert!(
        format!("{error:#}")
            .contains("linked component 'callee' was not pre-instantiated in this store"),
        "{error:#}"
    );
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");
    assert!(inv.param_tys.get().is_none());
    let mut store = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &host,
        &active,
        &[linked],
        &[("callee".into(), pre)],
        false,
    )
    .await?;
    invoke_linked_sync_export(store.as_context_mut(), &[], &mut [], &inv).await?;
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");
    assert!(inv.param_tys.get().is_some_and(|tys| tys.is_empty()));
    Ok(())
}

#[tokio::test]
async fn ephemeral_store_recipe_names_missing_components() -> anyhow::Result<()> {
    let engine = engine_builder().build()?;
    let component = Component::new(engine.inner(), wat::parse_str("(component)")?)?;
    let linker = Arc::new(wasmtime::component::Linker::<SharedCtx>::new(
        engine.inner(),
    ));
    let components = Arc::new(RwLock::new(BTreeMap::new()));
    let call = EphemeralLinkedCall {
        pre: linker.instantiate_pre(&component)?,
        engine: engine.inner().clone(),
        stream_payloads: Arc::clone(engine.stream_payloads()),
        http_handler: crate::host::HostRef::detached(),
        invocation: crate::observability::InvocationMeter::default(),
        components: Arc::downgrade(&components),
        active_component_id: "ghost".into(),
        linked_component_ids: vec!["missing".into()],
        #[cfg(feature = "wasi-tls")]
        tls_provider: None,
        mode: EphemeralCallMode::PlainValue,
    };
    let workload_component = |name: &str| {
        let mut entry = WorkloadComponent::new(
            "contract-workload",
            "contract",
            "tests",
            name,
            component.clone(),
            Arc::clone(&linker),
            vec![],
            crate::types::LocalResources::default(),
            Arc::new(std::sync::Mutex::new(loopback::Network::default())),
            instance_pool::InstancePolicy::Ephemeral,
        );
        entry.metadata.id = name.into();
        entry
    };
    let error = new_ephemeral_store(&call)
        .await
        .err()
        .expect("an absent active component must fail");
    assert!(
        format!("{error:#}").contains("component 'ghost' not found"),
        "{error:#}"
    );
    components
        .write()
        .await
        .insert("ghost".into(), workload_component("ghost"));
    let error = new_ephemeral_store(&call)
        .await
        .err()
        .expect("an absent linked component must fail");
    assert!(
        format!("{error:#}").contains("component 'missing' not found"),
        "{error:#}"
    );
    components
        .write()
        .await
        .insert("missing".into(), workload_component("missing"));
    let store = new_ephemeral_store(&call).await?;
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "ghost");
    assert!(store.data().contexts.contains_key("missing"));
    assert!(store.data().exporter_instances.contains_key("missing"));
    Ok(())
}

#[tokio::test]
async fn linked_store_applies_the_engine_hostcall_fuel() -> anyhow::Result<()> {
    let default = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
    let configured = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .with_hostcall_fuel(std::num::NonZeroUsize::new(4096).unwrap())
        .build()?;
    for (engine, expected) in [
        (
            &default,
            wasmtime::Store::new(default.inner(), ()).hostcall_fuel(),
        ),
        (&configured, 4096),
    ] {
        let store = new_store_from_templates(
            engine.inner(),
            engine.stream_payloads(),
            &crate::host::HostRef::detached(),
            &template(engine, "caller"),
            &[],
            &[],
            false,
        )
        .await?;
        assert_eq!(store.hostcall_fuel(), expected);
        // What the remediation hint names.
        assert_eq!(store.data().store_settings, engine.store_settings);
    }
    Ok(())
}

/// A callee exporting `add-one`, a `bump` that counts the calls its instance
/// has served, a trapping `trap`, and an `iface` instance that is no function.
const CALLEE: &str = r#"(component
    (core module $m
        (global $calls (mut i32) (i32.const 0))
        (func (export "add-one") (param i32) (result i32)
            local.get 0
            i32.const 1
            i32.add)
        (func (export "bump") (result i32)
            global.get $calls
            i32.const 1
            i32.add
            global.set $calls
            global.get $calls)
        (func (export "trap") unreachable))
    (core instance $i (instantiate $m))
    (func $add-one (param "x" u32) (result u32) (canon lift (core func $i "add-one")))
    (func $bump (result u32) (canon lift (core func $i "bump")))
    (func $trap (canon lift (core func $i "trap")))
    (instance $iface (export "add-one" (func $add-one)))
    (export "add-one" (func $add-one))
    (export "bump" (func $bump))
    (export "trap" (func $trap))
    (export "iface" (instance $iface)))"#;

/// A callee whose instantiation traps in its start function.
const UNSTARTABLE: &str = r#"(component
    (core module $m
        (func $start unreachable)
        (start $start)
        (func (export "add-one") (param i32) (result i32) local.get 0))
    (core instance $i (instantiate $m))
    (func (export "add-one") (param "x" u32) (result u32) (canon lift (core func $i "add-one"))))"#;

/// One warm instance, serving one call at a time.
fn warm() -> instance_pool::InstancePolicy {
    instance_pool::InstancePolicy::Warm {
        pool_size: std::num::NonZeroUsize::MIN,
        max_invocations: None,
        max_concurrency: std::num::NonZeroUsize::MIN,
        reclaim: None,
    }
}

/// The callee of a linked call, registered in a workload's component map.
struct Callee {
    engine: crate::engine::Engine,
    component: Component,
    components: Arc<ComponentMap>,
    pool: Arc<InstancePool>,
}

impl Callee {
    fn new(wat: &str, policy: instance_pool::InstancePolicy) -> anyhow::Result<Self> {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();
        let engine = engine_builder().build()?;
        let component = Component::new(engine.inner(), wat::parse_str(wat)?)?;
        let linker = Arc::new(wasmtime::component::Linker::<SharedCtx>::new(
            engine.inner(),
        ));
        let mut entry = WorkloadComponent::new(
            "contract-workload",
            "contract",
            "tests",
            "callee",
            component.clone(),
            linker,
            vec![],
            crate::types::LocalResources::default(),
            Arc::new(std::sync::Mutex::new(loopback::Network::default())),
            policy,
        );
        entry.metadata.id = "callee".into();
        let pool = Arc::clone(&entry.instances);
        let components = Arc::new(RwLock::new(BTreeMap::from([("callee".into(), entry)])));
        Ok(Self {
            engine,
            component,
            components,
            pool,
        })
    }

    async fn ephemeral(
        &self,
        mode: EphemeralCallMode,
        invocation: crate::observability::InvocationMeter,
    ) -> anyhow::Result<Arc<EphemeralLinkedCall>> {
        let pre = self.components.read().await["callee"].pre_instantiate_ref()?;
        Ok(Arc::new(EphemeralLinkedCall {
            pre,
            engine: self.engine.inner().clone(),
            stream_payloads: Arc::clone(self.engine.stream_payloads()),
            http_handler: crate::host::HostRef::detached(),
            invocation,
            components: Arc::downgrade(&self.components),
            active_component_id: "callee".into(),
            linked_component_ids: vec![],
            #[cfg(feature = "wasi-tls")]
            tls_provider: None,
            mode,
        }))
    }

    fn invocation(&self, export: &str, target: LinkedTarget) -> LinkedExportInvocation {
        LinkedExportInvocation {
            import_name: "contract:callee/api".into(),
            export_name: export.into(),
            plugin_component_id: "callee".into(),
            func_idx: self.component.get_export_index(None, export).unwrap(),
            param_tys: Arc::default(),
            target,
        }
    }

    /// `export`'s param and result types.
    fn signature(&self, export: &str) -> (Arc<[Type]>, Arc<[Type]>) {
        let Some(ComponentItem::ComponentFunc(func)) = self
            .component
            .get_export(None, export)
            .map(|(item, _)| item)
        else {
            panic!("{export} is not a function export");
        };
        (
            func.params().map(|(_, ty)| ty).collect(),
            func.results().collect(),
        )
    }

    /// Both ways an ephemeral call can carry `export`'s signature.
    fn modes(&self, export: &str) -> [EphemeralCallMode; 2] {
        let (param_tys, result_tys) = self.signature(export);
        [
            EphemeralCallMode::PlainValue,
            EphemeralCallMode::Relocated {
                param_tys,
                result_tys,
            },
        ]
    }

    /// A store for `id`, carrying as much of the callee as `linked` says.
    async fn store(&self, id: &str, linked: Linked) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
        let templates = match linked {
            Linked::No => vec![],
            Linked::ContextOnly | Linked::Instantiated => vec![template(&self.engine, "callee")],
        };
        let instances = match linked {
            Linked::No | Linked::ContextOnly => vec![],
            Linked::Instantiated => {
                let pre = self.components.read().await["callee"].pre_instantiate_ref()?;
                vec![(Arc::from("callee"), pre)]
            }
        };
        new_store_from_templates(
            self.engine.inner(),
            self.engine.stream_payloads(),
            &crate::host::HostRef::detached(),
            &template(&self.engine, id),
            &templates,
            &instances,
            false,
        )
        .await
    }
}

/// How much of the callee a caller's store carries.
#[derive(Clone, Copy)]
enum Linked {
    No,
    ContextOnly,
    Instantiated,
}

/// Run `inv` from inside `store`, as a guest import would.
async fn invoke(
    store: &mut wasmtime::Store<SharedCtx>,
    inv: &LinkedExportInvocation,
    params: &[Val],
    results_len: usize,
) -> wasmtime::Result<Vec<Val>> {
    store
        .run_concurrent(async |accessor| {
            let mut results = vec![Val::Bool(false); results_len];
            invoke_linked_async_export(accessor, params, &mut results, inv).await?;
            Ok::<_, wasmtime::Error>(results)
        })
        .await?
}

async fn ephemeral_invoke(
    callee: &Callee,
    call: &Arc<EphemeralLinkedCall>,
    export: &str,
    params: &[Val],
    results_len: usize,
) -> wasmtime::Result<Vec<Val>> {
    let inv = callee.invocation(export, LinkedTarget::Ephemeral(Arc::clone(call)));
    let mut caller = callee
        .store("caller", Linked::No)
        .await
        .map_err(|e| wasmtime::format_err!("{e:#}"))?;
    invoke(&mut caller, &inv, params, results_len).await
}

#[tokio::test]
async fn ephemeral_calls_run_each_call_on_a_fresh_instance() -> anyhow::Result<()> {
    let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
    for invocation in [
        crate::observability::InvocationMeter::default(),
        crate::observability::InvocationMeter::new(true),
    ] {
        for mode in callee.modes("add-one") {
            let call = callee.ephemeral(mode, invocation.clone()).await?;
            let sum = ephemeral_invoke(&callee, &call, "add-one", &[Val::U32(41)], 1).await?;
            assert_eq!(sum, [Val::U32(42)]);
        }
        for mode in callee.modes("bump") {
            let call = callee.ephemeral(mode, invocation.clone()).await?;
            for _ in 0..2 {
                let calls = ephemeral_invoke(&callee, &call, "bump", &[], 1).await?;
                assert_eq!(
                    calls,
                    [Val::U32(1)],
                    "an ephemeral call must not share state"
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn warm_linked_calls_reuse_an_instance_until_it_traps() -> anyhow::Result<()> {
    let callee = Callee::new(CALLEE, warm())?;
    let call = callee
        .ephemeral(EphemeralCallMode::PlainValue, Default::default())
        .await?;
    for expected in 1..=2 {
        let calls = ephemeral_invoke(&callee, &call, "bump", &[], 1).await?;
        assert_eq!(
            calls,
            [Val::U32(expected)],
            "a warm instance keeps its state"
        );
    }
    let error = ephemeral_invoke(&callee, &call, "trap", &[], 0)
        .await
        .expect_err("a trapping callee must fail the call");
    assert!(format!("{error:?}").contains("unreachable"), "{error:?}");
    let calls = ephemeral_invoke(&callee, &call, "bump", &[], 1).await?;
    assert_eq!(
        calls,
        [Val::U32(1)],
        "the trapped instance must not serve again"
    );

    let error = ephemeral_invoke(&callee, &call, "iface", &[Val::U32(1)], 1)
        .await
        .expect_err("an export that is no function must fail the call");
    assert!(
        format!("{error:#}").contains("function not found for contract:callee/api.iface"),
        "{error:#}"
    );
    let sum = ephemeral_invoke(&callee, &call, "add-one", &[Val::U32(1)], 1).await?;
    assert_eq!(sum, [Val::U32(2)]);

    // A closed pool declines every call, which then runs in a store of its own
    // on the params the pool handed back.
    callee.pool.close();
    for _ in 0..2 {
        let calls = ephemeral_invoke(&callee, &call, "bump", &[], 1).await?;
        assert_eq!(calls, [Val::U32(1)]);
    }
    let sum = ephemeral_invoke(&callee, &call, "add-one", &[Val::U32(7)], 1).await?;
    assert_eq!(sum, [Val::U32(8)]);
    Ok(())
}

#[tokio::test]
async fn ephemeral_calls_report_guest_failures_to_the_caller() -> anyhow::Result<()> {
    let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
    for mode in callee.modes("trap") {
        let call = callee.ephemeral(mode, Default::default()).await?;
        let error = ephemeral_invoke(&callee, &call, "trap", &[], 0)
            .await
            .expect_err("a trapping callee must fail the call");
        assert!(format!("{error:?}").contains("unreachable"), "{error:?}");
    }
    // Wired as the importer declared it, which the callee's export is not.
    for mode in callee.modes("add-one") {
        let call = callee.ephemeral(mode, Default::default()).await?;
        let error = ephemeral_invoke(&callee, &call, "iface", &[Val::U32(1)], 1)
            .await
            .expect_err("an export that is no function must fail the call");
        assert!(
            format!("{error:#}")
                .contains("function not found for linked import contract:callee/api.iface"),
            "{error:#}"
        );

        // The callee still serves calls after both failures.
        let sum = ephemeral_invoke(&callee, &call, "add-one", &[Val::U32(1)], 1).await?;
        assert_eq!(sum, [Val::U32(2)]);
    }
    Ok(())
}

#[tokio::test]
async fn warm_instance_traps_reach_every_pending_caller() -> anyhow::Result<()> {
    use crate::engine::instance_driver::{InstanceDriver, InstanceJob, LinkedCall, LinkedJob};
    use crate::host::trigger_service::{BrokerMessage, MessagingJob};

    for reserved in [false, true] {
        let callee = Callee::new(CALLEE, warm())?;
        let pre = callee.components.read().await["callee"].pre_instantiate_ref()?;
        let mut store = callee.store("callee", Linked::No).await?;
        let dropped = store.data().dropped.token();
        let instance = pre.instantiate_async(&mut store).await?;
        let job = || {
            let (reply, rx) = tokio::sync::oneshot::channel();
            let call = crate::engine::abandon::DispatchedCall::new(
                "linked trap regression",
                std::time::Duration::from_secs(5),
            );
            let job = InstanceJob::Linked(Box::new(LinkedJob {
                call: LinkedCall {
                    func_idx: callee.component.get_export_index(None, "trap").unwrap(),
                    params: vec![],
                    results_len: 0,
                    import_name: "contract:callee/api".into(),
                    export_name: "trap".into(),
                    abandoned: call.flag(),
                    attributes: Arc::from([]),
                },
                reply,
            }));
            (job, rx, call)
        };
        let (first, first_rx, first_call) = job();
        let (second, second_rx, second_call) = job();
        let driver = InstanceDriver::spawn(ComponentInstance { store, instance }, &first, 3, None)
            .map_err(|_| anyhow::anyhow!("linked calls must be accepted"))?;
        // A delivery of another kind, held reserved until the instance has
        // trapped. Reserving skips admission, which an instance without the
        // messaging export would decline.
        let deliver = driver.reserve_send_for_test();
        let delivery_call = crate::engine::abandon::DispatchedCall::new(
            "queued delivery",
            std::time::Duration::from_secs(5),
        );
        let (result_tx, delivered) = tokio::sync::oneshot::channel();
        let delivery = InstanceJob::Messaging(Box::new(MessagingJob {
            msg: BrokerMessage {
                subject: "queued".into(),
                body: vec![],
                reply_to: None,
            },
            result_tx,
            abandoned: delivery_call.flag(),
            attributes: Arc::from([]),
        }));
        driver
            .try_send(first)
            .map_err(|_| anyhow::anyhow!("first call must be admitted"))?;
        let publish = if reserved {
            let send = driver.reserve_send_for_test();
            Some(move || send(second))
        } else {
            driver
                .try_send(second)
                .map_err(|_| anyhow::anyhow!("second call must be admitted"))?;
            None
        };

        let error = first_call
            .await_reply(first_rx)
            .await
            .expect("a store trap must produce a reply")
            .expect("a store trap must not drop the reply")
            .expect_err("the first call must trap");
        assert!(format!("{error:#}").contains("unreachable"), "{error:#}");
        assert!(driver.is_gone());
        assert!(dropped.is_cancelled(), "the failed store must be released");
        if let Some(publish) = publish {
            assert_eq!(driver.in_flight(), 2);
            publish();
        }

        let error = second_call
            .await_reply(second_rx)
            .await
            .expect("a store trap must answer queued and reserved sends")
            .expect("a store trap must not drop the reply")
            .expect_err("the pending call must fail with the store");
        assert!(format!("{error:#}").contains("unreachable"), "{error:#}");

        assert_eq!(driver.in_flight(), 1);
        deliver(delivery);
        let error = delivered
            .await
            .expect("a store trap must answer a queued delivery")
            .expect_err("the delivery must fail with the store");
        assert!(error.contains("unreachable"), "{error}");
        assert_eq!(driver.in_flight(), 0);
    }
    Ok(())
}

#[tokio::test]
async fn ephemeral_calls_report_a_callee_that_will_not_instantiate() -> anyhow::Result<()> {
    let ephemeral = Callee::new(UNSTARTABLE, instance_pool::InstancePolicy::Ephemeral)?;
    let pooled = Callee::new(UNSTARTABLE, warm())?;
    let [plain, relocated] = ephemeral.modes("add-one");
    // Only a plain call consults the pool, which builds the instance itself.
    let cases = [
        (&ephemeral, plain),
        (&ephemeral, relocated),
        (&pooled, EphemeralCallMode::PlainValue),
    ];
    for (callee, mode) in cases {
        let call = callee.ephemeral(mode, Default::default()).await?;
        let error = ephemeral_invoke(callee, &call, "add-one", &[Val::U32(1)], 1)
            .await
            .expect_err("a callee whose start traps must fail the call");
        assert!(format!("{error:?}").contains("unreachable"), "{error:?}");
    }
    Ok(())
}

#[tokio::test]
async fn linked_calls_into_a_stopped_workload_are_refused() -> anyhow::Result<()> {
    let mut callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
    let mut ephemeral = Vec::new();
    for mode in callee.modes("add-one") {
        ephemeral.push(
            callee
                .ephemeral(mode, crate::observability::InvocationMeter::new(true))
                .await?,
        );
    }
    // A stopped workload's map is gone before its pool is consulted, so the
    // instance policy makes no difference here.
    callee.components = Arc::new(RwLock::new(BTreeMap::new()));
    for call in &ephemeral {
        let inv = callee.invocation("add-one", LinkedTarget::Ephemeral(Arc::clone(call)));
        assert!(linked_attributes(call, &inv).await.is_empty());
        let error = ephemeral_invoke(&callee, call, "add-one", &[Val::U32(1)], 1)
            .await
            .expect_err("a stopped workload must refuse the call");
        assert!(
            format!("{error:#}")
                .contains("linked component 'callee' belongs to a workload that has stopped"),
            "{error:#}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn linked_calls_are_measured_under_the_callee_and_operation() -> anyhow::Result<()> {
    let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
    let attribute = |attributes: &[opentelemetry::KeyValue], key: &str| {
        attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    };

    let quiet = callee
        .ephemeral(EphemeralCallMode::PlainValue, Default::default())
        .await?;
    let inv = callee.invocation("add-one", LinkedTarget::Ephemeral(Arc::clone(&quiet)));
    assert!(linked_attributes(&quiet, &inv).await.is_empty());

    let measured = callee
        .ephemeral(
            EphemeralCallMode::PlainValue,
            crate::observability::InvocationMeter::new(true),
        )
        .await?;
    let attributes = linked_attributes(&measured, &inv).await;
    assert_eq!(attribute(&attributes, "plugin").as_deref(), Some("linked"));
    assert_eq!(
        attribute(&attributes, "operation").as_deref(),
        Some("contract:callee/api#add-one")
    );
    assert_eq!(
        attribute(&attributes, "component").as_deref(),
        Some("callee")
    );
    assert_eq!(
        attribute(&attributes, "workload.name").as_deref(),
        Some("contract")
    );
    assert_eq!(
        attribute(&attributes, "workload.namespace").as_deref(),
        Some("tests")
    );

    // A callee that has left the map is about to fail its call anyway.
    callee.components.write().await.clear();
    assert!(linked_attributes(&measured, &inv).await.is_empty());
    Ok(())
}

#[tokio::test]
async fn shared_store_async_calls_reach_the_callee_in_the_callers_store() -> anyhow::Result<()> {
    let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
    let mut store = callee.store("caller", Linked::Instantiated).await?;

    let add_one = callee.invocation("add-one", LinkedTarget::SharedStore);
    assert_eq!(
        invoke(&mut store, &add_one, &[Val::U32(41)], 1).await?,
        [Val::U32(42)]
    );
    assert!(
        add_one
            .param_tys
            .get()
            .is_some_and(|tys| matches!(**tys, [Type::U32]))
    );
    let bump = callee.invocation("bump", LinkedTarget::SharedStore);
    for expected in 1..=2 {
        assert_eq!(
            invoke(&mut store, &bump, &[], 1).await?,
            [Val::U32(expected)],
            "the callee lives as long as the caller's store"
        );
    }
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");

    let trap = callee.invocation("trap", LinkedTarget::SharedStore);
    let error = invoke(&mut store, &trap, &[], 0)
        .await
        .expect_err("a trapping callee must fail the call");
    assert!(format!("{error:?}").contains("unreachable"), "{error:?}");
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");

    let mut unlinked = callee.store("caller", Linked::No).await?;
    let error = invoke(&mut unlinked, &add_one, &[Val::U32(1)], 1)
        .await
        .expect_err("a callee with no context in the store must be refused");
    assert!(
        format!("{error:#}").contains("Context for component callee not found"),
        "{error:#}"
    );
    let mut uninstantiated = callee.store("caller", Linked::ContextOnly).await?;
    let error = invoke(&mut uninstantiated, &add_one, &[Val::U32(1)], 1)
        .await
        .expect_err("a callee absent from the store must be refused");
    assert!(
        format!("{error:#}")
            .contains("linked component 'callee' was not pre-instantiated in this store"),
        "{error:#}"
    );
    Ok(())
}

#[cfg(feature = "host-component-plugins")]
mod service {
    use super::*;
    use crate::engine::dispatch::ServiceCalls;
    use crate::host::trigger_service::{CliRunError, Ingress, run_trigger_driver};

    /// Run the service's driver on `calls`, with a fresh incarnation each time
    /// one faults. The watch counts incarnations whose ingress is in place.
    fn serve(
        engine: crate::engine::Engine,
        pre: InstancePre<SharedCtx>,
        calls: Arc<ServiceCalls>,
    ) -> (AbortOnDropHandle<()>, tokio::sync::watch::Receiver<usize>) {
        let (started, incarnations) = tokio::sync::watch::channel(0);
        let server = AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some(rx) = calls.next_incarnation() {
                started.send_modify(|n| *n += 1);
                let mut store = new_store_from_templates(
                    engine.inner(),
                    engine.stream_payloads(),
                    &crate::host::HostRef::detached(),
                    &template(&engine, "service"),
                    &[],
                    &[],
                    true,
                )
                .await
                .expect("service store");
                let _ = run_trigger_driver(
                    &mut store,
                    &pre,
                    vec![Ingress::Guest(rx)],
                    CliRunError::Logged,
                )
                .await;
            }
        }));
        (server, incarnations)
    }

    /// A service call carrying `signature`'s types.
    fn service_call(
        calls: &Arc<ServiceCalls>,
        (param_tys, result_tys): (Arc<[Type]>, Arc<[Type]>),
        plain: bool,
    ) -> LinkedTarget {
        LinkedTarget::Service(Arc::new(ServiceExportCall {
            calls: Arc::clone(calls),
            param_tys,
            result_tys,
            plain,
            attributes: Arc::from([]),
        }))
    }

    #[tokio::test]
    async fn service_calls_run_on_the_running_service() -> anyhow::Result<()> {
        let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
        let calls = Arc::new(ServiceCalls::default());
        let _claim = calls.claim()?;
        let pre = callee.components.read().await["callee"].pre_instantiate_ref()?;
        let (_server, mut incarnations) = serve(callee.engine.clone(), pre, Arc::clone(&calls));
        let mut caller = callee.store("caller", Linked::No).await?;
        let service = |export: &str, signature: &str, plain: bool| {
            callee.invocation(
                export,
                service_call(&calls, callee.signature(signature), plain),
            )
        };

        for plain in [true, false] {
            assert_eq!(
                invoke(
                    &mut caller,
                    &service("add-one", "add-one", plain),
                    &[Val::U32(41)],
                    1
                )
                .await?,
                [Val::U32(42)]
            );

            // Wired as the importer declared it, which the service's export is not.
            let error = invoke(
                &mut caller,
                &service("iface", "add-one", plain),
                &[Val::U32(1)],
                1,
            )
            .await
            .expect_err("an export that is no function must fail the call");
            assert!(
                format!("{error:#}")
                    .contains("function not found for service export contract:callee/api.iface"),
                "{error:#}"
            );

            let restarted = *incarnations.borrow() + 1;
            let error = invoke(&mut caller, &service("trap", "trap", plain), &[], 0)
                .await
                .expect_err("a trapping service export must fail the call");
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("contract:callee/api.trap") && rendered.contains("unreachable"),
                "{rendered}"
            );
            // The trap ended the incarnation; wait for the next one's ingress.
            // Its state starts over, even after the previous round's calls.
            incarnations.wait_for(|n| *n >= restarted).await?;
            for expected in 1..=2 {
                assert_eq!(
                    invoke(&mut caller, &service("bump", "bump", plain), &[], 1).await?,
                    [Val::U32(expected)],
                    "the service keeps its state between calls"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn service_calls_need_a_service_accepting_them() -> anyhow::Result<()> {
        let callee = Callee::new(CALLEE, instance_pool::InstancePolicy::Ephemeral)?;
        let calls = Arc::new(ServiceCalls::default());
        let mut caller = callee.store("caller", Linked::No).await?;
        for plain in [true, false] {
            let add_one = callee.invocation(
                "add-one",
                service_call(&calls, callee.signature("add-one"), plain),
            );
            let error = invoke(&mut caller, &add_one, &[Val::U32(1)], 1)
                .await
                .expect_err("an unclaimed service must refuse the call");
            assert!(
                format!("{error:#}")
                    .contains("the workload's service is not accepting dispatched calls"),
                "{error:#}"
            );
        }
        Ok(())
    }
}
