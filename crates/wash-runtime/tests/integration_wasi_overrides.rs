//! WASI overrides reach services even without a direct capability import.

use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use wash_runtime::{
    engine::{Engine, workload::WorkloadItem},
    host::HostRef,
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
