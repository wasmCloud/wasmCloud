use super::*;
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

fn signature(payload: &str, aggregate: &str) -> ComponentFunc {
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model(true)
        .wasm_component_model_async(true)
        .wasm_component_model_error_context(true)
        .wasm_component_model_map(true)
        .wasm_component_model_fixed_length_lists(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let bytes = wat::parse_str(format!(
        r#"(component
        (import "resource" (type $resource (sub resource)))
        (type $payload-type {payload})
        (import "payload" (type $payload (eq $payload-type)))
        (type $value-type {aggregate})
        (import "value" (type $value (eq $value-type)))
        (type $call (func (param "value" $value) (result u32)))
        (import "call" (func (type $call))))"#
    ))
    .unwrap();
    let component = Component::new(&engine, bytes)
        .unwrap_or_else(|error| panic!("{payload} in {aggregate}: {error:#}"));
    let function = component
        .component_type()
        .imports(&engine)
        .find_map(|(_, item)| match item.ty {
            ComponentItem::ComponentFunc(function) => Some(function),
            _ => None,
        })
        .unwrap();
    function
}

#[test]
fn linked_signature_routing_checks_nested_streams_and_store_bound_handles() {
    let payloads = super::super::stream_payload::StreamPayloads::default();
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
            let function = signature(payload, aggregate);
            assert_eq!(
                func_is_ephemeral_safe(&function),
                ephemeral,
                "{payload} in {aggregate}"
            );
            assert_eq!(
                func_is_bridge_safe(&function, &payloads),
                bridge,
                "{payload} in {aggregate}"
            );
            #[cfg(feature = "host-component-plugins")]
            {
                let params = function.params().map(|(_, ty)| ty).collect::<Vec<_>>();
                assert_eq!(types_are_ephemeral_safe(&params), ephemeral);
                assert_eq!(types_are_bridge_safe(&params, &payloads), bridge);
            }
        }
    }
}

#[test]
fn typed_payload_registration_enables_only_the_registered_stream_shape() -> anyhow::Result<()> {
    let function = signature("(stream (tuple u32 string))", "(option $payload)");
    let ty = function.params().next().unwrap().1;
    let Type::Option(option) = ty else {
        unreachable!()
    };
    let Type::Stream(stream) = option.ty() else {
        unreachable!()
    };
    let element = stream.ty().unwrap();
    let payloads = super::super::stream_payload::StreamPayloads::default();
    assert!(!func_is_bridge_safe(&function, &payloads));
    payloads.register::<(u32, String)>(element.clone())?;
    assert!(func_is_bridge_safe(&function, &payloads));
    assert!(payloads.register::<(u32, String)>(element).is_err());
    assert!(!func_is_ephemeral_safe(&function));
    Ok(())
}

fn template(engine: &crate::engine::Engine, id: &str) -> ComponentCtxTemplate {
    ComponentCtxTemplate {
        component_id: id.into(),
        workload_id: "contract-workload".into(),
        local_resources: crate::types::LocalResources {
            environment: HashMap::from([("COMPONENT".into(), id.into())]),
            ..Default::default()
        },
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

#[tokio::test]
async fn linked_store_templates_share_identity_and_restore_the_active_component()
-> anyhow::Result<()> {
    let engine = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .with_fuel_consumption(true)
        .build()?;
    let active = template(&engine, "caller");
    let linked = template(&engine, "callee");
    let component = Component::new(engine.inner(), wat::parse_str("(component)")?)?;
    let pre = wasmtime::component::Linker::new(engine.inner()).instantiate_pre(&component)?;
    let mut store = new_store_from_templates(
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
    store.data_mut().set_active_ctx(&Arc::from("callee"))?;
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "callee");
    store.data_mut().set_active_ctx(&Arc::from("caller"))?;
    assert_eq!(store.data().active_ctx.component_id.as_ref(), "caller");
    Ok(())
}

#[tokio::test]
async fn linked_store_instantiation_failures_name_the_callee() -> anyhow::Result<()> {
    let engine = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
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
        true,
    )
    .await
    .err()
    .unwrap();
    assert!(
        format!("{error:#}").contains("failed to instantiate linked component 'callee'"),
        "{error:#}"
    );
    Ok(())
}

#[test]
fn dynamic_results_check_output_capacity() -> anyhow::Result<()> {
    let mut results = [Val::Bool(false), Val::Bool(false)];
    write_results(
        vec![Val::U32(42), Val::String("result".into())],
        &mut results,
    )?;
    assert!(matches!(results.first(), Some(Val::U32(42))));
    assert!(matches!(results.get(1), Some(Val::String(s)) if s == "result"));
    assert!(write_results(vec![Val::U32(1)], &mut []).is_err());
    Ok(())
}
