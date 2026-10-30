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
