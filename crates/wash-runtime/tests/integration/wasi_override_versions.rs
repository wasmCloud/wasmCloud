//! Partial overrides reopen the host instance across compatible WIT revisions.

use anyhow::Result;
use std::{collections::HashMap, sync::Arc};
use wash_runtime::{
    engine::{Engine, WasmProposal, workload::WorkloadItem},
    host::HostRef,
    plugin::{HostPlugin, WitInterfaces},
    types::{Component, Workload},
    wit::WitWorld,
};

struct ClockOverride(&'static str);

#[async_trait::async_trait]
impl HostPlugin for ClockOverride {
    fn id(&self) -> &'static str {
        "clock-override"
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: ["test:override/opt-in".into()].into(),
            ..Default::default()
        }
    }
    fn wasi_overrides(&self) -> WitWorld {
        WitWorld {
            imports: [format!("wasi:clocks/monotonic-clock@{}", self.0)
                .as_str()
                .into()]
            .into(),
            ..Default::default()
        }
    }
    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        _: WitInterfaces<'_>,
    ) -> Result<()> {
        assert!(
            item.wasi_override_instance("wasi:clocks/monotonic-clock")
                .is_err()
        );
        assert!(
            item.wasi_override_instance("wasi:clocks/monotonic-clock@0.4.0")
                .is_err()
        );
        assert!(
            item.wasi_override_instance("test:override/opt-in@0.2.0")
                .is_err()
        );
        item.wasi_override_instance(&format!("wasi:clocks/monotonic-clock@{}", self.0))?
            .func_wrap("now", |_, (): ()| Ok((4242u64,)))?;
        Ok(())
    }
}

#[tokio::test]
async fn compatible_partial_overrides_preserve_other_host_functions() -> Result<()> {
    for (imported, overridden) in [
        ("0.2.0", "0.2.0"),
        ("0.2.11", "0.2.0"),
        ("0.2.12", "0.2.0"),
        ("0.2.12", "0.2.12"),
        ("0.3.0", "0.3.0"),
    ] {
        let host_resolution = if imported == "0.3.0" {
            "get-resolution"
        } else {
            "resolution"
        };
        let wasm = wat::parse_str(format!(
            r#"(component
            (import "wasi:clocks/monotonic-clock@{imported}" (instance $clock
                (export "now" (func (result u64)))
                (export "{host_resolution}" (func (result u64)))))
            (alias export $clock "now" (func $now))
            (alias export $clock "{host_resolution}" (func $resolution))
            (core func $now (canon lower (func $now)))
            (core func $resolution (canon lower (func $resolution)))
            (core module $m
                (import "clock" "now" (func $now (result i64)))
                (import "clock" "resolution" (func $resolution (result i64)))
                (func (export "now") (result i64) call $now)
                (func (export "resolution") (result i64) call $resolution))
            (core instance $m (instantiate $m (with "clock" (instance
                (export "now" (func $now)) (export "resolution" (func $resolution))))))
            (func $now-export (result u64) (canon lift (core func $m "now")))
            (func $resolution-export (result u64) (canon lift (core func $m "resolution")))
            (instance $probe (export "now" (func $now-export)) (export "resolution" (func $resolution-export)))
            (export "test:override/probe" (instance $probe)))"#
        ))?;
        let engine = Engine::builder()
            .with_pooling_allocator(false)
            .with_wasm_proposal(WasmProposal::ComponentModelAsync)
            .build()?;
        let plugin: Arc<dyn HostPlugin> = Arc::new(ClockOverride(overridden));
        let resolved = engine
            .initialize_workload(
                "clock-version",
                Workload {
                    name: "clock-version".into(),
                    namespace: "test".into(),
                    annotations: HashMap::new(),
                    service: None,
                    volumes: vec![],
                    host_interfaces: vec!["test:override/opt-in".into()],
                    components: vec![Component {
                        name: "clock".into(),
                        bytes: wasm.into(),
                        ..Default::default()
                    }],
                },
            )?
            .resolve(
                Some(&HashMap::from([(plugin.id(), plugin)])),
                &Default::default(),
                &HostRef::detached(),
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
            .ok_or_else(|| anyhow::anyhow!("missing component"))?;
        let mut store = resolved.new_store(&id).await?;
        let instance = resolved
            .instantiate_pre(&id)
            .await?
            .instantiate_async(&mut store)
            .await?;
        let (_, api) = instance
            .get_export(&mut store, None, "test:override/probe")
            .ok_or_else(|| anyhow::anyhow!("missing probe"))?;
        let (_, now) = instance
            .get_export(&mut store, Some(&api), "now")
            .ok_or_else(|| anyhow::anyhow!("missing now"))?;
        let now = instance
            .get_func(&mut store, now)
            .ok_or_else(|| anyhow::anyhow!("missing now function"))?
            .typed::<(), (u64,)>(&store)?;
        assert_eq!(
            now.call_async(&mut store, ()).await?,
            (4242,),
            "guest {imported}, override {overridden}"
        );
        let (_, resolution) = instance
            .get_export(&mut store, Some(&api), "resolution")
            .ok_or_else(|| anyhow::anyhow!("missing resolution"))?;
        let resolution = instance
            .get_func(&mut store, resolution)
            .ok_or_else(|| anyhow::anyhow!("missing resolution function"))?
            .typed::<(), (u64,)>(&store)?;
        // This function was not overridden and must remain callable.
        resolution.call_async(&mut store, ()).await?;
        resolved.unbind_all_plugins().await?;
    }
    Ok(())
}
