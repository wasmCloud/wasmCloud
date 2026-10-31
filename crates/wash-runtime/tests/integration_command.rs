//! Commands preserve invocation I/O and report exits without service restarts.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context as _, Result};
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
    let command = Component {
        name: "command".into(),
        bytes: wasm,
        local_resources: LocalResources {
            environment,
            ..Default::default()
        },
        ..Default::default()
    };
    resolve_components(vec![command], host_interfaces, plugins).await
}

/// Resolve `components` as one workload, returning the ID of the one named
/// `command`.
async fn resolve_components(
    components: Vec<Component>,
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
                components,
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
        .iter()
        .find(|(_, component)| component.name() == "command")
        .map(|(id, _)| id.clone())
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

/// How a constructor's core start function ends.
#[derive(Clone, Copy, Debug)]
enum Start {
    ExitWithCode(u8),
    Exit { ok: bool },
    Trap,
}

/// A component whose core start function ends the way `start` says, and
/// which exports a `run` that would succeed as `export`.
fn constructor(exit_version: &str, start: Start, export: &str) -> Result<Vec<u8>> {
    let (func, params, body) = match start {
        Start::ExitWithCode(code) => (
            "exit-with-code",
            r#"(param "status-code" u8)"#,
            format!("i32.const {code} call $exit"),
        ),
        Start::Exit { ok } => (
            "exit",
            r#"(param "status" (result))"#,
            format!("i32.const {} call $exit", u8::from(!ok)),
        ),
        Start::Trap => ("exit", r#"(param "status" (result))"#, "unreachable".into()),
    };
    Ok(wat::parse_str(format!(
        r#"(component
        (import "wasi:cli/exit@{exit_version}" (instance $exit
            (export "{func}" (func {params}))))
        (alias export $exit "{func}" (func $exit))
        (core func $exit (canon lower (func $exit)))
        (core module $m
            (import "exit" "exit" (func $exit (param i32)))
            (memory (export "memory") 1)
            (func $start {body}) (start $start)
            (func (export "run") (result i32) i32.const 0))
        (core instance $m (instantiate $m (with "exit" (instance (export "exit" (func $exit))))))
        (func $run (result (result)) (canon lift (core func $m "run") (memory (core memory $m "memory"))))
        (instance $cli (export "run" (func $run)))
        (export "{export}" (instance $cli)))"#
    ))?)
}

/// A command that runs `test:constructor/probe` from a linked library as its
/// own `wasi:cli/run@{run_version}`.
fn linked_command(run_version: &str) -> Result<Vec<u8>> {
    Ok(wat::parse_str(format!(
        r#"(component
        (import "test:constructor/probe@1.0.0"
            (instance (export "run" (func (result (result))))))
        (core module $m
            (func (export "run") (result i32) i32.const 0))
        (core instance $m (instantiate $m))
        (func $run (result (result)) (canon lift (core func $m "run")))
        (instance $cli (export "run" (func $run)))
        (export "wasi:cli/run@{run_version}" (instance $cli)))"#
    ))?)
}

/// An exit from a core start function, in the command or in a library it
/// links, is the command's outcome in both WASI previews, and a trap there is
/// still a command trap.
#[tokio::test]
async fn constructor_exits_are_outcomes_and_traps_are_errors() -> Result<()> {
    for (exit_version, run_version) in [("0.2.12", "0.2.0"), ("0.3.0", "0.3.0")] {
        for linked in [false, true] {
            for (start, expected) in [
                (Start::ExitWithCode(0), Some(0)),
                (Start::ExitWithCode(23), Some(23)),
                (Start::Exit { ok: true }, Some(0)),
                (Start::Exit { ok: false }, Some(1)),
                (Start::Trap, None),
            ] {
                let case = format!("WASI {run_version}, linked={linked}, {start:?}");
                let components = if linked {
                    vec![
                        Component {
                            name: "library".into(),
                            bytes: constructor(
                                exit_version,
                                start,
                                "test:constructor/probe@1.0.0",
                            )?
                            .into(),
                            ..Default::default()
                        },
                        Component {
                            name: "command".into(),
                            bytes: linked_command(run_version)?.into(),
                            ..Default::default()
                        },
                    ]
                } else {
                    vec![Component {
                        name: "command".into(),
                        bytes: constructor(
                            exit_version,
                            start,
                            &format!("wasi:cli/run@{run_version}"),
                        )?
                        .into(),
                        ..Default::default()
                    }]
                };
                let (resolved, id) = resolve_components(components, Vec::new(), None)
                    .await
                    .with_context(|| case.clone())?;
                let result = resolved.run_command(&id, CommandOptions::new()).await;
                resolved.unbind_all_plugins().await?;
                match expected {
                    Some(code) => {
                        assert_eq!(
                            result.with_context(|| case.clone())?.exit_code,
                            code,
                            "{case}"
                        )
                    }
                    None => {
                        let Err(error) = result else {
                            anyhow::bail!("{case}: constructor trap succeeded");
                        };
                        assert_eq!(
                            error.downcast_ref::<wasmtime::Trap>(),
                            Some(&wasmtime::Trap::UnreachableCodeReached),
                            "{case}: {error:#}"
                        );
                        assert!(format!("{error:#}").contains("command trapped"), "{case}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// A command the host cannot set up is an error, but not a trap: here the
/// `run` export has the wrong type, so instantiation fails before any guest
/// code runs.
#[tokio::test]
async fn setup_failures_are_not_traps() -> Result<()> {
    let wasm = wat::parse_str(
        r#"(component
        (core module $m (func (export "run") (result i32) i32.const 0))
        (core instance $m (instantiate $m))
        (func $run (result u32) (canon lift (core func $m "run")))
        (instance $cli (export "run" (func $run)))
        (export "wasi:cli/run@0.2.0" (instance $cli)))"#,
    )?;
    let (resolved, id) = resolve(wasm.into(), HashMap::new(), Vec::new(), None).await?;
    let result = resolved.run_command(&id, CommandOptions::new()).await;
    resolved.unbind_all_plugins().await?;
    let Err(error) = result else {
        anyhow::bail!("mistyped run export succeeded");
    };
    let message = format!("{error:#}");
    assert!(message.contains("type mismatch"), "{message}");
    assert!(!message.contains("command trapped"), "{message}");
    Ok(())
}
