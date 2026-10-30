use super::*;
use crate::engine::stream_payload::StreamPayloads;
use wasmtime::component::{Component, types::ComponentItem};

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
        trap_hint_logged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    let error = new_store_from_templates(
        engine.inner(),
        engine.stream_payloads(),
        &crate::host::HostRef::detached(),
        &active,
        &[linked],
        &[("callee".into(), pre)],
        false,
    )
    .await
    .err()
    .expect("a trapping start must fail instantiation");
    assert!(
        format!("{error:#}").contains("failed to instantiate linked component 'callee'"),
        "{error:#}"
    );
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
        components: Arc::clone(&components),
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
