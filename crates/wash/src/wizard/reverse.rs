//! Recovering a [`Spec`] from an existing project — the reverse of generation.
//! Anything outside the expressible set is refused by name rather than
//! approximated, since a recipe's value is that replaying it reproduces the thing.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use wash_topology::{EdgeKind, Role, Shape, Topology};

use super::spec::{Capability, CapabilityKind, Edition, Linking, Spec, Trigger};

/// Derive a project's shape and recover the answers that would regenerate it.
pub async fn spec_from_project(project: &Path) -> Result<Spec> {
    // Derived fresh, never from a stored snapshot, so the recipe matches disk now.
    let id = project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project.display().to_string());
    let topology: Topology = wash_topology::derive(project, &id).with_context(|| {
        format!(
            "failed to work out the shape of {}. A project is a directory with a \
             `.wash/config.yaml` (or .yml/.json/.toml)",
            project.display()
        )
    })?;

    // The allowlisted egress host lives in config, not the manifest.
    let allowed_host = read_allowed_host(project).await;
    spec_from_topology(&topology, allowed_host.as_deref())
}

/// The first entry of `workload.allowedHosts`, when there is one — via the
/// typed loader, since the config may be any of four filenames/formats.
async fn read_allowed_host(project: &Path) -> Option<String> {
    let path = crate::config::locate_project_config(project);
    let config = crate::config::load_config_from_file(&path).ok()?;
    config
        .workload
        .as_ref()?
        .allowed_hosts
        .first()
        .map(ToString::to_string)
}

/// Recover a [`Spec`] from a derived topology, or explain why it cannot be.
pub fn spec_from_topology(topology: &Topology, allowed_host: Option<&str>) -> Result<Spec> {
    let ingresses: Vec<&str> = topology
        .nodes
        .iter()
        .filter(|n| n.role == Role::Ingress)
        .map(|n| n.id.as_str())
        .collect();

    // Hidden wiring means the manifest does not describe the whole project.
    if let Some(hidden) = topology
        .unresolved
        .iter()
        .find(|u| u.reason == "socket-edge" || u.reason == "no-world")
    {
        bail!(
            "'{}' cannot be expressed: {} ({}). Components wired this way are \
             invisible to the manifest, so a recipe would not reproduce them",
            topology.id,
            hidden.hint,
            hidden.reason
        );
    }

    // A project with a service node is a service regardless of its linking.
    let trigger = if topology.nodes.iter().any(|n| n.role == Role::Service) {
        Trigger::Service
    } else if ingresses.is_empty() && topology.nodes.iter().any(|n| n.role == Role::Worker) {
        Trigger::Messaging
    } else {
        Trigger::Http
    };

    let linking = match topology.shape {
        Shape::Single | Shape::Service => Linking::None,
        Shape::Chain => Linking::Chain,
        Shape::FanOut => {
            if topology
                .edges
                .iter()
                .any(|edge| edge.kind == EdgeKind::Messaging)
            {
                Linking::Messaging
            } else {
                Linking::FanOut
            }
        }
        Shape::Unknown => bail!(
            "'{}' cannot be expressed: its components have no derivable connection, \
             so there is no architecture to reproduce",
            topology.id
        ),
    };

    if linking != Linking::None && ingresses.len() > 1 {
        bail!(
            "'{}' cannot be expressed: it has {} HTTP entry points ({}), and this \
             generates exactly one",
            topology.id,
            ingresses.len(),
            ingresses.join(", ")
        );
    }

    // Per-branch depths, so a lopsided project round-trips instead of flattening.
    let paths = recover_paths(topology, linking);
    let branches: Vec<usize> = paths.iter().map(Vec::len).collect();

    let mut capabilities = Vec::new();
    for interface in &topology.capabilities {
        // Messaging interfaces are already carried by the trigger/linking axes.
        if (linking == Linking::Messaging || trigger == Trigger::Messaging)
            && interface.starts_with("wasmcloud:messaging/")
        {
            continue;
        }
        match capability_from_interface(interface, allowed_host) {
            Some(capability) => {
                if !capabilities.contains(&capability) {
                    capabilities.push(capability);
                }
            }
            None => bail!(
                "'{}' cannot be expressed: it uses `{interface}`, which this does not \
                 generate. Supported: wasi:keyvalue, wasi:blobstore, wasi:config, \
                 wasi:logging, wasmcloud:postgres, wasmcloud:secrets, wasi:webgpu, \
                 wasi:http egress, and their wasmcloud:* p3 revisions",
                topology.id
            ),
        }
    }

    // A `@0.3.0` interface anywhere, or a trigger that demands p3, means p3.
    let edition = if trigger.required_edition().is_some()
        || topology
            .nodes
            .iter()
            .flat_map(|n| n.imports.iter().chain(n.exports.iter()))
            .any(|i| i.contains("@0.3.0"))
    {
        Edition::P3
    } else {
        Edition::P2
    };

    let mut spec = Spec {
        name: topology.id.clone(),
        trigger,
        edition,
        branches,
        over_messaging: linking == Linking::Messaging,
        capabilities,
        placement: std::collections::BTreeMap::new(),
    };

    // Per-component imports, read from the components rather than the workload list.
    let placement: std::collections::BTreeMap<String, Vec<String>> =
        planned_names(&spec, topology, &paths)
            .into_iter()
            .map(|(actual, planned)| {
                let found: Vec<Capability> = topology
                    .nodes
                    .iter()
                    .find(|n| n.id == actual)
                    .map(|node| {
                        node.imports
                            .iter()
                            .filter_map(|i| capability_from_interface(i, allowed_host))
                            .collect()
                    })
                    .unwrap_or_default();
                // Palette order, not import order, so this compares with the default.
                let labels = spec
                    .capabilities
                    .iter()
                    .filter(|capability| found.contains(capability))
                    .map(|capability| capability.label().to_string())
                    .collect();
                (planned, labels)
            })
            .collect();
    // A palette entry no component imports would be refused by `validate`; drop it.
    let imported: std::collections::BTreeSet<&str> =
        placement.values().flatten().map(String::as_str).collect();
    spec.capabilities
        .retain(|capability| !capability.emits_code() || imported.contains(capability.label()));

    if placement != spec.default_placement() {
        spec.placement = placement;
    }

    spec.validate().with_context(|| {
        format!(
            "'{}' recovered an architecture this could not regenerate",
            topology.id
        )
    })?;
    Ok(spec)
}

/// The project's own component ids at each hop of each branch out of the trigger.
fn recover_paths(topology: &Topology, linking: Linking) -> Vec<Vec<String>> {
    if linking == Linking::None {
        return Vec::new();
    }
    let trigger = trigger_id(topology);

    // Distinct roots: a worker on two subjects is still one branch.
    let roots: std::collections::BTreeSet<&str> = topology
        .edges
        .iter()
        .filter(|e| e.from == trigger)
        .map(|e| e.to.as_str())
        .collect();

    roots
        .into_iter()
        .map(|root| {
            let mut path = vec![root.to_string()];
            let mut current = root;
            let mut seen = std::collections::BTreeSet::from([root]);
            while let Some(next) = topology
                .edges
                .iter()
                .find(|e| e.from == current && !seen.contains(e.to.as_str()))
            {
                current = next.to.as_str();
                seen.insert(current);
                path.push(current.to_string());
            }
            path
        })
        .collect()
}

/// Whatever the host drives.
fn trigger_id(topology: &Topology) -> &str {
    topology
        .nodes
        .iter()
        .find(|n| n.role == Role::Ingress || n.role == Role::Service)
        .or_else(|| topology.nodes.first())
        .map(|n| n.id.as_str())
        .unwrap_or_default()
}

/// Pair each project component with the positional name a regenerated one uses.
fn planned_names(spec: &Spec, topology: &Topology, paths: &[Vec<String>]) -> Vec<(String, String)> {
    let mut pairs = vec![(trigger_id(topology).to_string(), "ingress".to_string())];
    for ((planned, _), actual) in spec
        .node_ids()
        .into_iter()
        .skip(1)
        .zip(paths.iter().flatten())
    {
        pairs.push((actual.clone(), planned));
    }
    pairs
}

/// Map a manifest interface back to the capability that emits it, driven by
/// [`Capability::wit_imports`] so a new capability row teaches this reader too.
fn capability_from_interface(interface: &str, allowed_host: Option<&str>) -> Option<Capability> {
    fn package(s: &str) -> &str {
        s.split('/').next().unwrap_or(s)
    }
    fn name(s: &str) -> &str {
        s.split('@').next().unwrap_or(s)
    }

    for kind in CapabilityKind::ALL {
        let capability = Capability::from(kind);
        for edition in Edition::ALL {
            for import in capability.wit_imports(edition) {
                if package(import) == package(interface) {
                    return Some(capability);
                }
            }
        }
    }

    let egress = Capability::HttpEgress {
        host: String::new(),
    };
    for edition in Edition::ALL {
        for import in egress.wit_imports(edition) {
            if name(import) == name(interface) {
                return Some(Capability::HttpEgress {
                    // An omitted `allowedHosts` means allow-all in YAML.
                    host: allowed_host.unwrap_or("*").to_string(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use wash_topology::{Edge, FactsSource, Node, Unresolved};

    fn node(id: &str, role: Role) -> Node {
        Node {
            id: id.into(),
            role,
            world: None,
            world_match: None,
            facts_from: FactsSource::Wasm,
            file: None,
            subscriptions: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
        }
    }

    fn topology(shape: Shape, nodes: Vec<Node>, edges: Vec<Edge>) -> Topology {
        Topology {
            schema: 1,
            id: "demo".into(),
            source: "templates/demo".into(),
            repo: None,
            subfolder: None,
            title: None,
            shape,
            capabilities: Vec::new(),
            nodes,
            edges,
            unresolved: Vec::new(),
        }
    }

    fn direct(from: &str, to: &str) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            via: format!("demo/{to}@0.1.0"),
            kind: EdgeKind::Direct,
            subject: None,
        }
    }

    #[test]
    fn a_chain_recovers_its_hop_count() {
        let t = topology(
            Shape::Chain,
            vec![
                node("ingress", Role::Ingress),
                node("step1", Role::Component),
                node("step2", Role::Component),
            ],
            vec![direct("ingress", "step1"), direct("step1", "step2")],
        );
        let spec = spec_from_topology(&t, None).expect("chain is expressible");
        assert_eq!(spec.linking(), Linking::Chain);
        assert_eq!(spec.branches, vec![2], "one branch, two deep");
    }

    #[test]
    fn a_direct_fan_out_counts_distinct_targets() {
        let t = topology(
            Shape::FanOut,
            vec![
                node("ingress", Role::Ingress),
                node("branch1", Role::Component),
                node("branch2", Role::Component),
            ],
            vec![direct("ingress", "branch1"), direct("ingress", "branch2")],
        );
        let spec = spec_from_topology(&t, None).expect("fan-out is expressible");
        assert_eq!(spec.linking(), Linking::FanOut);
        assert_eq!(spec.branches, vec![1, 1]);
    }

    #[test]
    fn messaging_edges_recover_the_messaging_architecture() {
        let mut t = topology(
            Shape::FanOut,
            vec![
                node("ingress", Role::Ingress),
                node("worker1", Role::Worker),
            ],
            vec![Edge {
                from: "ingress".into(),
                to: "worker1".into(),
                via: "wasmcloud:messaging/handler@0.2.0".into(),
                kind: EdgeKind::Messaging,
                subject: Some("tasks.worker1".into()),
            }],
        );
        t.edges.push(Edge {
            subject: Some("tasks.extra".into()),
            ..t.edges[0].clone()
        });
        let spec = spec_from_topology(&t, None).expect("messaging is expressible");
        assert_eq!(spec.linking(), Linking::Messaging);
        assert_eq!(spec.branches, vec![1], "two subjects, still one branch");
    }

    #[test]
    fn capabilities_come_back_with_the_allowlisted_host() {
        // `topology.capabilities` is the union of the nodes' imports, as derivation emits.
        let mut only = node("a", Role::Ingress);
        only.imports = vec![
            "wasi:keyvalue/store@0.2.0-draft".into(),
            "wasi:http/outgoing-handler@0.2.2".into(),
        ];
        let mut t = topology(Shape::Single, vec![only.clone()], Vec::new());
        t.capabilities = only.imports.clone();

        let spec = spec_from_topology(&t, Some("httpbin.org")).expect("expressible");
        assert!(spec.capabilities.contains(&Capability::Keyvalue));
        assert!(spec.capabilities.contains(&Capability::HttpEgress {
            host: "httpbin.org".into()
        }));
        assert!(spec.placement.is_empty(), "{:?}", spec.placement);
    }

    #[test]
    fn a_workload_capability_no_component_imports_is_dropped_not_refused() {
        // A hand-edited manifest can produce this; refusing would make it unrecoverable.
        let mut t = topology(Shape::Single, vec![node("a", Role::Ingress)], Vec::new());
        t.capabilities = vec!["wasi:keyvalue/store@0.2.0-draft".into()];

        let spec = spec_from_topology(&t, None).expect("recovered, not refused");
        assert!(
            spec.capabilities.is_empty(),
            "nothing imports it, so it is not in play: {:?}",
            spec.capabilities
        );
    }

    #[test]
    fn a_capability_comes_back_on_the_component_that_imports_it() {
        // Placement must follow the importing component, not flatten to the leaves.
        let mut api = node("api", Role::Ingress);
        api.imports = vec!["wasi:logging/logging@0.1.0-draft".into()];
        let mut left = node("left", Role::Component);
        left.imports = vec!["wasi:keyvalue/store@0.2.0-draft".into()];
        let right = node("right", Role::Component);

        let mut t = topology(
            Shape::FanOut,
            vec![api, left, right],
            vec![direct("api", "left"), direct("api", "right")],
        );
        t.capabilities = vec![
            "wasi:keyvalue/store@0.2.0-draft".into(),
            "wasi:logging/logging@0.1.0-draft".into(),
        ];

        let spec = spec_from_topology(&t, None).expect("expressible");
        assert_eq!(spec.branches, vec![1, 1]);
        assert_eq!(
            spec.placement.get("ingress").map(Vec::as_slice),
            Some(&["logging".to_string()][..]),
            "{:?}",
            spec.placement
        );
        assert_eq!(
            spec.placement.get("branch1").map(Vec::as_slice),
            Some(&["keyvalue".to_string()][..])
        );
        assert_eq!(
            spec.placement.get("branch2").map(Vec::as_slice),
            Some(&[][..]),
            "the branch that imports nothing gets nothing"
        );

        let nodes = spec.plan();
        assert!(nodes[0].has(&Capability::Logging));
        assert!(nodes[1].has(&Capability::Keyvalue));
        assert!(nodes[2].capabilities.is_empty());
    }

    #[test]
    fn a_project_whose_capabilities_are_all_at_the_ends_needs_no_placement() {
        // The common case must stay a short recipe.
        let api = node("api", Role::Ingress);
        let mut leaf = node("leaf", Role::Component);
        leaf.imports = vec!["wasi:keyvalue/store@0.2.0-draft".into()];

        let mut t = topology(Shape::Chain, vec![api, leaf], vec![direct("api", "leaf")]);
        t.capabilities = vec!["wasi:keyvalue/store@0.2.0-draft".into()];

        let spec = spec_from_topology(&t, None).expect("expressible");
        assert!(
            spec.placement.is_empty(),
            "this is the default, so it is not worth writing down: {:?}",
            spec.placement
        );
        assert!(spec.plan()[1].has(&Capability::Keyvalue));
    }

    #[test]
    fn a_service_recovers_as_a_service_trigger() {
        let t = topology(Shape::Service, vec![node("a", Role::Service)], Vec::new());
        let spec = spec_from_topology(&t, None).expect("services are generated now");
        assert_eq!(spec.trigger, Trigger::Service);
        assert_eq!(spec.linking(), Linking::None);
    }

    #[test]
    fn a_worker_with_no_ingress_recovers_as_a_messaging_trigger() {
        let t = topology(Shape::Single, vec![node("a", Role::Worker)], Vec::new());
        let spec = spec_from_topology(&t, None).expect("messaging triggers are generated");
        assert_eq!(spec.trigger, Trigger::Messaging);
    }

    #[test]
    fn hidden_socket_wiring_is_refused_by_name() {
        let mut t = topology(Shape::Single, vec![node("a", Role::Ingress)], Vec::new());
        t.unresolved = vec![Unresolved {
            node: None,
            reason: "socket-edge".into(),
            hint: "components are wired by a hardcoded address in Rust".into(),
        }];
        let err = spec_from_topology(&t, None).expect_err("hidden wiring is not expressible");
        assert!(err.to_string().contains("socket-edge"), "{err}");
    }

    #[test]
    fn messaging_interfaces_do_not_refuse_a_messaging_architecture() {
        let mut ingress = node("ingress", Role::Ingress);
        ingress.imports = vec!["wasmcloud:messaging/consumer@0.2.0".into()];
        let mut worker = node("worker1", Role::Worker);
        worker.imports = vec!["wasi:config/store@0.2.0-rc.1".into()];

        let mut t = topology(
            Shape::FanOut,
            vec![ingress, worker],
            vec![Edge {
                from: "ingress".into(),
                to: "worker1".into(),
                via: "wasmcloud:messaging/handler@0.2.0".into(),
                kind: EdgeKind::Messaging,
                subject: Some("tasks.worker1".into()),
            }],
        );
        // The broker interfaces come with the architecture, not as extras.
        t.capabilities = vec![
            "wasmcloud:messaging/consumer@0.2.0".into(),
            "wasmcloud:messaging/types@0.2.0".into(),
            "wasi:config/store@0.2.0-rc.1".into(),
        ];
        let spec = spec_from_topology(&t, None).expect("messaging must be expressible");
        assert_eq!(spec.linking(), Linking::Messaging);
        assert_eq!(spec.capabilities, vec![Capability::Config]);
        assert!(spec.plan()[1].has(&Capability::Config));
        assert!(spec.plan()[0].capabilities.is_empty());
    }

    #[test]
    fn an_unknown_capability_is_refused_rather_than_dropped() {
        // `wasi:tls` is real but feature-gated off; it must refuse, not silently drop.
        let mut t = topology(Shape::Single, vec![node("a", Role::Ingress)], Vec::new());
        let mut n = node("a", Role::Ingress);
        n.imports = vec!["wasi:tls/client@0.3.0-draft".into()];
        t.nodes = vec![n];
        t.capabilities = vec!["wasi:tls/client@0.3.0-draft".into()];
        let err = spec_from_topology(&t, None).expect_err("tls is not generated");
        assert!(err.to_string().contains("wasi:tls"), "{err}");
        assert!(err.to_string().contains("wasi:keyvalue"), "{err}");
    }
}
