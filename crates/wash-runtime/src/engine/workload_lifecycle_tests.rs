use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum Exit {
    Success,
    Error,
    TrapOnce,
}

async fn p2_attempts(exit: Exit, max_restarts: u64) -> anyhow::Result<usize> {
    let runtime = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
    let engine = runtime.inner();
    let status = u32::from(matches!(exit, Exit::Error));
    let component = Component::new(
        engine,
        wat::parse_str(format!(
            r#"(component
      (import "test:probe/host@0.1.0" (instance $host (export "probe" (func))))
      (alias export $host "probe" (func $probe))
      (core func $probe (canon lower (func $probe)))
      (core module $m
        (import "host" "probe" (func $probe))
        (func (export "run") (result i32) call $probe i32.const {status}))
      (core instance $m (instantiate $m (with "host" (instance (export "probe" (func $probe))))))
      (func $run (result (result)) (canon lift (core func $m "run")))
      (instance $cli (export "run" (func $run)))
      (export "wasi:cli/run@0.2.0" (instance $cli)))"#
        ))?,
    )?;
    let mut service = WorkloadService::new(
        "restarts",
        "restarts",
        "test",
        component,
        Linker::new(engine),
        vec![],
        LocalResources::default(),
        max_restarts,
        Arc::default(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    service
        .metadata
        .linker()?
        .instance("test:probe/host@0.1.0")?
        .func_wrap("probe", {
            let calls = Arc::clone(&calls);
            move |_, (): ()| -> wasmtime::Result<()> {
                let attempt = calls.fetch_add(1, Ordering::SeqCst);
                if matches!(exit, Exit::TrapOnce) && attempt == 0 {
                    return Err(wasmtime::format_err!("first invocation traps"));
                }
                Ok(())
            }
        })?;
    let mut resolved = resolve_service(service).await?;
    let handle = resolved
        .execute_service()
        .await?
        .context("service task missing")?;
    resolved.service_mut().context("service missing")?.handle = None;
    let handle =
        Arc::try_unwrap(handle).map_err(|_| anyhow::anyhow!("service task still shared"))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), handle).await??;
    Ok(calls.load(Ordering::SeqCst))
}

async fn resolve_service(service: WorkloadService) -> anyhow::Result<ResolvedWorkload> {
    UnresolvedWorkload::new("lifecycle", "lifecycle", "test", Some(service), [], vec![])
        .resolve(
            None,
            &Default::default(),
            &crate::host::HostRef::detached(),
            &Default::default(),
        )
        .await
}

#[tokio::test]
async fn p2_error_results_consume_the_restart_budget() -> anyhow::Result<()> {
    for budget in [0, 1, 3] {
        assert_eq!(p2_attempts(Exit::Error, budget).await?, budget as usize + 1);
    }
    Ok(())
}

#[tokio::test]
async fn p2_traps_restart_in_a_fresh_store() -> anyhow::Result<()> {
    assert_eq!(p2_attempts(Exit::TrapOnce, 1).await?, 2);
    Ok(())
}

#[tokio::test]
async fn p2_success_does_not_restart() -> anyhow::Result<()> {
    assert_eq!(p2_attempts(Exit::Success, 3).await?, 1);
    Ok(())
}

const P3_COMMAND: &str = r#"(component
  (core module $m
    START
    (func (export "run") (result i32) i32.const 0)
    (func (export "callback") (param i32 i32 i32) (result i32) i32.const 0))
  (core instance $m (instantiate $m))
  (func $run async RESULT
    (canon lift (core func $m "run") async (callback (core func $m "callback"))))
  (instance $cli (export "run" (func $run)))
  (export "wasi:cli/run@0.3.0" (instance $cli)))"#;

#[tokio::test]
async fn p3_instantiation_traps_fail_startup() -> anyhow::Result<()> {
    for trigger in [false, true] {
        let runtime = crate::engine::Engine::builder()
            .with_pooling_allocator(false)
            .build()?;
        let engine = runtime.inner();
        let wat = P3_COMMAND
            .replace("START", "(func $start unreachable) (start $start)")
            .replace("RESULT", "(result (result))");
        let service = WorkloadService::new(
            "lifecycle",
            "lifecycle",
            "test",
            Component::new(engine, wat::parse_str(wat)?)?,
            Linker::new(engine),
            vec![],
            LocalResources::default(),
            0,
            Arc::default(),
        );
        service.pre_instantiate_p3()?;
        let mut resolved = resolve_service(service).await?;
        let _claim = trigger
            .then(|| resolved.service_calls.claim())
            .transpose()?;
        let err = resolved
            .execute_service()
            .await
            .expect_err("an instantiation trap must fail startup");
        assert!(format!("{err:#}").contains("unreachable"), "{err:#}");
        assert!(
            resolved
                .service
                .as_ref()
                .is_some_and(|s| s.handle.is_none())
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_trigger_service_rejects_a_malformed_cli_export() -> anyhow::Result<()> {
    let runtime = crate::engine::Engine::builder()
        .with_pooling_allocator(false)
        .build()?;
    let engine = runtime.inner();
    let wat = P3_COMMAND.replace("START", "").replace("RESULT", "");
    let service = WorkloadService::new(
        "lifecycle",
        "lifecycle",
        "test",
        Component::new(engine, wat::parse_str(wat)?)?,
        Linker::new(engine),
        vec![],
        LocalResources::default(),
        0,
        Arc::default(),
    );
    let mut resolved = resolve_service(service).await?;
    let _claim = resolved.service_calls.claim()?;
    let err = resolved
        .execute_service()
        .await
        .expect_err("a malformed cli/run must fail startup");
    assert!(format!("{err:#}").contains("failed to bind"), "{err:#}");
    Ok(())
}

#[tokio::test]
async fn p3_commands_run_after_initialization() -> anyhow::Result<()> {
    for trigger in [false, true] {
        let runtime = crate::engine::Engine::builder()
            .with_pooling_allocator(false)
            .build()?;
        let engine = runtime.inner();
        let component = Component::new(
            engine,
            wat::parse_str(
                r#"(component
          (import "test:probe/host@0.1.0" (instance $host (export "probe" (func))))
          (alias export $host "probe" (func $probe))
          (core func $probe (canon lower (func $probe)))
          (core func $return (canon task.return (result (result))))
          (core module $m
            (import "host" "probe" (func $probe))
            (import "host" "return" (func $return (param i32)))
            (func (export "run") (result i32)
              call $probe i32.const 0 call $return i32.const 0)
            (func (export "callback") (param i32 i32 i32) (result i32) unreachable))
          (core instance $m (instantiate $m (with "host" (instance
            (export "probe" (func $probe)) (export "return" (func $return))))))
          (func $run async (result (result)) (canon lift (core func $m "run")
            async (callback (core func $m "callback"))))
          (instance $cli (export "run" (func $run)))
          (export "wasi:cli/run@0.3.0" (instance $cli)))"#,
            )?,
        )?;
        let mut service = WorkloadService::new(
            "lifecycle",
            "lifecycle",
            "test",
            component,
            Linker::new(engine),
            vec![],
            LocalResources::default(),
            0,
            Arc::default(),
        );
        let ran = Arc::new(tokio::sync::Notify::new());
        service
            .metadata
            .linker()?
            .instance("test:probe/host@0.1.0")?
            .func_wrap("probe", {
                let ran = Arc::clone(&ran);
                move |_, (): ()| {
                    ran.notify_one();
                    Ok(())
                }
            })?;
        let mut resolved = resolve_service(service).await?;
        let _claim = trigger
            .then(|| resolved.service_calls.claim())
            .transpose()?;
        resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        tokio::time::timeout(std::time::Duration::from_secs(5), ran.notified()).await?;
        resolved.begin_teardown();
    }
    Ok(())
}
