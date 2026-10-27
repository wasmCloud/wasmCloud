//! WASI overrides reach services even without a direct capability import.

use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use wash_runtime::{
    engine::{
        Engine,
        workload::{UnresolvedWorkload, WorkloadItem},
    },
    host::{HostBuilder, HostRef},
    plugin::{HostPlugin, WitInterfaces},
    types::{LocalResources, Service, Workload},
    wit::WitWorld,
};

struct Override(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl HostPlugin for Override {
    fn id(&self) -> &'static str {
        "clock-view"
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: ["test:override/opt-in".into()].into(),
            ..Default::default()
        }
    }
    fn wasi_overrides(&self) -> WitWorld {
        WitWorld {
            imports: ["wasi:clocks/monotonic-clock@0.2.0".into()].into(),
            ..Default::default()
        }
    }
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _: WitInterfaces<'_>,
    ) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| anyhow::anyhow!("bind recorder poisoned"))?
            .push(item.id().to_owned());
        item.linker()
            .instance("wasi:clocks/monotonic-clock@0.2.12")?
            .func_wrap("now", |_, (): ()| Ok((4242u64,)))?;
        Ok(())
    }
}

#[tokio::test]
async fn service_using_only_wasi_receives_its_opted_in_override() -> Result<()> {
    for opted_in in [true, false] {
        let engine = Engine::builder().with_pooling_allocator(false).build()?;
        let wasm = wat::parse_str(
            r#"(component
            (import "wasi:clocks/monotonic-clock@0.2.12" (instance))
            (core module $m
                (memory (export "memory") 1)
                (func (export "run") (result i32) i32.const 0))
            (core instance $m (instantiate $m))
            (func $run (result (result))
                (canon lift (core func $m "run") (memory (core memory $m "memory"))))
            (instance $cli (export "run" (func $run)))
            (export "wasi:cli/run@0.2.0" (instance $cli)))"#,
        )?;
        let bound = Arc::new(Mutex::new(Vec::new()));
        let plugin: Arc<dyn HostPlugin> = Arc::new(Override(bound.clone()));
        let unresolved = engine.initialize_workload(
            "service-clock",
            Workload {
                name: "service-clock".into(),
                namespace: "test".into(),
                annotations: HashMap::new(),
                components: vec![],
                volumes: vec![],
                service: Some(Service {
                    bytes: wasm.into(),
                    digest: None,
                    local_resources: LocalResources::default(),
                    max_restarts: 0,
                }),
                host_interfaces: if opted_in {
                    vec!["test:override/opt-in".into()]
                } else {
                    vec![]
                },
            },
        )?;
        let service_id = unresolved
            .service_id()
            .ok_or_else(|| anyhow::anyhow!("missing service"))?;
        let resolved = unresolved
            .resolve(
                Some(&HashMap::from([(plugin.id(), plugin)])),
                &Default::default(),
                &HostRef::detached(),
                &Default::default(),
            )
            .await?;
        let actual = bound
            .lock()
            .map_err(|_| anyhow::anyhow!("bind recorder poisoned"))?
            .clone();
        resolved.unbind_all_plugins().await?;
        let expected = if opted_in {
            vec![service_id.to_string()]
        } else {
            vec![]
        };
        assert_eq!(actual, expected);
    }
    Ok(())
}

struct Declarations(&'static str, Vec<&'static str>);

#[async_trait::async_trait]
impl HostPlugin for Declarations {
    fn id(&self) -> &'static str {
        self.0
    }
    fn world(&self) -> WitWorld {
        WitWorld::default()
    }
    fn wasi_overrides(&self) -> WitWorld {
        WitWorld {
            imports: self.1.iter().map(|entry| (*entry).into()).collect(),
            ..Default::default()
        }
    }
}

fn overlapping_declarations() -> Arc<dyn HostPlugin> {
    Arc::new(Declarations(
        "clock-a",
        vec![
            "wasi:clocks/monotonic-clock@0.2.0",
            "wasi:clocks/monotonic-clock,wall-clock@0.2.12",
        ],
    ))
}

#[test]
fn compatible_overlapping_declarations_from_one_plugin_are_allowed() -> Result<()> {
    let plugin = overlapping_declarations();
    wash_runtime::plugin::check_wasi_overrides(&HashMap::from([(plugin.id(), plugin)]))
}

#[test]
fn overlapping_declarations_still_conflict_with_another_plugin() -> Result<()> {
    let a = overlapping_declarations();
    let b: Arc<dyn HostPlugin> = Arc::new(Declarations(
        "clock-b",
        vec!["wasi:clocks/wall-clock@0.2.0"],
    ));
    let Err(error) =
        wash_runtime::plugin::check_wasi_overrides(&HashMap::from([(a.id(), a), (b.id(), b)]))
    else {
        anyhow::bail!("conflicting plugins were accepted");
    };
    let message = error.to_string();
    assert!(
        message.contains("clock-a") && message.contains("clock-b"),
        "{message}"
    );
    Ok(())
}

const INVALID_OVERRIDES: &[&str] = &[
    "wasi:otel/tracing@0.2.0-rc.2",
    "wasi:blobstore/blobstore@0.2.0-draft",
    "wasi:keyvalue",
    "wasi:config",
    "wasi:logging",
    "test:override/opt-in",
    "wasi:clocks/monotonic-clock@0.4.0",
    "wasi:io/streams@0.3.0",
];

const VALID_OVERRIDES: &[&str] = &[
    "wasi:filesystem/types@0.2.0",
    "wasi:clocks/monotonic-clock@0.3.0",
];

fn assert_unsupported_override(error: anyhow::Error, interface: &str) {
    let message = error.to_string();
    for expected in [
        "override-plugin",
        interface,
        "plugin-served",
        "another plugin",
    ] {
        assert!(
            message.contains(expected),
            "missing {expected:?} in {message:?}"
        );
    }
}

#[test]
fn unsupported_wasi_overrides_are_refused_at_registration() -> Result<()> {
    for interface in INVALID_OVERRIDES {
        let plugin = Arc::new(Declarations("override-plugin", vec![interface]));
        let Err(error) = HostBuilder::new().with_plugin(plugin) else {
            anyhow::bail!("registration accepted unsupported override {interface}");
        };
        assert_unsupported_override(error, interface);
    }
    Ok(())
}

#[tokio::test]
async fn unsupported_wasi_overrides_are_refused_at_bind() -> Result<()> {
    for interface in INVALID_OVERRIDES {
        let plugin: Arc<dyn HostPlugin> =
            Arc::new(Declarations("override-plugin", vec![interface]));
        // Bypass HostBuilder to exercise bind-time validation independently,
        // even when no workload item would bind the plugin.
        let mut workload = UnresolvedWorkload::new("test", "test", "test", None, vec![], vec![]);
        let Err(error) = workload
            .bind_plugins(&HashMap::from([(plugin.id(), plugin)]), &Default::default())
            .await
        else {
            anyhow::bail!("binding accepted unsupported override {interface}");
        };
        assert_unsupported_override(error, interface);
    }
    Ok(())
}

#[test]
fn host_builtin_wasi_overrides_are_allowed_at_registration() -> Result<()> {
    for interface in VALID_OVERRIDES {
        HostBuilder::new()
            .with_plugin(Arc::new(Declarations("override-plugin", vec![interface])))?;
    }
    Ok(())
}

#[tokio::test]
async fn host_builtin_wasi_overrides_are_allowed_at_bind() -> Result<()> {
    for interface in VALID_OVERRIDES {
        let plugin: Arc<dyn HostPlugin> =
            Arc::new(Declarations("override-plugin", vec![interface]));
        let mut workload = UnresolvedWorkload::new("test", "test", "test", None, vec![], vec![]);
        workload
            .bind_plugins(&HashMap::from([(plugin.id(), plugin)]), &Default::default())
            .await?;
    }
    Ok(())
}
