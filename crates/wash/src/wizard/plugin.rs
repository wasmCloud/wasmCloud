//! Generating a **host component plugin**: a component that exports a bespoke
//! capability, which other components import as if the host provided it. Two
//! crates come out, wired together, so `wash dev` can serve through the plugin.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::config::{ComponentSourceConfig, Config, DevConfig, HostPluginConfig};

use super::spec::Edition;
use super::stubs::{WIT_BINDGEN, gitignore};

/// Crate (and directory) name for the capability provider.
const PLUGIN: &str = "plugin";
/// Crate (and directory) name for the component that imports it.
const CONSUMER: &str = "consumer";

/// A capability to generate, addressed the way WIT addresses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSpec {
    /// Project directory name.
    pub name: String,
    /// WIT namespace, e.g. `acme`.
    pub namespace: String,
    /// WIT package, e.g. `cache`.
    pub package: String,
    /// Interface within that package, e.g. `store`.
    pub interface: String,
    /// Partition state per calling workload via `wasmcloud:host/identity`.
    pub with_identity: bool,
    /// Observe workloads binding/unbinding via `wasmcloud:host/workload-lifecycle`.
    pub with_lifecycle: bool,
}

impl PluginSpec {
    /// Parse `namespace:package/interface`, the spelling WIT itself uses.
    pub fn parse(target: &str, name: Option<&str>) -> Result<Self> {
        let (package_path, interface) = target
            .split_once('/')
            .with_context(|| format!("'{target}' has no interface; expected ns:pkg/iface"))?;
        let (namespace, package) = package_path
            .split_once(':')
            .with_context(|| format!("'{target}' has no namespace; expected ns:pkg/iface"))?;

        for (what, value) in [
            ("namespace", namespace),
            ("package", package),
            ("interface", interface),
        ] {
            if value.is_empty() {
                bail!("'{target}' has an empty {what}; expected ns:pkg/iface");
            }
            // WIT identifiers are kebab-case; a wrong one fails deep inside wit-bindgen.
            if !value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                bail!("'{value}' is not a WIT {what}: use lowercase, digits and '-'");
            }
        }

        Ok(Self {
            name: name.unwrap_or(package).to_string(),
            namespace: namespace.to_string(),
            package: package.to_string(),
            interface: interface.to_string(),
            with_identity: false,
            with_lifecycle: false,
        })
    }

    /// How the interface is addressed in WIT, versioned.
    fn qualified(&self) -> String {
        format!(
            "{}:{}/{}@0.1.0",
            self.namespace, self.package, self.interface
        )
    }

    /// Rust path to the generated bindings for this interface.
    fn rust_path(&self) -> String {
        format!(
            "{}::{}::{}",
            super::spec::rust_ident(&self.namespace),
            super::spec::rust_ident(&self.package),
            self.interface_ident()
        )
    }

    /// The name a `use` of [`rust_path`](Self::rust_path) binds, so what call sites spell.
    fn interface_ident(&self) -> String {
        super::spec::rust_ident(&self.interface)
    }
}

/// What was written, for the caller to report.
pub struct Generated {
    pub root: PathBuf,
}

/// Write the plugin, its consumer, and the wiring between them.
pub async fn generate(spec: &PluginSpec, parent: &Path) -> Result<Generated> {
    let root = parent.join(&spec.name);
    if root.exists() {
        bail!("output directory already exists: {}", root.display());
    }

    tokio::fs::create_dir_all(root.join("wit"))
        .await
        .with_context(|| format!("failed to create {}", root.join("wit").display()))?;
    write(&root.join("wit/world.wit"), world_wit(spec)).await?;
    // Out-of-crate `include_str!` blocks `cargo publish`, but `wash` already cannot publish (path-dep on `wash-runtime`).
    if spec.with_identity || spec.with_lifecycle {
        super::stubs::vendor_wit(
            &root,
            "wasmcloud-host-0.1.3",
            "wasmcloud:host",
            &[
                (
                    "types.wit",
                    include_str!("../../../../wit/host/wit/types.wit"),
                ),
                (
                    "identity.wit",
                    include_str!("../../../../wit/host/wit/identity.wit"),
                ),
                (
                    "lifecycle.wit",
                    include_str!("../../../../wit/host/wit/lifecycle.wit"),
                ),
                (
                    "cancel.wit",
                    include_str!("../../../../wit/host/wit/cancel.wit"),
                ),
                (
                    "workload.wit",
                    include_str!("../../../../wit/host/wit/workload.wit"),
                ),
            ],
        )
        .await?;
    }
    write(&root.join("Cargo.toml"), workspace_cargo_toml()).await?;
    write(&root.join(".gitignore"), gitignore()).await?;
    write(&root.join("README.md"), readme(spec)).await?;

    for (dir, source) in [
        (PLUGIN, plugin_source(spec)),
        (CONSUMER, consumer_source(spec)),
    ] {
        let crate_dir = root.join(dir);
        tokio::fs::create_dir_all(crate_dir.join("src"))
            .await
            .with_context(|| format!("failed to create {}", crate_dir.display()))?;
        write(&crate_dir.join("Cargo.toml"), component_cargo_toml(dir)).await?;
        write(&crate_dir.join("src/lib.rs"), source).await?;
    }

    let config = build_config(spec);
    // Failing wash's own config validation is a generator bug; surface it here.
    config
        .validate(&root)
        .await
        .context("generated config failed validation")?;
    tokio::fs::create_dir_all(root.join(".wash"))
        .await
        .with_context(|| format!("failed to create {}", root.join(".wash").display()))?;
    // The standard writer, so every `.wash/config.yaml` comes from one serialiser.
    crate::config::save_config(&config, &root.join(".wash").join("config.yaml")).await?;

    Ok(Generated { root })
}

async fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    tokio::fs::write(path, contents)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

/// The interface, plus a world for each side of it. Deliberately handle-free —
/// a `resource` or `stream` crosses the plugin's store boundary by a slower path.
fn world_wit(spec: &PluginSpec) -> String {
    let PluginSpec {
        namespace,
        package,
        interface,
        ..
    } = spec;
    let mut host_lines = String::new();
    if spec.with_identity {
        host_lines
            .push_str("    // Who is calling, resolved by the host per in-flight invocation.\n");
        host_lines.push_str("    import wasmcloud:host/identity@0.1.3;\n");
    }
    if spec.with_lifecycle {
        host_lines.push_str("    // The host calls these as workloads bind and unbind.\n");
        host_lines.push_str("    export wasmcloud:host/workload-lifecycle@0.1.3;\n");
    }
    format!(
        "package {namespace}:{package}@0.1.0;\n\n\
         /// TODO: the capability you are providing. Every operation is an\n\
         /// `async func` over plain values, which is what lets the host satisfy a\n\
         /// workload's import of it from a component running in its own store.\n\
         ///\n\
         /// `string` keeps the generated bodies readable; a real store probably\n\
         /// wants `list<u8>`.\n\
         interface {interface} {{\n    \
         set: async func(key: string, value: string);\n    \
         get: async func(key: string) -> option<string>;\n\
         }}\n\n\
         /// The provider. Loaded by the host from `dev.host_plugins`, not\n\
         /// deployed as part of any workload.\n\
         world {PLUGIN} {{\n\
         {host_lines}    \
         export {interface};\n\
         }}\n\n\
         /// An ordinary workload that happens to import it. The host resolves\n\
         /// this the same way it resolves `wasi:keyvalue`.\n\
         world {CONSUMER} {{\n    \
         import {interface};\n    \
         import {http_types};\n    \
         export {http_handler};\n\
         }}\n",
        http_types = Edition::P3.http_types_import(),
        http_handler = Edition::P3.http_ingress_export()
    )
}

fn workspace_cargo_toml() -> String {
    format!(
        r#"[workspace]
resolver = "3"
members = ["{PLUGIN}", "{CONSUMER}"]
package.edition = "2024"

[workspace.dependencies]
wit-bindgen = "{WIT_BINDGEN}"

{policy}"#,
        policy = super::stubs::WORKSPACE_POLICY,
    )
}

fn component_cargo_toml(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition.workspace = true

[lints]
workspace = true

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = {{ workspace = true, features = ["async-spawn", "inter-task-wakeup"] }}
"#
    )
}

/// The provider: exports the interface and owns the state behind it.
fn plugin_source(spec: &PluginSpec) -> String {
    let qualified = spec.qualified();
    let path = spec.rust_path();
    let interface = &spec.interface;
    // Identity keys state per calling workload; the key prefix is the whole mechanism.
    let (key_expr, identity_note) = if spec.with_identity {
        (
            "format!(\"{}:{key}\", bindings::wasmcloud::host::identity::get_workload_id())",
            "    // Partitioned per calling workload via the host identity import:\n    \
             // two workloads see two stores.\n",
        )
    } else {
        ("key", "")
    };
    let lifecycle_impl = if spec.with_lifecycle {
        "\nimpl bindings::exports::wasmcloud::host::workload_lifecycle::Guest for Component {\n    \
         /// The host delivers each workload as it binds. TODO: capture the\n    \
         /// per-workload config from `workload.interfaces` if this plugin\n    \
         /// needs it eagerly. Must stay idempotent — replayed after a restart.\n    \
         async fn on_workload_bind(\n        \
         workload: bindings::exports::wasmcloud::host::workload_lifecycle::WorkloadInfo,\n    \
         ) -> Result<(), String> {\n        \
         eprintln!(\"plugin: workload bound: {} ({})\", workload.name, workload.id);\n        \
         Ok(())\n    \
         }\n\n    \
         async fn on_workload_unbind(id: String) {\n        \
         eprintln!(\"plugin: workload unbound: {id}\");\n    \
         }\n\
         }\n"
    } else {
        ""
    };
    format!(
        "//! Generated by `wash wizard`. A host component plugin: the host loads\n\
         //! this once and serves every workload that imports `{qualified}`.\n\n\
         mod bindings {{\n    \
         wit_bindgen::generate!({{\n        \
         path: \"../wit\",\n        \
         world: \"{PLUGIN}\",\n        \
         generate_all,\n        \
         async: [\"export:{qualified}#set\", \"export:{qualified}#get\"],\n    \
         }});\n\
         }}\n\n\
         use std::collections::HashMap;\n\
         use std::sync::{{LazyLock, Mutex}};\n\n\
         use bindings::exports::{path}::Guest as {trait_name};\n\n\
         /// Lives as long as the plugin instance, which the host keeps up across\n\
         /// every call from every workload — that persistence is the whole\n\
         /// reason to be a plugin rather than a library.\n\
         static STATE: LazyLock<Mutex<HashMap<String, String>>> =\n    \
         LazyLock::new(|| Mutex::new(HashMap::new()));\n\n\
         struct Component;\n\n\
         impl {trait_name} for Component {{\n    \
         /// TODO: this is your capability. Everything else is wiring.\n    \
         async fn set(key: String, value: String) {{\n\
         {identity_note}        \
         let key = {key_expr};\n        \
         if let Ok(mut state) = STATE.lock() {{\n            \
         state.insert(key.to_string(), value);\n        \
         }}\n    \
         }}\n\n    \
         async fn get(key: String) -> Option<String> {{\n\
         {identity_note}        \
         let key = {key_expr};\n        \
         // A poisoned lock answers `none` rather than taking the host down\n        \
         // with it; a plugin serves every workload on the box.\n        \
         STATE.lock().ok().and_then(|state| state.get(key.as_str()).cloned())\n    \
         }}\n\
         }}\n\
         {lifecycle_impl}{epilogue}",
        epilogue = super::stubs::export_epilogue(),
        trait_name = capitalize(interface),
    )
}

/// The consumer: an ordinary p3 HTTP handler that imports the capability.
fn consumer_source(spec: &PluginSpec) -> String {
    let path = spec.rust_path();
    // `use bindings::a::b::store` binds `store`, which the call sites below must say.
    let iface = spec.interface_ident();
    format!(
        "//! Generated by `wash wizard`. An ordinary workload — it imports the\n\
         //! capability and never learns that a component is providing it.\n\n\
         mod bindings {{\n    \
         wit_bindgen::generate!({{\n        \
         path: \"../wit\",\n        \
         world: \"{CONSUMER}\",\n        \
         generate_all,\n        \
         async: [\"export:{http_handler}#handle\"],\n    \
         }});\n\
         }}\n\n\
         use bindings::exports::wasi::http::handler::Guest as Handler;\n\
         use bindings::wasi::http::types::{{ErrorCode, Fields, Request, Response}};\n\
         use bindings::{path};\n\n\
         struct Component;\n\n\
         impl Handler for Component {{\n    \
         /// TODO: handle the request. The capability call below proves the\n    \
         /// plugin is wired; replace it with whatever you are building.\n    \
         async fn handle(_request: Request) -> Result<Response, ErrorCode> {{\n        \
         {iface}::set(\"greeting\".to_string(), \"hello from a plugin\".to_string()).await;\n        \
         let text = {iface}::get(\"greeting\".to_string())\n            \
         .await\n            \
         .unwrap_or_else(|| \"nothing stored\".to_string());\n\n\
         {response}    \
         }}\n\
         }}\n{epilogue}",
        http_handler = Edition::P3.http_ingress_export(),
        response = super::stubs::streamed_response("format!(\"{text}\\n\").into_bytes()"),
        epilogue = super::stubs::export_epilogue(),
    )
}

/// The consumer is the build target; the plugin is host infrastructure in `dev.host_plugins`.
fn build_config(spec: &PluginSpec) -> Config {
    Config {
        build: Some(crate::config::BuildConfig {
            command: Some("cargo build --workspace --target wasm32-wasip2 --release".into()),
            component_path: Some(PathBuf::from(wasm_path(CONSUMER))),
            ..Default::default()
        }),
        dev: Some(DevConfig {
            host_plugins: vec![HostPluginConfig {
                id: spec.name.clone(),
                source: ComponentSourceConfig::file(wasm_path(PLUGIN)),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn wasm_path(crate_name: &str) -> String {
    // Mirrors `PlannedNode::wasm_path` (cdylib → underscored stem) so the formats cannot drift.
    format!(
        "target/wasm32-wasip2/release/{}.wasm",
        crate_name.replace('-', "_")
    )
}

fn capitalize(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn readme(spec: &PluginSpec) -> String {
    let qualified = spec.qualified();
    let name = &spec.name;
    format!(
        "# {name}\n\n\
         A host component plugin providing `{qualified}`, and a workload that\n\
         imports it.\n\n\
         ```\n\
         {PLUGIN}/     exports {qualified}\n\
         {CONSUMER}/   imports it, serves HTTP\n\
         ```\n\n\
         ## Run it\n\n\
         ```console\n\
         wash build\n\
         wash dev\n\
         curl localhost:8000/\n\
         ```\n\n\
         The response comes back through the plugin, so a reply means the whole\n\
         path is wired: the host loaded the plugin, matched the consumer's import\n\
         against its export, and routed the call across the store boundary.\n\n\
         ## Requires a wash built with `host-component-plugins`\n\n\
         Component host plugins are opt-in today. A `wash` without that feature\n\
         builds this project fine and then ignores `dev.host_plugins`, so the\n\
         consumer's import resolves to nothing:\n\n\
         ```console\n\
         cargo install --path crates/wash --features host-component-plugins\n\
         ```\n\n\
         ## What to change\n\n\
         The interface in `wit/world.wit` is a placeholder — `set` and `get` over\n\
         strings. Replace it with the capability you actually want, and the two\n\
         `TODO`s with its implementation and a caller.\n\n\
         Two things worth reaching for once it does something real:\n\n\
         - `wasmcloud:host/workload-lifecycle` — the host tells the plugin as each\n\
           workload binds and unbinds, so it can pick up per-workload config\n\
           eagerly instead of on first call.\n\
         - `wasmcloud:host/identity` — who is calling, so state can be partitioned\n\
           per workload rather than shared by everyone on the host.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PluginSpec {
        PluginSpec::parse("acme:cache/store", None).expect("parses")
    }

    #[test]
    fn an_interface_is_addressed_the_way_wit_addresses_it() {
        let spec = spec();
        assert_eq!(spec.namespace, "acme");
        assert_eq!(spec.package, "cache");
        assert_eq!(spec.interface, "store");
        // The package names the project unless the caller says otherwise.
        assert_eq!(spec.name, "cache");

        let named = PluginSpec::parse("acme:cache/store", Some("my-cache")).expect("parses");
        assert_eq!(named.name, "my-cache");
    }

    #[test]
    fn a_malformed_interface_is_refused_here_rather_than_by_wit_bindgen() {
        for bad in [
            "cache/store",      // no namespace
            "acme:cache",       // no interface
            "acme:/store",      // empty package
            "Acme:cache/store", // not kebab-case
        ] {
            assert!(
                PluginSpec::parse(bad, None).is_err(),
                "'{bad}' should not reach the generator"
            );
        }
    }

    #[test]
    fn the_call_sites_spell_the_name_the_use_actually_binds() {
        // Emitting the full path at the call site is a type error in generated text.
        let source = consumer_source(&spec());
        assert!(
            source.contains("use bindings::acme::cache::store;"),
            "{source}"
        );
        assert!(source.contains("store::set("), "{source}");
        assert!(source.contains("store::get("), "{source}");
        assert!(!source.contains("acme::cache::store::set("), "{source}");
    }

    #[test]
    fn a_kebab_case_interface_becomes_a_rust_identifier() {
        let spec = PluginSpec::parse("acme:my-pkg/key-store", None).expect("parses");
        assert_eq!(spec.rust_path(), "acme::my_pkg::key_store");
        assert_eq!(spec.interface_ident(), "key_store");
        // The WIT keeps its hyphens; only the Rust side is rewritten.
        assert!(
            world_wit(&spec).contains("interface key-store"),
            "{}",
            world_wit(&spec)
        );
        assert!(
            plugin_source(&spec).contains("Guest as KeyStore"),
            "{}",
            plugin_source(&spec)
        );
    }

    #[test]
    fn the_plugin_exports_what_the_consumer_imports() {
        // The host matches a workload's import against a plugin's export by name.
        let wit = world_wit(&spec());
        assert!(wit.contains("world plugin {\n    export store;"), "{wit}");
        assert!(wit.contains("import store;"), "{wit}");
    }

    #[test]
    fn the_consumer_is_the_build_target_and_the_plugin_is_not() {
        // A host plugin belongs in `dev.host_plugins`, never in `dev.components`.
        let config = build_config(&spec());
        let build = config.build.expect("a build section");
        assert_eq!(
            build.component_path.expect("a target"),
            PathBuf::from("target/wasm32-wasip2/release/consumer.wasm")
        );

        let dev = config.dev.expect("a dev section");
        assert_eq!(dev.host_plugins.len(), 1);
        assert_eq!(dev.host_plugins[0].id, "cache");
        assert!(
            dev.components.is_empty(),
            "the plugin is not a workload component"
        );
    }

    #[test]
    fn generated_sources_stay_inside_the_lints_the_workspace_denies() {
        // The generated workspace denies unwrap/expect/panic; emitted bodies must hold to it.
        for source in [plugin_source(&spec()), consumer_source(&spec())] {
            assert!(!source.contains(".unwrap()"), "{source}");
            assert!(!source.contains(".expect("), "{source}");
            assert!(!source.contains("panic!"), "{source}");
        }
    }
}
