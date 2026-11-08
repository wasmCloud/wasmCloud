use super::*;

const SHARED_TYPES: &str = r#"
    (type $text string)
    (import "example:shared/records@1.2.3" (instance
        (export "text" (type (eq $text)))
        (type $message (record (field "value" string)))
        (export "message" (type (eq $message)))
    ))
    (import "example:shared/tags@1.2.3" (instance
        (type $tags (list string))
        (export "tags" (type (eq $tags)))
    ))
"#;

fn fixture(imports: &str) -> anyhow::Result<Vec<u8>> {
    Ok(wat::parse_str(format!(
        r#"(component
            {imports}
            (core module $m (func (export "ping")))
            (core instance $i (instantiate $m))
            (func $ping (canon lift (core func $i "ping")))
            (instance $api (export "ping" (func $ping)))
            (export "example:plugin/service@1.0.0" (instance $api))
        )"#
    ))?)
}

fn imports(workload_calls: bool) -> String {
    let calls = if workload_calls {
        r#"(import "wasmcloud:host/workload-call@0.1.6" (instance))"#
    } else {
        ""
    };
    format!("{SHARED_TYPES} {calls}")
}

struct RefusingSharedTypes;

#[async_trait::async_trait]
impl HostPlugin for RefusingSharedTypes {
    fn id(&self) -> &'static str {
        "refusing-shared-types"
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: std::collections::HashSet::from([WitInterface::from(
                "example:shared/records,tags@1.2.0",
            )]),
            exports: Default::default(),
        }
    }

    fn claims(&self, _interface: &WitInterface) -> bool {
        false
    }
}

#[tokio::test]
async fn shared_types_load_without_a_native_provider() -> anyhow::Result<()> {
    for workload_calls in [false, true] {
        let wasm = fixture(&imports(workload_calls))?;
        let plugin = ComponentHostPlugin::builder()
            .id("shared-types")
            .wasm(&wasm)
            .engine(Engine::builder().with_pooling_allocator(false).build()?)
            .build()
            .await?;
        let world = plugin.world();
        assert!(world.imports.iter().all(|i| i.package != "shared"));
        assert!(world.imports.iter().any(|i| i.package == "plugin"));
    }
    Ok(())
}

#[tokio::test]
async fn shared_types_do_not_depend_on_native_claims() -> anyhow::Result<()> {
    for workload_calls in [true, false] {
        let wasm = fixture(&imports(workload_calls))?;
        let native: Arc<dyn HostPlugin> = Arc::new(RefusingSharedTypes);
        let plugin = ComponentHostPlugin::builder()
            .id("shared-types-with-native")
            .wasm(&wasm)
            .engine(Engine::builder().with_pooling_allocator(false).build()?)
            .native_plugins(HashMap::from([(native.id(), native)]))
            .build()
            .await?;
        assert!(plugin.world().imports.iter().all(|i| i.package != "shared"));
    }
    Ok(())
}

#[tokio::test]
async fn shared_types_do_not_satisfy_capability_imports() -> anyhow::Result<()> {
    for (name, import) in [
        (
            "calls",
            r#"(import "example:shared/calls@1.2.3" (instance
            (export "read" (func))))"#,
        ),
        (
            "handles",
            r#"(import "example:shared/handles@1.2.3" (instance
            (export "handle" (type (sub resource)))))"#,
        ),
        (
            "mixed",
            r#"(import "example:shared/mixed@1.2.3" (instance
            (type $text string) (export "text" (type (eq $text)))
            (export "read" (func))))"#,
        ),
        (
            "marker",
            r#"(import "example:shared/marker@1.2.3" (instance))"#,
        ),
    ] {
        let wasm = fixture(&format!("{SHARED_TYPES} {import}"))?;
        let result = ComponentHostPlugin::builder()
            .id("needs-provider")
            .wasm(&wasm)
            .engine(Engine::builder().with_pooling_allocator(false).build()?)
            .build()
            .await;
        let error = result.err().context("a capability needs a provider")?;
        assert!(format!("{error:#}").contains(name), "{error:#}");
    }
    Ok(())
}

#[tokio::test]
async fn shared_types_do_not_make_a_plugin_workload_facing() -> anyhow::Result<()> {
    let wasm = wat::parse_str(format!("(component {})", imports(true)))?;
    let result = ComponentHostPlugin::builder()
        .id("types-only")
        .wasm(&wasm)
        .engine(Engine::builder().with_pooling_allocator(false).build()?)
        .build()
        .await;
    let error = result
        .err()
        .context("a plugin needs a capability or callback")?;
    assert!(
        error
            .to_string()
            .contains("exports no capability functions")
    );
    Ok(())
}
