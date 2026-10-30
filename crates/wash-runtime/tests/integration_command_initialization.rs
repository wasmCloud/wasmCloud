//! Constructor exits are normal command outcomes in both WASI previews.

use anyhow::Result;
use std::collections::HashMap;
use wash_runtime::{
    engine::{Engine, WasmProposal, command::CommandOptions},
    host::HostRef,
    types::{Component, Workload},
};

#[tokio::test]
async fn initialization_exits_preserve_status_and_traps_remain_errors() -> Result<()> {
    for (exit_version, run_version) in [("0.2.12", "0.2.0"), ("0.3.0", "0.3.0")] {
        for (exit_code, linked) in [Some(0), Some(23), None]
            .into_iter()
            .flat_map(|code| [false, true].map(|linked| (code, linked)))
        {
            let export = if linked {
                "test:constructor/probe@1.0.0".to_string()
            } else {
                format!("wasi:cli/run@{run_version}")
            };
            let start = match exit_code {
                Some(code) => format!("i32.const {code} call $exit"),
                None => "unreachable".into(),
            };
            let wasm = wat::parse_str(format!(
                r#"(component
                (import "wasi:cli/exit@{exit_version}" (instance $exit
                    (export "exit-with-code" (func (param "status-code" u8)))))
                (alias export $exit "exit-with-code" (func $exit))
                (core func $exit (canon lower (func $exit)))
                (core module $m
                    (import "exit" "exit" (func $exit (param i32)))
                    (memory (export "memory") 1)
                    (func $start {start}) (start $start)
                    (func (export "run") (result i32) i32.const 0))
                (core instance $m (instantiate $m (with "exit" (instance (export "exit" (func $exit))))))
                (func $run (result (result)) (canon lift (core func $m "run") (memory (core memory $m "memory"))))
                (instance $cli (export "run" (func $run)))
                (export "{export}" (instance $cli)))"#
            ))?;
            let mut components = vec![Component {
                name: if linked { "library" } else { "command" }.into(),
                bytes: wasm.into(),
                ..Default::default()
            }];
            if linked {
                components.push(Component {
                    name: "command".into(),
                    bytes: wat::parse_str(format!(
                        r#"(component
                            (import "test:constructor/probe@1.0.0"
                                (instance (export "run" (func (result (result))))))
                            (core module $m
                                (func (export "run") (result i32) i32.const 0))
                            (core instance $m (instantiate $m))
                            (func $run (result (result)) (canon lift (core func $m "run")))
                            (instance $cli (export "run" (func $run)))
                            (export "wasi:cli/run@{run_version}" (instance $cli)))"#
                    ))?
                    .into(),
                    ..Default::default()
                });
            }
            let engine = Engine::builder()
                .with_pooling_allocator(false)
                .with_wasm_proposal(WasmProposal::ComponentModelAsync)
                .build()?;
            let resolved = engine
                .initialize_workload(
                    "constructor-exit",
                    Workload {
                        name: "constructor-exit".into(),
                        namespace: "test".into(),
                        annotations: HashMap::new(),
                        service: None,
                        volumes: vec![],
                        host_interfaces: vec![],
                        components,
                    },
                )?
                .resolve(
                    None,
                    &Default::default(),
                    &HostRef::detached(),
                    &Default::default(),
                )
                .await?;
            let id = resolved
                .components()
                .read_owned()
                .await
                .values()
                .find(|component| component.name() == "command")
                .map(|component| component.id().to_string())
                .ok_or_else(|| anyhow::anyhow!("missing command"))?;
            let result = resolved.run_command(&id, CommandOptions::default()).await;
            resolved.unbind_all_plugins().await?;
            match exit_code {
                Some(expected) => assert_eq!(
                    result?.exit_code, expected,
                    "WASI {run_version}, linked={linked}"
                ),
                None => {
                    let Err(error) = result else {
                        anyhow::bail!("constructor trap succeeded");
                    };
                    assert!(format!("{error:#}").contains("command trapped"));
                }
            }
        }
    }
    Ok(())
}
