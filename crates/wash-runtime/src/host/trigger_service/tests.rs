use super::*;

#[tokio::test]
async fn a_trigger_service_without_cli_run_is_valid() -> anyhow::Result<()> {
    let runtime = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
    let engine = runtime.inner();
    let component = wasmtime::component::Component::new(engine, wat::parse_str("(component)")?)?;
    let pre = wasmtime::component::Linker::<SharedCtx>::new(engine).instantiate_pre(&component)?;
    let data = SharedCtx::new(crate::engine::ctx::Ctx::builder("no-run", "no-run").build());
    let mut store = Store::new(engine, data);
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(tx);
    run_trigger_driver(
        &mut store,
        &pre,
        vec![Ingress::Guest(rx)],
        CliRunError::Fatal,
    )
    .await
}

/// One plugin store serves every workload, so a fault one workload's call
/// causes must not carry its cause to another workload's call.
#[cfg(feature = "host-component-plugins")]
#[tokio::test]
async fn a_plugin_fault_keeps_its_cause_from_other_workloads() -> anyhow::Result<()> {
    use crate::engine::abandon::DispatchedCall;
    use crate::engine::ctx::WorkloadCaller;

    const PLUGIN: &str = r#"(component
        (core module $m (func (export "trap") unreachable))
        (core instance $i (instantiate $m))
        (func $trap (canon lift (core func $i "trap")))
        (instance $plugin (export "trap" (func $trap)))
        (export "plugin" (instance $plugin)))"#;
    let runtime = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
    let engine = runtime.inner();
    let component = wasmtime::component::Component::new(engine, wat::parse_str(PLUGIN)?)?;
    let pre = wasmtime::component::Linker::<SharedCtx>::new(engine).instantiate_pre(&component)?;
    let data = SharedCtx::new(crate::engine::ctx::Ctx::builder("plugin", "plugin").build());
    let mut store = Store::new(engine, data);
    let caller = |workload: &str| WorkloadCaller {
        workload_id: workload.into(),
        component_id: None,
        binding: None,
    };

    // Workload B's call waits in the channel while workload A's replayed
    // bind traps, which ends the store before the channel is read.
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (reply, queued) = tokio::sync::oneshot::channel();
    let call = DispatchedCall::new("queued capability call", std::time::Duration::from_secs(5));
    tx.send(CapabilityJob::Call(CapabilityCall {
        interface: "plugin".into(),
        func: "trap".into(),
        caller: CallerIdentity::Workload(caller("workload-b")),
        args: vec![],
        result_tys: Arc::from([]),
        reply,
        abandoned: call.flag(),
    }))
    .await
    .map_err(|_| anyhow::anyhow!("the channel must take the call"))?;
    let ingress = Ingress::Capability {
        funcs: vec![CapabilityFunc {
            interface: "plugin".into(),
            func: "trap".into(),
        }],
        rx,
        registry: JobRegistry::new(),
        replay: vec![LifecycleReplay {
            interface: "plugin".into(),
            func: "trap".into(),
            caller: caller("workload-a"),
            args: vec![],
            result_tys: Arc::from([]),
        }],
        secret_source: None,
    };
    let driven = run_trigger_driver(&mut store, &pre, vec![ingress], CliRunError::Logged);
    let fault = tokio::time::timeout(std::time::Duration::from_secs(10), driven)
        .await?
        .expect_err("the replayed bind must trap");
    assert!(format!("{fault:#}").contains("unreachable"), "{fault:#}");

    let error = match queued.await? {
        Ok(_) => anyhow::bail!("the queued call must not run"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("host component plugin faulted"), "{error}");
    assert!(!error.contains("unreachable"), "{error}");
    drop(tx);
    Ok(())
}
