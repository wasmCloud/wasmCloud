use super::contract_tests::{API, EXPORTER, component, engine, resolve};
use super::*;

struct HostResource;

fn store(component: &WorkloadComponent) -> wasmtime::Store<SharedCtx> {
    let ctx = crate::engine::ctx::Ctx::builder("contracts", component.id()).build();
    wasmtime::Store::new(engine(), SharedCtx::new(ctx))
}

#[test]
fn rejected_wasi_overrides_preserve_the_linker_and_cached_link() -> anyhow::Result<()> {
    let invalid = "a WASI override must name one host-linked interface at a supported version";
    let unversioned = "a WASI override requires an explicit version to select WASIp2 or WASIp3";
    for (request, diagnostic) in [
        ("", invalid),
        ("test:workload/api@0.2.0", invalid),
        ("wasi:keyvalue/store@0.2.0", invalid),
        ("wasi:clocks@0.2.0", invalid),
        ("wasi:clocks/monotonic-clock,wall-clock@0.2.0", invalid),
        ("wasi:clocks/monotonic-clock", unversioned),
        ("wasi:clocks/monotonic-clock@invalid", unversioned),
        ("wasi:clocks/monotonic-clock@0.4.0", invalid),
        ("wasi:clocks/monotonic-clock@1.0.0", invalid),
        ("wasi:clocks/monotonic-clock@0.2.0-preview", invalid),
        ("wasi:io/streams@0.3.0", invalid),
    ] {
        let mut importer = component("override", "(component)");
        importer.pre_instantiate_ref()?;
        let LinkState::Open { linker, .. } = &importer.link else {
            panic!("linker must be open");
        };
        let original = Arc::clone(linker);
        let error = importer
            .wasi_override_instance(request)
            .err()
            .expect("override must fail");
        assert_eq!(error.to_string(), diagnostic, "{request}");
        let LinkState::Open {
            linker,
            pre,
            written,
        } = &importer.link
        else {
            panic!("rejection changed linker state for {request}");
        };
        assert!(!written, "{request}");
        assert!(Arc::ptr_eq(linker, &original), "{request}");
        assert!(pre.get().is_some(), "{request}");
    }
    Ok(())
}

async fn value(
    instance: &wasmtime::component::Instance,
    store: &mut wasmtime::Store<SharedCtx>,
    name: &str,
) -> anyhow::Result<u64> {
    let function = instance.get_typed_func::<(), (u64,)>(&mut *store, name)?;
    let (value,) = function.call_async(store, ()).await?;
    Ok(value)
}

#[tokio::test]
async fn compatible_wasi_overrides_replace_only_the_selected_host_definition() -> anyhow::Result<()>
{
    for (minor, compatible) in [(2, "0.2.0"), (3, "0.3.1")] {
        let version = super::super::WASI_LINKER_VERSIONS
            .into_iter()
            .find(|version| version.major == 0 && version.minor == minor)
            .unwrap();
        for requested in [compatible.to_string(), version.to_string()] {
            for imported in [compatible.to_string(), version.to_string()] {
                let exact_clock = format!("wasi:clocks/monotonic-clock@{version}");
                let exact_other = format!("wasi:clocks/wall-clock@{version}");
                let mut importer = component(
                    "override",
                    &format!(
                        r#"(component
                    (import "wasi:clocks/monotonic-clock@{imported}" (instance $clock
                        (export "now" (func (result u64)))
                        (export "resolution" (func (result u64)))
                        (export "token" (type (sub resource)))))
                    (import "wasi:clocks/wall-clock@{imported}" (instance $other
                        (export "marker" (func (result u64)))))
                    (alias export $clock "now" (func $now))
                    (alias export $clock "resolution" (func $resolution))
                    (alias export $clock "token" (type $token))
                    (alias export $other "marker" (func $marker))
                    (core func $now (canon lower (func $now)))
                    (core func $resolution (canon lower (func $resolution)))
                    (core func $marker (canon lower (func $marker)))
                    (core module $m
                        (import "host" "now" (func $now (result i64)))
                        (import "host" "resolution" (func $resolution (result i64)))
                        (import "host" "marker" (func $marker (result i64)))
                        (func (export "now") (result i64) call $now)
                        (func (export "resolution") (result i64) call $resolution)
                        (func (export "marker") (result i64) call $marker))
                    (core instance $m (instantiate $m (with "host" (instance
                        (export "now" (func $now)) (export "resolution" (func $resolution))
                        (export "marker" (func $marker))))))
                    (func $now-export (result u64) (canon lift (core func $m "now")))
                    (func $resolution-export (result u64) (canon lift (core func $m "resolution")))
                    (func $marker-export (result u64) (canon lift (core func $m "marker")))
                    (export "now" (func $now-export))
                    (export "resolution" (func $resolution-export))
                    (export "token" (type $token))
                    (export "marker" (func $marker-export)))"#
                    ),
                );
                {
                    let mut clock = importer.linker()?.instance(&exact_clock)?;
                    clock.func_wrap("now", |_, (): ()| Ok((7_u64,)))?;
                    clock.func_wrap("resolution", |_, (): ()| Ok((13_u64,)))?;
                    clock.resource("token", ResourceType::host::<HostResource>(), |_, _| Ok(()))?;
                }
                importer
                    .linker()?
                    .instance(&exact_other)?
                    .func_wrap("marker", |_, (): ()| Ok((17_u64,)))?;
                importer.linker()?.allow_shadowing(true);
                let mut before = store(&importer);
                let instance = importer
                    .pre_instantiate_ref()?
                    .instantiate_async(&mut before)
                    .await?;
                assert_eq!(value(&instance, &mut before, "now").await?, 7);

                importer
                    .wasi_override_instance(&format!("wasi:clocks/monotonic-clock@{requested}"))?
                    .func_wrap("now", |_, (): ()| Ok((99_u64,)))?;
                assert!(importer.seal_linker());
                let mut after = store(&importer);
                let instance = importer
                    .pre_instantiate_ref()?
                    .instantiate_async(&mut after)
                    .await?;
                assert_eq!(value(&instance, &mut after, "now").await?, 99);
                assert_eq!(value(&instance, &mut after, "resolution").await?, 13);
                assert_eq!(value(&instance, &mut after, "marker").await?, 17);
                assert_eq!(
                    instance.get_resource(&mut after, "token"),
                    Some(ResourceType::host::<HostResource>())
                );
            }
        }
    }
    Ok(())
}

#[test]
fn sealed_linkers_refuse_wasi_overrides() -> anyhow::Result<()> {
    for version in super::super::WASI_LINKER_VERSIONS {
        let mut importer = component("override", "(component)");
        importer.seal_linker();
        let error = importer
            .wasi_override_instance(&format!("wasi:clocks/monotonic-clock@{version}"))
            .err()
            .expect("sealed linker accepted an override");
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains("is sealed; plugins must bind in on_workload_item_bind"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains(importer.id()), "{diagnostic}");
        assert!(importer.is_sealed());
    }
    Ok(())
}

fn resource_exporter(name: &str) -> WorkloadComponent {
    component(
        "exporter",
        &format!(
            r#"(component
        (type $resource (resource (rep i32)))
        (instance $api (export "{name}" (type $resource)))
        (export "test:workload/api@0.1.0" (instance $api)))"#
        ),
    )
}

fn resource_importer(name: &str) -> WorkloadComponent {
    component(
        "importer",
        &format!(
            r#"(component
        (import "test:workload/api@0.1.0" (instance $api
            (export "{name}" (type (sub resource)))))
        (alias export $api "{name}" (type $resource))
        (export "linked-resource" (type $resource)))"#
        ),
    )
}

async fn link_imports(
    importer: &mut WorkloadComponent,
    exporter: WorkloadComponent,
    bound: HashSet<String>,
) -> anyhow::Result<()> {
    let exports = build_export_map(&[(
        ComponentExporter {
            name: Arc::from(exporter.name()),
            id: Arc::from(exporter.id()),
        },
        vec![API.to_string()],
    )]);
    let resolved = resolve(None, [exporter]).await?;
    let compiled = importer.component().clone();
    let id = importer.id().to_string();
    let linked = resolved
        .resolve_component_imports(
            &compiled,
            &mut importer.metadata,
            &exports,
            &bound,
            Some(&id),
        )
        .await?;
    assert!(linked.is_empty());
    Ok(())
}

async fn assert_resource(
    importer: &WorkloadComponent,
    name: &str,
    expected: ResourceType,
) -> anyhow::Result<()> {
    let mut store = store(importer);
    let instance = importer
        .pre_instantiate_ref()?
        .instantiate_async(&mut store)
        .await?;
    assert_eq!(instance.get_resource(&mut store, name), Some(expected));
    Ok(())
}

fn missing_resource(importer: &WorkloadComponent, name: &str) {
    let diagnostic = format!(
        "{:#}",
        importer
            .pre_instantiate_ref()
            .err()
            .expect("missing resource linked")
    );
    assert!(diagnostic.contains(API), "{diagnostic}");
    assert!(diagnostic.contains(&format!("`{name}`")), "{diagnostic}");
    assert!(diagnostic.contains("not found"), "{diagnostic}");
}

#[tokio::test]
async fn ordinary_resource_imports_receive_a_bridge_resource_type() -> anyhow::Result<()> {
    let mut importer = resource_importer("value");
    link_imports(&mut importer, resource_exporter("value"), HashSet::new()).await?;
    assert_resource(
        &importer,
        "linked-resource",
        ResourceType::host::<ResourceAny>(),
    )
    .await
}

const HOST_RESOURCES: [&str; 5] = [
    "output-stream",
    "input-stream",
    "pollable",
    "tcp-socket",
    "incoming-value-async-body",
];

#[tokio::test]
async fn reserved_resource_names_preserve_host_types_and_require_a_host_binding()
-> anyhow::Result<()> {
    for name in HOST_RESOURCES {
        for host_bound in [false, true] {
            let mut importer = resource_importer(name);
            if host_bound {
                importer.linker()?.instance(API)?.resource(
                    name,
                    ResourceType::host::<HostResource>(),
                    |_, _| Ok(()),
                )?;
            }
            link_imports(&mut importer, resource_exporter(name), HashSet::new()).await?;
            if host_bound {
                assert_resource(
                    &importer,
                    "linked-resource",
                    ResourceType::host::<HostResource>(),
                )
                .await?;
            } else {
                missing_resource(&importer, name);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_existing_resource_definition_survives_duplicate_guest_linking() -> anyhow::Result<()> {
    let mut importer = resource_importer("value");
    importer.linker()?.instance(API)?.resource(
        "value",
        ResourceType::host::<HostResource>(),
        |_, _| Ok(()),
    )?;
    link_imports(&mut importer, resource_exporter("value"), HashSet::new()).await?;
    assert_resource(
        &importer,
        "linked-resource",
        ResourceType::host::<HostResource>(),
    )
    .await
}

#[tokio::test]
async fn missing_and_non_resource_exports_leave_a_resource_import_unresolved() -> anyhow::Result<()>
{
    for exporter in [
        component(
            "empty",
            r#"(component (instance $api)
            (export "test:workload/api@0.1.0" (instance $api)))"#,
        ),
        component("function", &EXPORTER.replace("\"f\"", "\"value\"")),
    ] {
        let mut importer = resource_importer("value");
        link_imports(&mut importer, exporter, HashSet::new()).await?;
        missing_resource(&importer, "value");
    }
    Ok(())
}

#[tokio::test]
async fn plugin_bound_resource_imports_are_not_filled_from_guest_exports() -> anyhow::Result<()> {
    for host_bound in [false, true] {
        let mut importer = resource_importer("value");
        if host_bound {
            importer.linker()?.instance(API)?.resource(
                "value",
                ResourceType::host::<HostResource>(),
                |_, _| Ok(()),
            )?;
        }
        link_imports(
            &mut importer,
            resource_exporter("value"),
            HashSet::from([API.to_string()]),
        )
        .await?;
        if host_bound {
            assert_resource(
                &importer,
                "linked-resource",
                ResourceType::host::<HostResource>(),
            )
            .await?;
        } else {
            missing_resource(&importer, "value");
        }
    }
    Ok(())
}

#[tokio::test]
async fn root_resource_imports_need_host_definitions_without_blocking_instance_resources()
-> anyhow::Result<()> {
    let mut importer = component(
        "root",
        r#"(component
        (import "root-value" (type $root (sub resource)))
        (import "test:workload/api@0.1.0" (instance $api (export "value" (type (sub resource)))))
        (alias export $api "value" (type $resource))
        (export "root-resource" (type $root))
        (export "linked-resource" (type $resource)))"#,
    );
    link_imports(&mut importer, resource_exporter("value"), HashSet::new()).await?;
    let diagnostic = format!(
        "{:#}",
        importer
            .pre_instantiate_ref()
            .err()
            .expect("root resource linked without a host")
    );
    assert!(diagnostic.contains("root-value"), "{diagnostic}");
    assert!(
        diagnostic.contains("resource implementation is missing"),
        "{diagnostic}"
    );
    importer.linker()?.root().resource(
        "root-value",
        ResourceType::host::<HostResource>(),
        |_, _| Ok(()),
    )?;
    assert_resource(
        &importer,
        "root-resource",
        ResourceType::host::<HostResource>(),
    )
    .await?;
    assert_resource(
        &importer,
        "linked-resource",
        ResourceType::host::<ResourceAny>(),
    )
    .await
}
