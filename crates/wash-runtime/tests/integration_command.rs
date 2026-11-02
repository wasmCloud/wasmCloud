//! Commands preserve invocation I/O and report exits without service restarts.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use wash_runtime::{
    engine::{
        Engine,
        command::CommandOptions,
        workload::{ResolvedWorkload, WorkloadItem},
    },
    plugin::{HostPlugin, WitInterfaces},
    types::{Component, LocalResources, Workload},
    wit::WitWorld,
};
use wasmtime_wasi::{
    cli::WasiCliView,
    p2::{
        bindings::cli::{environment, stderr, stdin, stdout},
        pipe::{MemoryInputPipe, MemoryOutputPipe},
    },
};

/// A WASIp3 command built from `fixtures/command-p3`: it reports its
/// arguments, environment and stdin on stdout, writes `diagnostic` to stderr,
/// and ends the way `EXIT_MODE` says.
const COMMAND_P3_WASM: &[u8] = include_bytes!("wasm/command_p3.wasm");

struct Probe {
    calls: Arc<AtomicUsize>,
    exit: Option<i32>,
}

#[async_trait::async_trait]
impl HostPlugin for Probe {
    fn id(&self) -> &'static str {
        "command-probe"
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: ["test:command/probe".into()].into(),
            ..Default::default()
        }
    }
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _: WitInterfaces<'_>,
    ) -> Result<()> {
        let calls = self.calls.clone();
        let exit = self.exit;
        item.linker()?.instance("test:command/probe")?.func_wrap(
            "check",
            move |mut store, (): ()| {
                calls.fetch_add(1, Ordering::SeqCst);
                let mut cli = store.data_mut().cli();
                assert_eq!(
                    environment::Host::get_arguments(&mut cli)?,
                    ["command", "one", "two"]
                );
                assert_eq!(
                    environment::Host::get_environment(&mut cli)?,
                    [("FROM_MANIFEST".into(), "yes".into())]
                );
                let input = stdin::Host::get_stdin(&mut cli)?;
                let data = cli.table.get_mut(&input)?.read(1024)?;
                assert_eq!(&data[..], b"hello");
                let output = stdout::Host::get_stdout(&mut cli)?;
                cli.table.get_mut(&output)?.write(data)?;
                let error = stderr::Host::get_stderr(&mut cli)?;
                cli.table
                    .get_mut(&error)?
                    .write(bytes::Bytes::from_static(b"diagnostic"))?;
                match exit {
                    Some(code) => Err(wasmtime_wasi::I32Exit(code).into()),
                    None => Ok(()),
                }
            },
        )?;
        Ok(())
    }
}

async fn resolve(
    wasm: bytes::Bytes,
    environment: HashMap<String, String>,
    host_interfaces: Vec<wash_runtime::wit::WitInterface>,
    plugins: Option<&HashMap<&'static str, Arc<dyn HostPlugin>>>,
) -> Result<(ResolvedWorkload, Arc<str>)> {
    let engine = Engine::builder().with_pooling_allocator(false).build()?;
    let resolved = engine
        .initialize_workload(
            "command-test",
            Workload {
                name: "command-test".into(),
                namespace: "test".into(),
                annotations: HashMap::new(),
                service: None,
                volumes: Vec::new(),
                host_interfaces,
                components: vec![Component {
                    name: "command".into(),
                    bytes: wasm,
                    local_resources: LocalResources {
                        environment,
                        ..Default::default()
                    },
                    ..Default::default()
                }],
            },
        )?
        .resolve(
            plugins,
            &Default::default(),
            &wash_runtime::host::HostRef::detached(),
            &Default::default(),
        )
        .await?;
    let id = resolved
        .components()
        .read_owned()
        .await
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing command"))?;
    Ok((resolved, id))
}

/// A WASIp2 command whose `run` calls the probe and then returns `body`.
async fn p2_workload(
    body: &str,
    exit: Option<i32>,
) -> Result<(ResolvedWorkload, Arc<str>, Arc<AtomicUsize>)> {
    let wasm = wat::parse_str(format!(
        r#"(component
        (import "test:command/probe" (instance $probe (export "check" (func))))
        (alias export $probe "check" (func $check))
        (core func $check (canon lower (func $check)))
        (core module $m
            (import "probe" "check" (func $check))
            (memory (export "memory") 1)
            (func (export "run") (result i32)
                call $check {body}))
        (core instance $m (instantiate $m (with "probe" (instance (export "check" (func $check))))))
        (func $run (result (result)) (canon lift (core func $m "run") (memory (core memory $m "memory"))))
        (instance $cli (export "run" (func $run)))
        (export "wasi:cli/run@0.2.0" (instance $cli)))"#
    ))?;
    let calls = Arc::new(AtomicUsize::new(0));
    let probe: Arc<dyn HostPlugin> = Arc::new(Probe {
        calls: calls.clone(),
        exit,
    });
    let (resolved, id) = resolve(
        wasm.into(),
        HashMap::from([("FROM_MANIFEST".into(), "yes".into())]),
        vec!["test:command/probe".into()],
        Some(&HashMap::from([(probe.id(), probe)])),
    )
    .await?;
    Ok((resolved, id, calls))
}

async fn p3_workload(exit_mode: &str) -> Result<(ResolvedWorkload, Arc<str>)> {
    resolve(
        bytes::Bytes::from_static(COMMAND_P3_WASM),
        HashMap::from([
            ("FROM_MANIFEST".into(), "yes".into()),
            ("EXIT_MODE".into(), exit_mode.into()),
        ]),
        Vec::new(),
        None,
    )
    .await
}

fn options(stdout: &MemoryOutputPipe, stderr: &MemoryOutputPipe) -> CommandOptions {
    CommandOptions::new()
        .args(["command", "one", "two"])
        .stdin(MemoryInputPipe::new("hello"))
        .stdout(stdout.clone())
        .stderr(stderr.clone())
}

#[tokio::test]
async fn command_io_and_exit_status() -> Result<()> {
    for (body, explicit_exit, expected) in [
        ("i32.const 0", None, 0),
        ("i32.const 1", None, 1),
        ("i32.const 0", Some(23), 23),
        ("i32.const 0", Some(0), 0),
    ] {
        let (resolved, id, calls) = p2_workload(body, explicit_exit).await?;
        for run in 1..=2 {
            let stdout = MemoryOutputPipe::new(1024);
            let stderr = MemoryOutputPipe::new(1024);
            let outcome = resolved.run_command(&id, options(&stdout, &stderr)).await?;
            assert_eq!(outcome.exit_code, expected);
            assert_eq!(calls.load(Ordering::SeqCst), run);
            assert_eq!(&stdout.contents()[..], b"hello");
            assert_eq!(&stderr.contents()[..], b"diagnostic");
        }
        resolved.unbind_all_plugins().await?;
    }
    Ok(())
}

/// The WASIp3 path runs `wasi:cli/run@0.3` under `run_concurrent`, serves the
/// invocation's streams through the p3 `wasi:cli` host, and reports both a
/// returned status and a `wasi:cli/exit` raised from inside the guest.
#[tokio::test]
async fn wasip3_command_io_and_exit_status() -> Result<()> {
    for (exit_mode, expected) in [
        ("return-ok", 0),
        ("return-err", 1),
        // Each `exit-*` mode would return the opposite status if the exit
        // were not honored, so the code proves `wasi:cli/exit` was taken.
        ("exit-ok", 0),
        ("exit-err", 1),
    ] {
        let (resolved, id) = p3_workload(exit_mode).await?;
        for _ in 1..=2 {
            let stdout = MemoryOutputPipe::new(4096);
            let stderr = MemoryOutputPipe::new(1024);
            let outcome = resolved.run_command(&id, options(&stdout, &stderr)).await?;
            assert_eq!(outcome.exit_code, expected, "exit mode {exit_mode}");
            let report = format!(
                "args=command one two\nenv=EXIT_MODE={exit_mode},FROM_MANIFEST=yes\nstdin=hello\n"
            );
            assert_eq!(
                String::from_utf8_lossy(&stdout.contents()),
                report,
                "exit mode {exit_mode}"
            );
            assert_eq!(&stderr.contents()[..], b"diagnostic");
        }
        resolved.unbind_all_plugins().await?;
    }
    Ok(())
}

#[tokio::test]
async fn traps_are_errors_and_are_not_retried() -> Result<()> {
    let (resolved, id, calls) = p2_workload("unreachable", None).await?;
    let stdout = MemoryOutputPipe::new(1024);
    let stderr = MemoryOutputPipe::new(1024);
    let result = resolved.run_command(&id, options(&stdout, &stderr)).await;
    let Err(error) = result else {
        anyhow::bail!("trap unexpectedly succeeded");
    };
    assert!(format!("{error:#}").contains("command trapped"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    resolved.unbind_all_plugins().await?;
    Ok(())
}
