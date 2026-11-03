use super::*;

pub(super) const API: &str = "test:workload/api@0.1.0";
pub(super) const EXPORTER: &str = r#"(component
    (core module $m (func (export "f") (result i32) i32.const 42))
    (core instance $m (instantiate $m))
    (func $f (result u32) (canon lift (core func $m "f")))
    (instance $api (export "f" (func $f)))
    (export "test:workload/api@0.1.0" (instance $api)))"#;

pub(super) fn engine() -> &'static wasmtime::Engine {
    static ENGINE: std::sync::OnceLock<wasmtime::Engine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        crate::init_crypto();
        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_error_context(true);
        wasmtime::Engine::new(&config).unwrap()
    })
}

pub(super) fn component(name: &str, wat: &str) -> WorkloadComponent {
    WorkloadComponent::new(
        "contracts",
        "contracts",
        "test",
        name,
        Component::new(engine(), wat::parse_str(wat).unwrap()).unwrap(),
        Linker::new(engine()),
        vec![],
        LocalResources::default(),
        Arc::default(),
        InstancePolicy::Ephemeral,
    )
}

pub(super) fn service(wat: &str) -> WorkloadService {
    WorkloadService::new(
        "contracts",
        "contracts",
        "test",
        Component::new(engine(), wat::parse_str(wat).unwrap()).unwrap(),
        Linker::new(engine()),
        vec![],
        LocalResources::default(),
        0,
        Arc::default(),
    )
}

pub(super) async fn resolve(
    service: Option<WorkloadService>,
    components: impl IntoIterator<Item = WorkloadComponent>,
) -> anyhow::Result<ResolvedWorkload> {
    UnresolvedWorkload::new(
        "contracts",
        "contracts",
        "test",
        service,
        components,
        vec![],
    )
    .resolve(
        None,
        &Default::default(),
        &crate::host::HostRef::detached(),
        &Default::default(),
    )
    .await
}

#[tokio::test]
async fn ambiguous_imports_fail_only_when_a_component_uses_them() -> anyhow::Result<()> {
    let exporters = || [component("first", EXPORTER), component("second", EXPORTER)];
    assert_eq!(resolve(None, exporters()).await?.component_count(), 2);

    let importer = component(
        "caller",
        r#"(component (import "test:workload/api@0.1.0"
            (instance (export "f" (func (result u32))))))"#,
    );
    let error = resolve(None, exporters().into_iter().chain([importer]))
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains(API), "{message}");
    assert!(
        message.contains("exported by multiple components"),
        "{message}"
    );
    assert!(message.contains("(implements ..)"), "{message}");
    Ok(())
}

#[tokio::test]
async fn circular_imports_fail_before_instantiation() {
    let components = [
        component(
            "first",
            r#"(component
                (import "test:workload/second@0.1.0" (instance))
                (instance $api)
                (export "test:workload/first@0.1.0" (instance $api)))"#,
        ),
        component(
            "second",
            r#"(component
                (import "test:workload/first@0.1.0" (instance))
                (instance $api)
                (export "test:workload/second@0.1.0" (instance $api)))"#,
        ),
    ];
    let error = resolve(None, components).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("circular dependency"),
        "{error:#}"
    );
}

#[tokio::test]
async fn failed_linking_restores_the_importer_for_teardown() -> anyhow::Result<()> {
    for service_import in [false, true] {
        let exporter = component(
            "broken",
            r#"(component
                (import "test:workload/missing@0.1.0" (instance $host (export "f" (func))))
                (alias export $host "f" (func $host-f))
                (core func $host-f (canon lower (func $host-f)))
                (core module $m
                    (import "host" "f" (func $host-f))
                    (func (export "f") call $host-f))
                (core instance $m
                    (instantiate $m (with "host" (instance (export "f" (func $host-f))))))
                (func $f (canon lift (core func $m "f")))
                (instance $api (export "f" (func $f)))
                (export "test:workload/api@0.1.0" (instance $api)))"#,
        );
        let wat =
            r#"(component (import "test:workload/api@0.1.0" (instance (export "f" (func)))))"#;
        let mut components = vec![exporter];
        let (service, importer_id) = if service_import {
            let service = service(wat);
            let id = service.id().to_string();
            (Some(service), id)
        } else {
            let importer = component("caller", wat);
            let id = importer.id().to_string();
            components.push(importer);
            (None, id)
        };
        let resources = WorkloadResources::default();
        let error = UnresolvedWorkload::new(
            "contracts",
            "contracts",
            "test",
            service,
            components,
            vec![],
        )
        .resolve_for_start(
            None,
            &Default::default(),
            &crate::host::HostRef::detached(),
            &Default::default(),
            Some(&resources),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("failed to pre-instantiate during component linking"),
            "{error:#}"
        );
        let resolved = resources.state.lock().unwrap().resolved.clone().unwrap();
        if service_import {
            assert_eq!(resolved.service_id(), Some(importer_id.as_str()));
        } else {
            assert!(
                resolved
                    .components
                    .read()
                    .await
                    .contains_key(importer_id.as_str())
            );
        }
        resources.release("contracts").await?;
        assert!(resolved.released());
        assert!(resources.state.lock().unwrap().resolved.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn an_exporter_missing_a_function_cannot_dispatch() -> anyhow::Result<()> {
    let exporter = component(
        "empty",
        r#"(component (instance $api)
            (export "test:workload/api@0.1.0" (instance $api)))"#,
    );
    let importer = component(
        "caller",
        r#"(component (import "test:workload/api@0.1.0"
            (instance (export "f" (func (result u32))))))"#,
    );
    let id = importer.id().to_string();
    let resolved = resolve(None, [exporter, importer]).await?;
    let error = resolved.component_dispatch(&id).await.err().unwrap();
    let message = format!("{error:#}");
    assert!(
        message.contains("cannot be pre-instantiated to dispatch to"),
        "{message}"
    );
    assert!(message.contains(API), "{message}");
    Ok(())
}

#[tokio::test]
async fn an_exported_type_cannot_satisfy_a_function_import() {
    let exporter = component(
        "wrong-kind",
        r#"(component
            (type $value (record (field "value" u32)))
            (instance $api (export "f" (type $value)))
            (export "test:workload/api@0.1.0" (instance $api)))"#,
    );
    let importer = component(
        "caller",
        r#"(component (import "test:workload/api@0.1.0"
            (instance (export "f" (func (result u32))))))"#,
    );
    let error = resolve(None, [exporter, importer]).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("expected function export, found other"),
        "{error:#}"
    );
}

#[tokio::test]
async fn non_instance_items_do_not_become_interface_dependencies() -> anyhow::Result<()> {
    let component = component(
        "raw-items",
        r#"(component
            (import "resource" (type (sub resource)))
            (import "callback" (func))
            (core module $m (func (export "f")))
            (core instance $m (instantiate $m))
            (func $f (canon lift (core func $m "f")))
            (export "f" (func $f)))"#,
    );
    let id = component.id().to_string();
    assert!(component.world().imports.is_empty());
    assert!(component.world().exports.is_empty());
    assert!(component.component_exports()?.is_empty());
    let resolved = resolve(None, [component]).await?;
    let components = resolved.components.read().await;
    assert!(components[&*id].metadata.linked_components.is_empty());
    assert!(components[&*id].pre_instantiate_ref().is_err());
    Ok(())
}

#[tokio::test]
async fn service_mounts_are_pinned_and_reused() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("volume");
    std::fs::create_dir(&path)?;
    let mut service = service("(component)");
    service.metadata.volume_mounts.push((
        path.clone(),
        VolumeMount {
            name: "data".into(),
            mount_path: "/data".into(),
            read_only: true,
        },
    ));
    let mut resolved = resolve(Some(service), []).await?;
    resolved.resolve_service_volume_mounts().await?;
    assert_eq!(
        resolved
            .service
            .as_ref()
            .unwrap()
            .metadata
            .resolved_volume_mounts
            .len(),
        1
    );
    // Unix permits removing an open directory; Windows keeps it locked.
    #[cfg(unix)]
    std::fs::remove_dir(&path)?;
    // Reopening the configured source must fail on every platform.
    let service = resolved.service_mut().context("service is missing")?;
    service.metadata.volume_mounts[0].0 = directory.path().join("missing");
    resolved.resolve_service_volume_mounts().await?;
    assert_eq!(
        resolved
            .service
            .as_ref()
            .unwrap()
            .metadata
            .resolved_volume_mounts
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn invalid_service_mounts_fail_without_caching_partial_results() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let mut service = service("(component)");
    for path in [
        directory.path().to_path_buf(),
        directory.path().join("missing"),
    ] {
        service.metadata.volume_mounts.push((
            path,
            VolumeMount {
                name: "data".into(),
                mount_path: "/data".into(),
                read_only: false,
            },
        ));
    }
    let mut resolved = resolve(Some(service), []).await?;
    assert!(resolved.resolve_service_volume_mounts().await.is_err());
    assert!(
        resolved
            .service
            .as_ref()
            .unwrap()
            .metadata
            .resolved_volume_mounts
            .is_empty()
    );
    std::fs::create_dir(directory.path().join("missing"))?;
    resolved.resolve_service_volume_mounts().await?;
    assert_eq!(
        resolved
            .service
            .as_ref()
            .unwrap()
            .metadata
            .resolved_volume_mounts
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn service_store_recipes_reject_missing_linked_components() -> anyhow::Result<()> {
    let resolved = resolve(Some(service("(component)")), []).await?;
    let mut metadata = resolved.service.as_ref().unwrap().metadata.clone();
    metadata.linked_components.insert(Arc::from("missing"));
    let error = resolved
        .service_store_recipe(&metadata)
        .await
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("linked component 'missing' not found"),
        "{error:#}"
    );
    Ok(())
}

#[cfg(feature = "host-component-plugins")]
mod external_calls {
    use super::*;
    use wasmtime::component::types::{ComponentFunc, Type};

    const SERVICE_EXPORTER: &str = r#"(component
        (core module $m
            (func (export "f") (result i32) i32.const 42)
            (func (export "run") (result i32) i32.const 0)
            (func (export "callback") (param i32 i32 i32) (result i32) i32.const 0))
        (core instance $m (instantiate $m))
        (func $f (result u32) (canon lift (core func $m "f")))
        (instance $api (export "f" (func $f)))
        (export "test:workload/api@0.1.0" (instance $api))
        (func $run async (result (result))
            (canon lift (core func $m "run") async (callback (core func $m "callback"))))
        (instance $cli (export "run" (func $run)))
        (export "wasi:cli/run@0.3.0" (instance $cli)))"#;

    fn signature(payload: &str, result: bool) -> ComponentFunc {
        let signature = if result {
            "(result $payload)"
        } else {
            "(param \"value\" $payload)"
        };
        let component = Component::new(
            engine(),
            wat::parse_str(format!(
                r#"(component
                (import "resource" (type $resource (sub resource)))
                (type $payload-type {payload})
                (import "payload" (type $payload (eq $payload-type)))
                (type $f (func {signature}))
                (import "f" (func (type $f))))"#,
            ))
            .unwrap(),
        )
        .unwrap();
        component
            .component_type()
            .imports(engine())
            .find_map(|(_, item)| match item.ty {
                ComponentItem::ComponentFunc(func) => Some(func),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn component_routes_require_the_item_interface_and_function() -> anyhow::Result<()> {
        let component = component("callee", EXPORTER);
        let id = component.id().to_string();
        let resolved = resolve(None, [component]).await?;
        let funcs = [ExternalCallFunc {
            name: "absent",
            param_tys: &[],
            result_tys: &[],
        }];
        for (item, interface, functions, expected) in [
            ("missing", API, &[][..], "component ID not found"),
            (
                id.as_str(),
                "test:workload/missing@0.1.0",
                &[][..],
                "does not export",
            ),
            (id.as_str(), API, &funcs[..], "but not absent"),
        ] {
            let error = resolved
                .external_export_invocations(item, interface, functions)
                .await
                .err()
                .unwrap();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        assert!(matches!(
            resolved
                .item_exporting("missing", &WitInterface::from(API))
                .await,
            ItemExport::None
        ));
        Ok(())
    }

    #[tokio::test]
    async fn service_routes_require_the_interface_and_function() -> anyhow::Result<()> {
        let resolved = resolve(Some(service(SERVICE_EXPORTER)), []).await?;
        let claim = resolved.claim_service_dispatch()?;
        let funcs = [ExternalCallFunc {
            name: "absent",
            param_tys: &[],
            result_tys: &[],
        }];
        for (interface, functions, expected) in [
            ("test:workload/missing@0.1.0", &[][..], "does not export"),
            (API, &funcs[..], "but not absent"),
        ] {
            let error = resolved
                .service_export_invocations(&claim, interface, functions)
                .err()
                .unwrap();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn routes_classify_both_parameters_and_results() -> anyhow::Result<()> {
        let callee = component("callee", EXPORTER);
        let id = callee.id().to_string();
        let resolved = resolve(Some(service(SERVICE_EXPORTER)), [callee]).await?;
        let claim = resolved.claim_service_dispatch()?;
        for payload in ["u32", "(stream u8)", "(future string)"] {
            for result in [false, true] {
                let signature = signature(payload, result);
                let params = signature.params().map(|(_, ty)| ty).collect::<Vec<_>>();
                let results = signature.results().collect::<Vec<_>>();
                let funcs = [ExternalCallFunc {
                    name: "f",
                    param_tys: &params,
                    result_tys: &results,
                }];
                let component_routes = resolved
                    .external_export_invocations(&id, API, &funcs)
                    .await?;
                let route = &component_routes["f"];
                assert_eq!(route.import_name.as_ref(), API);
                assert_eq!(route.export_name.as_ref(), "f");
                assert_eq!(route.plugin_component_id.as_ref(), id);
                let LinkedTarget::Ephemeral(call) = &route.target else {
                    panic!("expected component route")
                };
                match &call.mode {
                    EphemeralCallMode::PlainValue => assert_eq!(payload, "u32"),
                    EphemeralCallMode::Relocated {
                        param_tys,
                        result_tys,
                    } => {
                        assert_ne!(payload, "u32");
                        assert_eq!(&**param_tys, params);
                        assert_eq!(&**result_tys, results);
                    }
                }
                let service_routes = resolved.service_export_invocations(&claim, API, &funcs)?;
                let LinkedTarget::Service(call) = &service_routes["f"].target else {
                    panic!("expected service route")
                };
                assert_eq!(call.plain, payload == "u32");
                assert_eq!(&*call.param_tys, params);
                assert_eq!(&*call.result_tys, results);
                assert!(Arc::ptr_eq(&call.calls, claim.calls()));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn store_bound_handles_and_unregistered_payloads_fail_deployment() -> anyhow::Result<()> {
        let callee = component("callee", EXPORTER);
        let id = callee.id().to_string();
        let resolved = resolve(Some(service(SERVICE_EXPORTER)), [callee]).await?;
        let claim = resolved.claim_service_dispatch()?;
        for payload in [
            "(own $resource)",
            "error-context",
            "(stream (tuple u32 string))",
        ] {
            for result in [false, true] {
                let signature = signature(payload, result);
                let params = signature.params().map(|(_, ty)| ty).collect::<Vec<_>>();
                let results = signature.results().collect::<Vec<_>>();
                let funcs = [ExternalCallFunc {
                    name: "f",
                    param_tys: &params,
                    result_tys: &results,
                }];
                let component_error = resolved
                    .external_export_invocations(&id, API, &funcs)
                    .await
                    .err()
                    .unwrap();
                let service_error = resolved
                    .service_export_invocations(&claim, API, &funcs)
                    .err()
                    .unwrap();
                for error in [component_error, service_error] {
                    let message = error.to_string();
                    assert!(
                        message.contains(&format!("{API}#f carries a handle")),
                        "{message}"
                    );
                    assert!(
                        message.contains("Engine::stream_payloads().register"),
                        "{message}"
                    );
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn registered_payloads_enable_component_and_service_routes() -> anyhow::Result<()> {
        let callee = component("callee", EXPORTER);
        let id = callee.id().to_string();
        let resolved = resolve(Some(service(SERVICE_EXPORTER)), [callee]).await?;
        let claim = resolved.claim_service_dispatch()?;
        let signature = signature("(stream (tuple u32 string))", false);
        let params = signature.params().map(|(_, ty)| ty).collect::<Vec<_>>();
        let Type::Stream(stream) = &params[0] else {
            panic!("expected stream")
        };
        resolved
            .stream_payloads
            .register::<(u32, String)>(stream.ty().unwrap())?;
        let funcs = [ExternalCallFunc {
            name: "f",
            param_tys: &params,
            result_tys: &[],
        }];
        assert!(
            resolved
                .external_export_invocations(&id, API, &funcs)
                .await?
                .contains_key("f")
        );
        assert!(
            resolved
                .service_export_invocations(&claim, API, &funcs)?
                .contains_key("f")
        );
        Ok(())
    }
}
