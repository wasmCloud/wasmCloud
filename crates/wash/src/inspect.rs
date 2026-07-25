//! Common utilities for inspecting and decoding WIT components

use std::io::Read;

use anyhow::Context;
use serde::Serialize;
use wit_component::{DecodedWasm, OutputToString};
use wit_parser::{Resolve, WorldItem, WorldKey};

/// Decode Wasm from anything that implements `Read` into a [`DecodedWasm`].
pub async fn decode_component(component_bytes: impl Read) -> anyhow::Result<DecodedWasm> {
    wit_component::decode_reader(component_bytes).context("failed to decode component bytes")
}

/// Get the decoded WIT world from a component as a pretty-printed string.
pub async fn get_component_wit(component: DecodedWasm) -> anyhow::Result<String> {
    let resolve = component.resolve();
    let main = component.package();

    let mut printer = wit_component::WitPrinter::new(OutputToString::default());
    printer
        .print(resolve, main, &[])
        .context("failed to print WIT world from a component")?;

    Ok(printer.output.to_string())
}

/// One external resource a component names via `@external-id(..)`, together
/// with the interface it is reached through — the machine-readable form of what
/// the printed WIT shows, so tooling need not parse WIT text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalIdRef {
    /// `import` or `export`.
    pub direction: &'static str,
    /// The world item's name — an implements label such as `users`, or the
    /// interface id itself for a plain (unlabeled) item.
    pub name: String,
    /// The interface contract, e.g. `wasi:keyvalue/store@0.2.0-draft`.
    pub interface: String,
    /// The platform's name for the resource. Opaque and free-form.
    pub external_id: String,
}

/// Collect every instance-level `external-id` a component declares, imports
/// first. Func- and type-level ids are preserved on round-trip but not reported:
/// wasmCloud binds whole instances, so an id on one function has nothing to
/// select.
pub fn component_external_ids(component: &DecodedWasm) -> Vec<ExternalIdRef> {
    let resolve = component.resolve();
    let mut found = Vec::new();

    for (_, world) in resolve.worlds.iter() {
        for (direction, items) in [("import", &world.imports), ("export", &world.exports)] {
            for (key, item) in items.iter() {
                let WorldItem::Interface {
                    id,
                    external_id: Some(external_id),
                    ..
                } = item
                else {
                    continue;
                };
                found.push(ExternalIdRef {
                    direction,
                    name: world_key_name(resolve, key),
                    interface: interface_id(resolve, *id),
                    external_id: external_id.clone(),
                });
            }
        }
    }

    found
}

/// A `hostInterfaces` skeleton for the resources a component names: one stanza
/// per distinct external-id, backend config left to fill in.
///
/// Import-side only. An export-side id claims inbound traffic, which a binding
/// grants deliberately — scaffolding it would pre-approve an opt-in.
pub fn host_interfaces_scaffold(component: &DecodedWasm) -> Vec<serde_json::Value> {
    let mut stanzas: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for external in component_external_ids(component)
        .iter()
        .filter(|r| r.direction == "import")
    {
        if seen.contains(&external.external_id) {
            continue;
        }
        let Some((namespace, rest)) = external.interface.split_once(':') else {
            continue;
        };
        let (package, interface_and_version) = match rest.split_once('/') {
            Some(split) => split,
            None => (rest, ""),
        };
        let (interface, version) = match interface_and_version.split_once('@') {
            Some((interface, version)) => (interface, Some(version)),
            None => (interface_and_version, None),
        };

        let mut stanza = serde_json::json!({
            "namespace": namespace,
            "package": package,
            "interfaces": if interface.is_empty() { vec![] } else { vec![interface] },
            "externalId": external.external_id,
            "config": { "backend": "" },
        });
        if let Some(version) = version
            && let Some(map) = stanza.as_object_mut()
        {
            map.insert("version".into(), serde_json::Value::String(version.into()));
        }
        stanzas.push(stanza);
        seen.push(external.external_id.clone());
    }

    stanzas
}

/// The name a world item is declared under: the label for a `(implements ..)`
/// item, or the interface's own id for a plain one.
fn world_key_name(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(name) => name.clone(),
        WorldKey::Interface(id) => interface_id(resolve, *id),
    }
}

/// Render an interface as `namespace:package/interface@version`, falling back to
/// the bare interface name when the package is unknown.
fn interface_id(resolve: &Resolve, id: wit_parser::InterfaceId) -> String {
    let Some(interface) = resolve.interfaces.get(id) else {
        return String::new();
    };
    let name = interface.name.clone().unwrap_or_default();
    let Some(package) = interface.package.and_then(|p| resolve.packages.get(p)) else {
        return name;
    };
    let mut id = format!("{}:{}/{name}", package.name.namespace, package.name.name);
    if let Some(version) = &package.name.version {
        id.push('@');
        id.push_str(&version.to_string());
    }
    id
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Hand-written WAT rather than a built fixture, so this crate's tests do
    /// not depend on the wash-runtime fixture build having run.
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
      (import "wasi:logging/logging@0.1.0-draft" (instance))
    )
    "#;

    fn decode(wat: &str) -> DecodedWasm {
        let bytes = wat::parse_str(wat).expect("fixture WAT should assemble");
        wit_component::decode(&bytes).expect("component should decode")
    }

    #[test]
    fn external_ids_are_listed_with_their_interface() {
        let ids = component_external_ids(&decode(TWO_RESOURCES));

        let users = ids
            .iter()
            .find(|r| r.external_id == "user-db-prod:region-a")
            .expect("users external-id");
        assert_eq!(users.direction, "import");
        assert_eq!(users.name, "users");
        assert!(
            users.interface.starts_with("wasi:keyvalue/store"),
            "unexpected interface: {}",
            users.interface
        );

        assert!(
            ids.iter()
                .any(|r| r.external_id == "catalog-db-prod:region-a"),
            "both resources should be reported"
        );
        // The import that declares no external-id contributes nothing.
        assert_eq!(ids.len(), 2, "unexpected entries: {ids:?}");
    }

    #[test]
    fn a_component_naming_no_resources_reports_none() {
        let ids = component_external_ids(&decode(
            r#"(component (import "wasi:logging/logging@0.1.0-draft" (instance)))"#,
        ));
        assert!(ids.is_empty());
    }

    #[test]
    fn the_scaffold_names_one_stanza_per_resource_with_the_backend_left_blank() {
        let stanzas = host_interfaces_scaffold(&decode(TWO_RESOURCES));
        assert_eq!(
            stanzas.len(),
            2,
            "one per distinct external-id: {stanzas:?}"
        );

        let users = stanzas
            .iter()
            .find(|s| s["externalId"] == "user-db-prod:region-a")
            .expect("users stanza");
        assert_eq!(users["namespace"], "wasi");
        assert_eq!(users["package"], "keyvalue");
        assert_eq!(users["interfaces"][0], "store");
        assert_eq!(users["version"], "0.2.0-draft");
        assert_eq!(
            users["config"]["backend"], "",
            "the backend is the operator's to supply"
        );
    }

    #[test]
    fn the_scaffold_leaves_out_export_side_claims() {
        // An export external-id is a hostname claim, which a binding grants
        // deliberately; scaffolding it would pre-approve it. The import is only
        // there to give the exported instance something to be.
        let stanzas = host_interfaces_scaffold(&decode(
            r#"
            (component
              (import "wasi:http/types@0.2.2" (external-id "shared-types") (instance $types))
              (export "wasi:http/incoming-handler@0.2.2"
                (external-id "inventory.example.com")
                (instance $types))
            )
            "#,
        ));
        assert!(
            stanzas
                .iter()
                .all(|s| s["externalId"] != "inventory.example.com"),
            "a hostname claim must not be scaffolded: {stanzas:?}"
        );
        assert_eq!(
            stanzas.len(),
            1,
            "only the import is scaffolded: {stanzas:?}"
        );
    }
}
