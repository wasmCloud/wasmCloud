//! Writing a generated project to disk: stubbed but completely wired.
//! `.wash/config.yaml` is built as a [`Config`] value; note the inversion that
//! `WorkloadConfig::default()` denies all hosts while omitted YAML allows all.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use wash_topology::{Edge, EdgeKind, FactsSource, Node, SCHEMA_VERSION, Topology};

use wash_runtime::host::allowed_hosts::AllowedHost;

use crate::config::{
    ComponentSourceConfig, Config, DevComponent, DevConfig, WorkloadConfig, save_config,
};

use super::spec::{Capability, Spec, Trigger};
use super::stubs;

/// What was written, for reporting back.
#[derive(Debug)]
pub struct Generated {
    pub root: PathBuf,
    pub components: Vec<String>,
    pub topology: Topology,
}

/// Generate `spec` into `parent/<name>`; refuses to write into an existing directory.
pub async fn generate(spec: &Spec, parent: &Path) -> Result<Generated> {
    spec.validate()?;
    let root = parent.join(&spec.name);
    if root.exists() {
        bail!("output directory already exists: {}", root.display());
    }

    let nodes = spec.plan();

    tokio::fs::create_dir_all(root.join("wit"))
        .await
        .with_context(|| format!("failed to create {}", root.join("wit").display()))?;

    write(&root.join("wit/world.wit"), stubs::world_wit(spec)).await?;
    // p3 messaging WIT is not yet published, so the project vendors it.
    if spec.edition.is_p3() && (spec.trigger == Trigger::Messaging || spec.links_over_messaging()) {
        stubs::vendor_wit(
            &root,
            "wasmcloud-messaging-0.3.0",
            "wasmcloud:messaging",
            &[("package.wit", stubs::messaging_wit_0_3())],
        )
        .await?;
    }
    write(&root.join("Cargo.toml"), stubs::workspace_cargo_toml(spec)).await?;
    write(&root.join(".gitignore"), stubs::gitignore()).await?;
    write(&root.join("README.md"), stubs::readme(spec)).await?;

    for node in &nodes {
        let dir = root.join(&node.id);
        tokio::fs::create_dir_all(dir.join("src"))
            .await
            .with_context(|| format!("failed to create {}", dir.display()))?;

        let mut source = stubs::component_source(spec, node);
        for capability in &node.capabilities {
            source.push_str(&stubs::capability_helper(capability, spec.edition));
        }
        write(
            &dir.join("Cargo.toml"),
            stubs::component_cargo_toml(spec, node),
        )
        .await?;

        write(&dir.join("src/lib.rs"), source).await?;

        if node
            .capabilities
            .iter()
            .any(|c| matches!(c, Capability::Grpc { .. }))
        {
            tokio::fs::create_dir_all(dir.join("proto"))
                .await
                .with_context(|| format!("failed to create {}", dir.join("proto").display()))?;
            write(&dir.join("proto/wizard.proto"), stubs::grpc_proto()).await?;
            write(&dir.join("build.rs"), stubs::grpc_build_rs()).await?;
        }
    }

    let config = build_config(spec, &nodes)?;
    // A generated config failing wash's own validation is a generator bug; surface it here.
    config
        .validate(&root)
        .await
        .context("generated config failed validation")?;
    save_config(&config, &root.join(".wash").join("config.yaml"))
        .await
        .context("failed to write .wash/config.yaml")?;

    // No metadata files beside the config: shape is derived from source, so a
    // stored copy could only go stale.
    let topology = build_topology(spec, &nodes);

    Ok(Generated {
        root,
        components: nodes.iter().map(|n| n.id.clone()).collect(),
        topology,
    })
}

async fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    tokio::fs::write(path, contents)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Build `.wash/config.yaml`: first planned node is the build target, the rest dev components.
fn build_config(spec: &Spec, nodes: &[super::spec::PlannedNode]) -> Result<Config> {
    // A capability selected but placed nowhere must not seed config keys or allowlist entries.
    let placed = spec.placed_capabilities();

    let mut config = Config {
        build: Some(crate::config::BuildConfig {
            command: Some("cargo build --workspace --target wasm32-wasip2 --release".into()),
            component_path: nodes.first().map(|n| PathBuf::from(n.wasm_path())),
            ..Default::default()
        }),
        ..Default::default()
    };

    let mut dev = DevConfig {
        service: spec.trigger == Trigger::Service,
        // Host-side tracing needs no import, only this flag.
        wasi_otel: placed.contains(&Capability::Otel),
        // The host refuses webgpu on Windows and s390x at config load.
        wasi_webgpu: placed.contains(&Capability::Webgpu),
        // Development-only inline secret values; production delivers them at bind time.
        host_interfaces: if placed.contains(&Capability::Secrets) {
            // Derived from the same matrix entry as the world import, so version bumps stay in sync.
            let imports = Capability::Secrets.wit_imports(spec.edition);
            let (namespace, rest) = imports[0]
                .split_once(':')
                .unwrap_or(("wasmcloud", imports[0]));
            let (package, _) = rest.split_once('/').unwrap_or(("secrets", rest));
            let version = imports[0].split_once('@').map(|(_, v)| v);
            vec![wash_runtime::wit::WitInterface {
                namespace: namespace.into(),
                package: package.into(),
                interfaces: imports
                    .iter()
                    .filter_map(|import| import.split_once('/'))
                    .map(|(_, iface)| {
                        iface
                            .split_once('@')
                            .map_or(iface, |(name, _)| name)
                            .to_string()
                    })
                    .collect(),
                version: version.and_then(|v| v.parse().ok()),
                name: None,
                config: std::collections::HashMap::from([(
                    "api-key".to_string(),
                    "dev-only-secret".to_string(),
                )]),
            }]
        } else {
            Vec::new()
        },
        components: nodes
            .iter()
            .skip(1)
            .map(|node| {
                let mut component = DevComponent::from_source(
                    node.id.clone(),
                    ComponentSourceConfig::file(node.wasm_path()),
                );
                // Subject routing is pure config; nothing in the WIT says which worker gets what.
                if !node.subscriptions.is_empty() {
                    component
                        .config
                        .insert("subscriptions".to_string(), node.subscriptions.join(","));
                }
                component
            })
            .collect(),
        ..Default::default()
    };
    if placed.contains(&Capability::Postgres) {
        // Placeholder URL: the host connects eagerly, so a dead database takes `wash dev` down.
        dev.postgres_url = Some("postgres://user:password@localhost:5432/postgres".into());
    }
    config.dev = Some(dev);

    // Must never degrade to `AllowedHost::Any` — that would turn a typo into allow-everything.
    let mut allowed_hosts: Vec<AllowedHost> = Vec::new();
    for capability in &placed {
        if let Capability::HttpEgress { host } | Capability::Grpc { host } = capability {
            let parsed = host
                .parse::<AllowedHost>()
                .with_context(|| format!("'{host}' failed to parse after validation"))?;
            allowed_hosts.push(parsed);
        }
    }

    // Seed values the capability stubs read back.
    let mut workload_config = std::collections::HashMap::new();
    if placed.contains(&Capability::Config) {
        workload_config.insert(
            "wizard.greeting".to_string(),
            "hello from wasi:config".to_string(),
        );
    }

    // Always emitted: `allowedHosts` has a three-way default (omitted → allow-all,
    // `[]` → deny-all), so every writer states the policy.
    config.workload = Some(WorkloadConfig {
        allowed_hosts,
        config: workload_config,
        ..Default::default()
    });

    Ok(config)
}

/// The manifest the picker would derive from this project once built.
pub(super) fn build_topology(spec: &Spec, nodes: &[super::spec::PlannedNode]) -> Topology {
    let qualified = |iface: &str| format!("{}/{iface}@0.1.0", spec.name);

    let manifest_nodes = nodes
        .iter()
        .map(|node| {
            let mut imports: Vec<String> = node.imports.iter().map(|i| qualified(i)).collect();
            imports.extend(
                node.capabilities
                    .iter()
                    .filter_map(|c| c.manifest_interface(spec.edition))
                    .map(str::to_string),
            );
            let mut exports: Vec<String> = node.exports.iter().map(|e| qualified(e)).collect();
            if node.role == wash_topology::Role::Ingress {
                exports.push(spec.edition.http_ingress_export().into());
            }
            if node.role == wash_topology::Role::Service {
                exports.push(spec.edition.cli_run_export().into());
            }
            if node.role == wash_topology::Role::Worker {
                exports.push(spec.edition.messaging_handler().into());
            }
            if spec.links_over_messaging() && node.is_trigger {
                imports.push(spec.edition.messaging_consumer().into());
            }
            Node {
                id: node.id.clone(),
                role: node.role,
                world: Some(node.world.clone()),
                world_match: None,
                // Declared, not decoded: nothing is built yet.
                facts_from: FactsSource::Wit,
                file: Some(node.wasm_path()),
                subscriptions: node.subscriptions.clone(),
                imports,
                exports,
            }
        })
        .collect();

    let mut edges: Vec<Edge> = nodes
        .iter()
        .flat_map(|node| {
            node.imports.iter().map(move |import| Edge {
                from: node.id.clone(),
                to: import.clone(),
                via: qualified(import),
                kind: EdgeKind::Direct,
                subject: None,
            })
        })
        .collect();

    // Messaging edges are host-mediated; the subject ties publisher to subscriber.
    if spec.links_over_messaging() {
        let ingress = nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        for node in nodes
            .iter()
            .filter(|n| n.role == wash_topology::Role::Worker)
        {
            for subject in &node.subscriptions {
                edges.push(Edge {
                    from: ingress.clone(),
                    to: node.id.clone(),
                    // Edges must speak the same revision the nodes export.
                    via: spec.edition.messaging_handler().into(),
                    kind: EdgeKind::Messaging,
                    subject: Some(subject.clone()),
                });
            }
        }
    }

    Topology {
        schema: SCHEMA_VERSION,
        id: spec.name.clone(),
        source: spec.name.clone(),
        // Written into the user's own tree; no upstream to clone from.
        repo: None,
        subfolder: None,
        title: Some(format!(
            "{} ({} trigger, {} linking)",
            spec.name,
            spec.trigger.label(),
            spec.linking().label()
        )),
        shape: spec.shape(),
        capabilities: spec
            .placed_capabilities()
            .iter()
            .filter_map(|c| c.manifest_interface(spec.edition))
            .map(str::to_string)
            .collect(),
        nodes: manifest_nodes,
        edges,
        unresolved: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::spec::{Edition, Linking};

    fn spec(trigger: Trigger, linking: Linking, count: usize) -> Spec {
        let (branches, over_messaging) = linking.expand(count);
        Spec {
            name: "demo".into(),
            trigger,
            edition: Edition::P2,
            branches,
            over_messaging,
            capabilities: Vec::new(),
            placement: std::collections::BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn messaging_workers_get_their_subject_from_config() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let generated = generate(&spec(Trigger::Http, Linking::Messaging, 2), tempdir.path())
            .await
            .expect("generate");

        let raw = tokio::fs::read_to_string(generated.root.join(".wash/config.yaml"))
            .await
            .expect("read config");
        assert!(raw.contains("subscriptions"), "{raw}");
        assert!(raw.contains("tasks.worker1"), "{raw}");
        assert!(raw.contains("tasks.worker2"), "{raw}");

        // Host-mediated, so the edges must be messaging rather than direct.
        assert_eq!(generated.topology.edges.len(), 2);
        assert!(
            generated
                .topology
                .edges
                .iter()
                .all(|e| e.kind == wash_topology::EdgeKind::Messaging),
        );
    }

    #[tokio::test]
    async fn generates_a_crate_per_component_plus_one_shared_wit() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let generated = generate(&spec(Trigger::Http, Linking::FanOut, 2), tempdir.path())
            .await
            .expect("generate");

        assert_eq!(generated.components, ["ingress", "branch1", "branch2"]);
        for component in &generated.components {
            assert!(
                generated.root.join(component).join("src/lib.rs").is_file(),
                "missing source for {component}"
            );
        }
        assert!(generated.root.join("wit/world.wit").is_file());
        // The config is the only thing under `.wash/`.
        assert!(generated.root.join(".wash/config.yaml").is_file());
        assert!(!generated.root.join(".wash/recipe.yaml").exists());
        assert!(!generated.root.join(".wash/topology.yaml").exists());
    }

    #[tokio::test]
    async fn the_config_names_the_ingress_as_the_build_target() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let generated = generate(&spec(Trigger::Http, Linking::Chain, 1), tempdir.path())
            .await
            .expect("generate");

        let raw = tokio::fs::read_to_string(generated.root.join(".wash/config.yaml"))
            .await
            .expect("read config");
        assert!(raw.contains("release/ingress.wasm"), "{raw}");
        // The second component is a dev component, not the build target.
        assert!(raw.contains("step1"), "{raw}");
    }

    #[tokio::test]
    async fn a_generated_project_recovers_into_the_same_spec() {
        // No stored recipe: `--from` derives the project and recovers the answers.
        let tempdir = tempfile::tempdir().expect("tempdir");
        let original = spec(Trigger::Http, Linking::Chain, 3);
        let generated = generate(&original, tempdir.path()).await.expect("generate");

        let recovered = crate::wizard::reverse::spec_from_project(&generated.root)
            .await
            .expect("recover a spec from the generated project");

        assert_eq!(recovered.name, original.name);
        assert_eq!(recovered.trigger, original.trigger);
        assert_eq!(recovered.branches, original.branches);
        assert_eq!(recovered.over_messaging, original.over_messaging);
    }

    #[tokio::test]
    async fn refuses_to_overwrite_an_existing_directory() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        tokio::fs::create_dir(tempdir.path().join("demo"))
            .await
            .expect("mkdir");

        let err = generate(&spec(Trigger::Http, Linking::None, 1), tempdir.path())
            .await
            .expect_err("must not scaffold over existing work");
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[tokio::test]
    async fn egress_allowlists_exactly_the_requested_host() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.capabilities = vec![Capability::HttpEgress {
            host: "httpbin.org".into(),
        }];
        let generated = generate(&s, tempdir.path()).await.expect("generate");

        let raw = tokio::fs::read_to_string(generated.root.join(".wash/config.yaml"))
            .await
            .expect("read config");
        assert!(raw.contains("httpbin.org"), "{raw}");
    }

    #[tokio::test]
    async fn the_emitted_manifest_describes_the_requested_shape() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let generated = generate(&spec(Trigger::Http, Linking::FanOut, 3), tempdir.path())
            .await
            .expect("generate");

        assert_eq!(generated.topology.shape, wash_topology::Shape::FanOut);
        assert_eq!(generated.topology.edges.len(), 3, "one edge per branch");
        // Every edge carries a distinct interface, or the host would drop them.
        let vias: std::collections::BTreeSet<_> =
            generated.topology.edges.iter().map(|e| &e.via).collect();
        assert_eq!(vias.len(), 3);
    }
}
