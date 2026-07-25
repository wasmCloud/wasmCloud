#![cfg(feature = "wasm_component_model_implements")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! End-to-end proof that the component-model `external-id` attribute survives
//! the trip from a component binary into the runtime's `WitWorld`, and that
//! bindings select on it.
//!
//! The components here are hand-written WAT rather than built fixtures, to reach
//! shapes a Rust guest does not emit: an external-id on a *plain* (unlabeled)
//! interface import, and a component whose imports carry no attribute at all.
//! `keyvalue-external-id` is the built-fixture counterpart, exercised
//! end-to-end in `integration_keyvalue_external_id.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use wash_runtime::engine::{Engine, component_world};
use wash_runtime::plugin::multiplex::{EXTERNAL_ID_CONFIG_KEY, Multiplexer};
use wash_runtime::wit::WitInterface;
use wasmtime::component::Component;

/// Two imports of one interface, told apart only by the platform resource each
/// names, plus a plain import that carries an external-id without a label and
/// an export claiming a hostname.
const TWO_RESOURCES: &str = r#"
(component
  (import "users"
    (implements "wasi:keyvalue/store@0.2.0-draft")
    (external-id "user-db-prod:region-a")
    (instance))
  (import "catalog"
    (implements "wasi:keyvalue/store@0.2.0-draft")
    (external-id "catalog-db-prod:region-a")
    (instance))
  (import "wasi:logging/logging@0.1.0-draft"
    (external-id "central-logs")
    (instance))
  (core module $m)
  (core instance (instantiate $m))
)
"#;

fn compile(engine: &Engine, wat: &str) -> Component {
    let bytes = wat::parse_str(wat).expect("fixture WAT should assemble");
    Component::new(engine.inner(), &bytes).expect("runtime should accept an external-id component")
}

fn engine() -> Engine {
    Engine::builder().build().expect("engine")
}

fn import_named<'a>(imports: &'a [WitInterface], name: &str) -> &'a WitInterface {
    imports
        .iter()
        .find(|i| i.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no import named {name}"))
}

#[test]
fn world_carries_implements_and_external_id_together() {
    let engine = engine();
    let component = compile(&engine, TWO_RESOURCES);
    let world = component_world(&component);

    let imports: Vec<WitInterface> = world.imports.into_iter().collect();

    // The labeled imports keep all three pieces of metadata: the contract from
    // `implements`, the routing name from the import name, and the platform
    // resource from `external-id`.
    let users = import_named(&imports, "users");
    assert_eq!(users.namespace, "wasi");
    assert_eq!(users.package, "keyvalue");
    assert!(users.interfaces.contains("store"));
    assert_eq!(users.external_id.as_deref(), Some("user-db-prod:region-a"));

    let catalog = import_named(&imports, "catalog");
    assert_eq!(
        catalog.external_id.as_deref(),
        Some("catalog-db-prod:region-a")
    );

    // A plain (unlabeled) interface import carries an external-id just as well.
    let logging = imports
        .iter()
        .find(|i| i.package == "logging")
        .expect("logging import");
    assert_eq!(logging.name, None);
    assert_eq!(logging.external_id.as_deref(), Some("central-logs"));
}

#[test]
fn imports_without_the_attribute_are_unchanged() {
    let engine = engine();
    let component = compile(
        &engine,
        r#"
        (component
          (import "wasi:logging/logging@0.1.0-draft" (instance))
          (import "kv" (implements "wasi:keyvalue/store@0.2.0-draft") (instance))
        )
        "#,
    );
    let world = component_world(&component);
    assert!(
        world.imports.iter().all(|i| i.external_id.is_none()),
        "no import declared an external-id"
    );
    let imports: Vec<WitInterface> = world.imports.into_iter().collect();
    assert_eq!(import_named(&imports, "kv").package, "keyvalue");
}

#[test]
fn one_package_may_name_two_resources_through_two_interfaces() {
    // The component model puts no uniqueness requirement on external-id, and
    // nothing says two interfaces of one package must name one resource. These
    // stay separate rather than collapsing into a single binding.
    let engine = engine();
    let component = compile(
        &engine,
        r#"
        (component
          (import "wasi:keyvalue/store@0.2.0-draft"   (external-id "db-a") (instance))
          (import "wasi:keyvalue/atomics@0.2.0-draft" (external-id "db-b") (instance))
        )
        "#,
    );
    let world = component_world(&component);
    assert_eq!(world.imports.len(), 2, "{:?}", world.imports);

    // ... and the same resource named twice is one interface with both.
    let component = compile(
        &engine,
        r#"
        (component
          (import "wasi:keyvalue/store@0.2.0-draft"   (external-id "db") (instance))
          (import "wasi:keyvalue/atomics@0.2.0-draft" (external-id "db") (instance))
        )
        "#,
    );
    let world = component_world(&component);
    assert_eq!(world.imports.len(), 1);
    let merged = world.imports.iter().next().expect("one interface");
    assert!(merged.interfaces.contains("store"));
    assert!(merged.interfaces.contains("atomics"));
}

#[test]
fn two_labels_naming_one_resource_are_two_imports() {
    // Names identify imports (they are strongly unique); external-ids resolve
    // them and carry no uniqueness requirement. So one resource named by two
    // labeled imports must surface as two world imports — each label needs its
    // own linker entry, resolved to the same binding.
    let engine = engine();
    let component = compile(
        &engine,
        r#"
        (component
          (import "users"   (implements "wasi:keyvalue/store@0.2.0-draft") (external-id "db") (instance))
          (import "catalog" (implements "wasi:keyvalue/store@0.2.0-draft") (external-id "db") (instance))
        )
        "#,
    );
    let world = component_world(&component);
    assert_eq!(world.imports.len(), 2, "{:?}", world.imports);
    for label in ["users", "catalog"] {
        assert!(
            world
                .imports
                .iter()
                .any(|i| i.name.as_deref() == Some(label)
                    && i.external_id.as_deref() == Some("db")),
            "missing import {label}: {:?}",
            world.imports
        );
    }
}

#[tokio::test]
async fn one_binding_per_resource_serves_a_component_that_never_names_it() {
    // A recording backend so the test can see which resource each import was
    // bound to.
    struct Recorder(std::sync::Mutex<Vec<HashMap<String, String>>>);
    #[async_trait::async_trait]
    impl wash_runtime::plugin::multiplex::BackendProvider<Arc<String>> for Recorder {
        fn backend_type(&self) -> &'static str {
            "recording"
        }
        async fn instantiate(
            &self,
            config: &HashMap<String, String>,
        ) -> anyhow::Result<Arc<String>> {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(config.clone());
            Ok(Arc::new(
                config
                    .get(EXTERNAL_ID_CONFIG_KEY)
                    .cloned()
                    .unwrap_or_default(),
            ))
        }
    }

    let engine = engine();
    let component = compile(&engine, TWO_RESOURCES);
    let world = component_world(&component);

    let recorder = Arc::new(Recorder(std::sync::Mutex::new(Vec::new())));
    let mux = Multiplexer::new("wasi", "keyvalue", "recording").with_provider(recorder);

    // The platform authors one binding per resource. It never mentions `users`
    // or `catalog` — those are the component's own words for its dependencies.
    let bind = |external_id: &str| WitInterface {
        external_id: Some(external_id.to_string()),
        ..WitInterface::from("wasi:keyvalue/store@0.2.0-draft")
    };
    let entries = [
        bind("user-db-prod:region-a"),
        bind("catalog-db-prod:region-a"),
    ];

    let registry = mux
        .build_registry(world.imports.iter(), entries.iter())
        .await
        .expect("both imports should bind");

    assert_eq!(
        mux.resolve(&registry, "users").unwrap().as_str(),
        "user-db-prod:region-a"
    );
    assert_eq!(
        mux.resolve(&registry, "catalog").unwrap().as_str(),
        "catalog-db-prod:region-a"
    );
}

#[tokio::test]
async fn an_unbound_resource_is_named_in_the_error() {
    let engine = engine();
    let component = compile(&engine, TWO_RESOURCES);
    let world = component_world(&component);

    // A provider that always succeeds, so the only thing that can fail is the
    // binding lookup itself.
    struct Anything;
    #[async_trait::async_trait]
    impl wash_runtime::plugin::multiplex::BackendProvider<Arc<String>> for Anything {
        fn backend_type(&self) -> &'static str {
            "recording"
        }
        async fn instantiate(
            &self,
            _config: &HashMap<String, String>,
        ) -> anyhow::Result<Arc<String>> {
            Ok(Arc::new(String::new()))
        }
    }

    let mux = Multiplexer::new("wasi", "keyvalue", "recording").with_provider(Arc::new(Anything));
    let only_one = [WitInterface {
        external_id: Some("user-db-prod:region-a".to_string()),
        ..WitInterface::from("wasi:keyvalue/store@0.2.0-draft")
    }];

    let err = mux
        .build_registry(world.imports.iter(), only_one.iter())
        .await
        .expect_err("catalog has no binding and there is no default");
    let msg = err.to_string();
    assert!(
        msg.contains("catalog-db-prod:region-a"),
        "the error should name the missing platform resource: {msg}"
    );
}
