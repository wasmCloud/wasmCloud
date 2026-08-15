//! Deriving a [`Topology`] from a project on disk by applying the host's own
//! linking rules statically. Facts come from the built component when present
//! (authoritative), falling back to a textual scan of `wit/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::schema::{
    Edge, EdgeKind, FactsSource, Node, Role, SCHEMA_VERSION, Shape, Topology, Unresolved,
};
use crate::schema::{is_http_handler, is_messaging_consumer, is_messaging_handler};

/// Config filenames a project may use, matching `wash`'s own `VALID_CONFIG_FILES`.
const CONFIG_FILES: [&str; 4] = ["config.yaml", "config.yml", "config.json", "config.toml"];

/// How deep to walk looking for projects. Public because whatever reads the
/// manifests must walk the same tree as whatever writes them.
pub const MAX_DEPTH: usize = 4;

/// Directory names never worth descending into.
const SKIP_DIRS: [&str; 5] = ["target", ".git", "node_modules", "deps", ".wash"];

/// Whether a walk should descend into `name`; public because manifest readers
/// and writers must walk the same tree.
pub fn descends_into(name: &str) -> bool {
    !SKIP_DIRS.contains(&name) && !name.starts_with('.')
}

/// Every project under `root`, by convention: a directory holding a `.wash/config.*`.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, &mut found, 0);
    found.sort();
    found
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(config) = find_config(dir)
        && !is_overlay(&config)
    {
        found.push(dir.to_path_buf());
        // A project may contain nested projects, so keep descending.
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !descends_into(&name) {
            continue;
        }
        walk(&path, found, depth + 1);
    }
}

/// Whether a directory only *configures* a component built somewhere else,
/// judged by its `component_path` escaping the directory.
fn is_overlay(config_path: &Path) -> bool {
    let Ok(config) = read_config(config_path) else {
        return false;
    };
    // Only `build.component_path` decides this; `dev.components` legitimately points at siblings.
    config
        .build
        .and_then(|build| build.component_path)
        .is_some_and(|path| escapes(&path))
}

/// Whether a relative path climbs out of the directory it is written in;
/// counts depth so `a/../../b` is caught too.
fn escapes(path: &Path) -> bool {
    let mut depth = 0i32;
    for part in path.components() {
        match part {
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
            Component::RootDir | Component::Prefix(_) => return true,
            Component::CurDir => {}
        }
    }
    false
}

/// The project's config file, whichever form it takes.
pub fn find_config(project: &Path) -> Option<PathBuf> {
    CONFIG_FILES
        .iter()
        .map(|name| project.join(".wash").join(name))
        .find(|path| path.is_file())
}

/// Parse a config by extension; YAML is a superset of JSON, so one parser covers three of the four.
fn read_config(path: &Path) -> Result<WashConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if path.extension().is_some_and(|e| e == "toml") {
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
    } else {
        serde_yaml_ng::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
    }
}

// `.wash/config.yaml` — only the fields derivation needs.

#[derive(Debug, Default, Deserialize)]
struct WashConfig {
    build: Option<BuildSection>,
    dev: Option<DevSection>,
}

#[derive(Debug, Default, Deserialize)]
struct BuildSection {
    component_path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct DevSection {
    #[serde(default)]
    service: bool,
    service_file: Option<PathBuf>,
    #[serde(default)]
    components: Vec<ConfigComponent>,
}

#[derive(Debug, Deserialize)]
struct ConfigComponent {
    name: String,
    file: Option<PathBuf>,
    #[serde(default)]
    config: BTreeMap<String, String>,
}

/// One world's import/export interface names, however they were obtained.
#[derive(Debug, Clone)]
struct WorldFacts {
    name: String,
    imports: Vec<String>,
    exports: Vec<String>,
    /// Component directory this world was found in, for per-component `wit/` layouts.
    owner: Option<String>,
}

/// Decode a built component's world — the authoritative surface, including
/// exports only a proc macro wrote. Keeps interface items only.
fn facts_from_wasm(path: &Path) -> Result<WorldFacts> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read component {}", path.display()))?;
    let (resolve, world_id) = match wit_component::decode(&bytes)
        .with_context(|| format!("failed to decode component {}", path.display()))?
    {
        wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
        wit_component::DecodedWasm::WitPackage(..) => {
            bail!("{} is a WIT package, not a component", path.display())
        }
    };

    let world = resolve
        .worlds
        .get(world_id)
        .context("decoded component names a world that is not in its own resolve")?;
    Ok(WorldFacts {
        name: world.name.clone(),
        imports: interface_names(&resolve, world.imports.iter()),
        exports: interface_names(&resolve, world.exports.iter()),
        // Bound to its node directly, never matched.
        owner: None,
    })
}

/// Qualified names of a world's interface items, skipping bare function and type items.
fn interface_names<'a>(
    resolve: &wit_parser::Resolve,
    items: impl Iterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
) -> Vec<String> {
    items
        .filter(|(_, item)| matches!(item, wit_parser::WorldItem::Interface { .. }))
        .filter_map(|(key, _)| match key {
            wit_parser::WorldKey::Name(name) => Some(name.clone()),
            wit_parser::WorldKey::Interface(id) => resolve.id_of(*id),
        })
        .collect()
}

/// Scan the project's root and per-component `wit/` directories for worlds.
fn scan_wit(project: &Path) -> Result<Vec<WorldFacts>> {
    let mut worlds = Vec::new();

    // Shared layout: one `wit/` at the root.
    worlds.extend(scan_wit_dir(&project.join("wit"), None)?);

    // Per-component layout: each component carries its own `wit/`.
    let Ok(entries) = fs::read_dir(project) else {
        return Ok(worlds);
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if path.is_dir() && !matches!(name.as_str(), "target" | ".git" | "wit" | ".wash") {
            dirs.push(path);
        }
    }
    dirs.sort();
    for dir in dirs {
        let owner = dir.file_name().map(|n| n.to_string_lossy().to_string());
        worlds.extend(scan_wit_dir(&dir.join("wit"), owner)?);
    }

    worlds.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(worlds)
}

/// Read every world in one `wit/` directory; `wit/deps` (fetched packages) is skipped.
fn scan_wit_dir(wit_dir: &Path, owner: Option<String>) -> Result<Vec<WorldFacts>> {
    if !wit_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut worlds = Vec::new();
    for entry in
        fs::read_dir(wit_dir).with_context(|| format!("failed to read {}", wit_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "wit") {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            worlds.extend(
                parse_worlds(&strip_comments(&text))
                    .into_iter()
                    .map(|mut w| {
                        w.owner.clone_from(&owner);
                        w
                    }),
            );
        }
    }
    Ok(worlds)
}

/// Drop `//` and `/* */` comments so keywords inside prose are not parsed as items.
fn strip_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    // `get` throughout rather than indexing: the workspace denies `indexing_slicing`.
    while let Some(&ch) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        match (ch, next) {
            ('/', Some('/')) => {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while chars
                    .get(i)
                    .is_some_and(|c| *c != '*' || chars.get(i + 1).copied() != Some('/'))
                {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Pull `world <name> { ... }` blocks out of comment-stripped WIT source, matching braces.
fn parse_worlds(text: &str) -> Vec<WorldFacts> {
    let mut worlds = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut search = 0usize;

    while let Some(found) = text[search..].find("world ") {
        let kw = search + found;
        // Require a token boundary so `subworld ` never matches.
        let preceding = kw.checked_sub(1).and_then(|i| chars.get(i)).copied();
        if preceding.is_some_and(|c| c.is_alphanumeric() || c == '-') {
            search = kw + "world ".len();
            continue;
        }
        let after = kw + "world ".len();
        let Some(brace_rel) = text[after..].find('{') else {
            break;
        };
        let open = after + brace_rel;
        let name = text[after..open].trim().to_string();

        let mut depth = 0usize;
        let mut close = open;
        for (idx, ch) in text[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + idx;
                        break;
                    }
                }
                _ => {}
            }
        }

        let body = &text[open + 1..close];
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        for item in body.split(';') {
            let item = item.trim();
            if let Some(rest) = item.strip_prefix("import ") {
                imports.push(rest.trim().to_string());
            } else if let Some(rest) = item.strip_prefix("export ") {
                exports.push(rest.trim().to_string());
            }
        }
        if !name.is_empty() {
            worlds.push(WorldFacts {
                name,
                imports,
                exports,
                owner: None,
            });
        }
        search = close.max(open + 1);
    }
    worlds
}

pub fn derive(project: &Path, rel: &str) -> Result<Topology> {
    let config_path = find_config(project)
        .with_context(|| format!("no .wash/config.* in {}", project.display()))?;
    let config = read_config(&config_path)?;
    let worlds = scan_wit(project)?;

    let id = project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| rel.to_string());

    let mut nodes = Vec::new();
    let mut unresolved = Vec::new();

    // The primary component's id is the wasm stem, normalized to the `-` form config and WIT use.
    let dev = config.dev.unwrap_or_default();
    if let Some(path) = config.build.and_then(|b| b.component_path) {
        nodes.push(new_node(&stem_id(&path), Some(path), Vec::new()));
    }
    for component in &dev.components {
        let subscriptions = component
            .config
            .get("subscriptions")
            .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
            .unwrap_or_default();
        nodes.push(new_node(
            &component.name,
            component.file.clone(),
            subscriptions,
        ));
    }
    if let Some(path) = dev.service_file.clone() {
        let sid = stem_id(&path);
        if let Some(existing) = nodes.iter_mut().find(|n| n.id == sid) {
            existing.role = Role::Service;
        } else {
            let mut node = new_node(&sid, Some(path), Vec::new());
            node.role = Role::Service;
            nodes.push(node);
        }
    } else if dev.service
        && let Some(first) = nodes.first_mut()
    {
        first.role = Role::Service;
    }

    // Prefer the built component; fall back to WIT for whatever is unbuilt.
    for node in &mut nodes {
        let Some(file) = &node.file else { continue };
        let path = project.join(file);
        if !path.is_file() {
            continue;
        }
        match facts_from_wasm(&path) {
            Ok(facts) => {
                node.world = Some(facts.name);
                node.imports = facts.imports;
                node.exports = facts.exports;
                node.facts_from = FactsSource::Wasm;
            }
            Err(err) => unresolved.push(Unresolved {
                node: Some(node.id.clone()),
                reason: "undecodable-component".into(),
                hint: format!("{err:#}"),
            }),
        }
    }
    bind_worlds(&mut nodes, &worlds, &mut unresolved);

    // Roles follow from exports, mirroring how the host picks an ingress.
    for node in &mut nodes {
        if node.role == Role::Service {
            continue;
        }
        if node.exports.iter().any(|e| is_http_handler(e)) {
            node.role = Role::Ingress;
        } else if node.exports.iter().any(|e| is_messaging_handler(e)) {
            node.role = Role::Worker;
        }
    }

    let edges = derive_edges(&nodes, &mut unresolved);
    let capabilities = derive_capabilities(&nodes);
    detect_blind_spots(project, &nodes, &edges, &mut unresolved)?;

    let shape = classify(&nodes, &edges);

    Ok(Topology {
        schema: SCHEMA_VERSION,
        id,
        source: rel.to_string(),
        // Only a linked stub carries an origin, stamped on after derivation.
        repo: None,
        subfolder: None,
        title: read_title(project),
        shape,
        capabilities,
        nodes,
        edges,
        unresolved,
    })
}

fn new_node(id: &str, file: Option<PathBuf>, subscriptions: Vec<String>) -> Node {
    Node {
        id: id.to_string(),
        role: Role::Component,
        world: None,
        world_match: None,
        facts_from: FactsSource::None,
        file: file.map(|f| f.to_string_lossy().replace('\\', "/")),
        subscriptions,
        imports: Vec::new(),
        exports: Vec::new(),
    }
}

/// `target/wasm32-wasip2/release/http_api.wasm` -> `http-api`.
fn stem_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().replace('_', "-"))
        .unwrap_or_default()
}

/// Bind each node to a world: own-directory, exact id, `<id>-component`,
/// `-`-delimited prefix, then a sole world covering a sole node.
fn bind_worlds(nodes: &mut [Node], worlds: &[WorldFacts], unresolved: &mut Vec<Unresolved>) {
    let pending = nodes
        .iter()
        .filter(|n| n.facts_from != FactsSource::Wasm)
        .count();
    if pending == 0 {
        return;
    }
    if worlds.is_empty() {
        for node in nodes.iter().filter(|n| n.facts_from != FactsSource::Wasm) {
            unresolved.push(Unresolved {
                node: Some(node.id.clone()),
                reason: "no-world".into(),
                hint: "component is not built and no world was found under wit/, so imports \
                       and exports are unknown; build the project (or re-run with --build) \
                       to derive from the component itself"
                    .into(),
            });
        }
        return;
    }

    let sole = worlds.len() == 1 && pending == 1;
    for node in nodes
        .iter_mut()
        .filter(|n| n.facts_from != FactsSource::Wasm)
    {
        let matched = worlds
            .iter()
            .find(|w| w.owner.as_deref() == Some(node.id.as_str()))
            .map(|w| (w, "own-directory"))
            .or_else(|| {
                worlds
                    .iter()
                    .find(|w| w.owner.is_none() && w.name == node.id)
                    .map(|w| (w, "exact"))
            })
            .or_else(|| {
                let suffixed = format!("{}-component", node.id);
                worlds
                    .iter()
                    .find(|w| w.owner.is_none() && w.name == suffixed)
                    .map(|w| (w, "component-suffix"))
            })
            .or_else(|| {
                worlds
                    .iter()
                    .find(|w| {
                        w.owner.is_none()
                            && node
                                .id
                                .strip_prefix(&w.name)
                                .is_some_and(|rest| rest.starts_with('-'))
                    })
                    .map(|w| (w, "prefix"))
            })
            // The sole-world fallback never claims a world another component's directory owns.
            .or_else(|| {
                sole.then(|| worlds.first())
                    .flatten()
                    .filter(|w| w.owner.is_none())
                    .map(|w| (w, "sole"))
            });

        match matched {
            Some((world, how)) => {
                node.world = Some(world.name.clone());
                node.world_match = Some(how.to_string());
                node.imports.clone_from(&world.imports);
                node.exports.clone_from(&world.exports);
                node.facts_from = FactsSource::Wit;
            }
            None => unresolved.push(Unresolved {
                node: Some(node.id.clone()),
                reason: "unbound-world".into(),
                hint: format!(
                    "no world matched this component; candidates were: {}",
                    worlds
                        .iter()
                        .map(|w| w.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        }
    }
}

/// The host's linking rule, statically: a sole exporter links; duplicates are ambiguous.
fn derive_edges(nodes: &[Node], unresolved: &mut Vec<Unresolved>) -> Vec<Edge> {
    let mut exporters: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in nodes {
        for export in &node.exports {
            exporters
                .entry(export.as_str())
                .or_default()
                .push(node.id.as_str());
        }
    }

    let mut edges = Vec::new();
    for node in nodes {
        for import in &node.imports {
            let Some(owners) = exporters.get(import.as_str()) else {
                continue; // Host-provided; counted as a capability instead.
            };
            match owners.as_slice() {
                [only] if *only != node.id => edges.push(Edge {
                    from: node.id.clone(),
                    to: (*only).to_string(),
                    via: import.clone(),
                    kind: EdgeKind::Direct,
                    subject: None,
                }),
                [_] => {} // Self-import; legal, but not an edge between nodes.
                many => unresolved.push(Unresolved {
                    node: Some(node.id.clone()),
                    reason: "ambiguous-import".into(),
                    hint: format!(
                        "`{import}` is exported by {} components ({}), so the host drops it \
                         from the export map and this import will fail to link",
                        many.len(),
                        many.join(", ")
                    ),
                }),
            }
        }
    }

    // Messaging is host-mediated; a node that both publishes and handles replies
    // to a dynamic subject, so it is reported rather than drawn.
    let (repliers, publishers): (Vec<&Node>, Vec<&Node>) = nodes
        .iter()
        .filter(|n| n.imports.iter().any(|i| is_messaging_consumer(i)))
        .partition(|n| n.exports.iter().any(|e| is_messaging_handler(e)));
    for replier in &repliers {
        unresolved.push(Unresolved {
            node: Some(replier.id.clone()),
            reason: "dynamic-publish-subject".into(),
            hint: "imports the messaging consumer while also handling messages, so it \
                   publishes to a subject carried by the incoming message; the destination \
                   is not statically known"
                .into(),
        });
    }
    let subscribers: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.exports.iter().any(|e| is_messaging_handler(e)))
        .collect();
    for publisher in &publishers {
        for subscriber in &subscribers {
            if publisher.id == subscriber.id {
                continue;
            }
            let via = subscriber
                .exports
                .iter()
                .find(|e| is_messaging_handler(e))
                .cloned()
                .unwrap_or_default();
            if subscriber.subscriptions.is_empty() {
                edges.push(Edge {
                    from: publisher.id.clone(),
                    to: subscriber.id.clone(),
                    via: via.clone(),
                    kind: EdgeKind::Messaging,
                    subject: None,
                });
            }
            for subject in &subscriber.subscriptions {
                edges.push(Edge {
                    from: publisher.id.clone(),
                    to: subscriber.id.clone(),
                    via: via.clone(),
                    kind: EdgeKind::Messaging,
                    subject: Some(subject.clone()),
                });
            }
        }
    }

    edges
}

/// Interfaces every workload gets unconditionally, so listing them tells a picker nothing.
const PLUMBING_PREFIXES: [&str; 6] = [
    "wasi:io/",
    "wasi:clocks/",
    "wasi:random/",
    "wasi:cli/",
    "wasi:filesystem/",
    // Companion types, never a choice on their own.
    "wasi:http/types",
];

/// Imports nothing in the workload exports — the host's definition of a capability.
fn derive_capabilities(nodes: &[Node]) -> Vec<String> {
    let exported: BTreeSet<&str> = nodes
        .iter()
        .flat_map(|n| n.exports.iter().map(String::as_str))
        .collect();
    nodes
        .iter()
        .flat_map(|n| n.imports.iter())
        .filter(|i| !exported.contains(i.as_str()))
        .filter(|i| !PLUMBING_PREFIXES.iter().any(|p| i.starts_with(p)))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Flag what static derivation structurally cannot see.
fn detect_blind_spots(
    project: &Path,
    nodes: &[Node],
    edges: &[Edge],
    unresolved: &mut Vec<Unresolved>,
) -> Result<()> {
    let [
        uses_wstd_http,
        uses_wstd_main,
        uses_tcp_listener,
        uses_tcp_connect,
    ] = source_probes(
        project,
        &[
            "#[wstd::http_server]",
            "#[wstd::main]",
            "TcpListener",
            "TcpStream::connect",
        ],
    )?;

    // Only reachable for WIT-sourced nodes: a decoded component reports injected exports.
    let macro_injected = uses_wstd_http || uses_wstd_main;
    for node in nodes.iter().filter(|n| n.facts_from == FactsSource::Wit) {
        if node.world.is_some() && node.exports.is_empty() && macro_injected {
            unresolved.push(Unresolved {
                node: Some(node.id.clone()),
                reason: "macro-injected-export".into(),
                hint: "world declares no exports but the source uses a wstd macro, which \
                       injects one (e.g. wasi:http/incoming-handler); build this project \
                       (or re-run with --build) to read the real trigger off the component"
                    .into(),
            });
        }
    }

    // Raw socket wiring between components never appears in WIT or config.
    if uses_tcp_listener && uses_tcp_connect {
        let has_derived_edge = !edges.is_empty();
        unresolved.push(Unresolved {
            node: None,
            reason: "socket-edge".into(),
            hint: format!(
                "source binds a TcpListener and connects a TcpStream, so components are \
                 wired by a hardcoded address in Rust rather than by WIT{}",
                if has_derived_edge {
                    ""
                } else {
                    "; no edge could be derived for this project"
                }
            ),
        });
    }
    Ok(())
}

/// Which of `needles` appear in the project's Rust sources; probed per file
/// with an early exit once every needle has been seen.
fn source_probes<const N: usize>(project: &Path, needles: &[&str; N]) -> Result<[bool; N]> {
    fn walk(dir: &Path, needles: &[&str], hits: &mut [bool], depth: usize) -> Result<()> {
        if depth > 4 || hits.iter().all(|hit| *hit) {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                if name != "target" && name != ".git" {
                    walk(&path, needles, hits, depth + 1)?;
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                let source = fs::read_to_string(&path).unwrap_or_default();
                for (needle, hit) in needles.iter().zip(hits.iter_mut()) {
                    if !*hit && source.contains(needle) {
                        *hit = true;
                    }
                }
                if hits.iter().all(|hit| *hit) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    let mut hits = [false; N];
    walk(project, needles, &mut hits, 0)?;
    Ok(hits)
}

fn classify(nodes: &[Node], edges: &[Edge]) -> Shape {
    if edges.iter().any(|e| e.kind == EdgeKind::Messaging) {
        return Shape::FanOut;
    }
    if nodes.iter().any(|n| n.role == Role::Service) {
        return Shape::Service;
    }
    if edges.iter().any(|e| e.kind == EdgeKind::Direct) {
        // Out-degree separates a chain from a fan-out.
        let mut fanned: BTreeMap<&str, usize> = BTreeMap::new();
        for edge in edges.iter().filter(|e| e.kind == EdgeKind::Direct) {
            *fanned.entry(edge.from.as_str()).or_default() += 1;
        }
        if fanned.values().any(|count| *count > 1) {
            return Shape::FanOut;
        }
        return Shape::Chain;
    }
    if nodes.len() <= 1 {
        Shape::Single
    } else {
        Shape::Unknown
    }
}

/// First `# ` heading of the project README, used as the wizard's label.
fn read_title(project: &Path) -> Option<String> {
    let readme = fs::read_to_string(project.join("README.md")).ok()?;
    readme
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Node {
        new_node(id, None, Vec::new())
    }

    #[test]
    fn a_path_that_climbs_out_of_the_project_is_an_escape() {
        assert!(escapes(Path::new("../../target/release/thing.wasm")));
        assert!(escapes(Path::new("/abs/target/thing.wasm")));
        // Depth is counted, so a mid-path climb is caught too.
        assert!(escapes(Path::new("build/../../thing.wasm")));

        assert!(!escapes(Path::new("target/release/thing.wasm")));
        assert!(!escapes(Path::new("./target/thing.wasm")));
        assert!(!escapes(Path::new("build/../target/thing.wasm")));
    }

    #[test]
    fn an_origin_stub_is_not_something_to_derive_from() {
        // A linked entry is a `.wash/catalog.yaml` and nothing else; the source lives elsewhere.
        let root = tempfile::tempdir().expect("tempdir");
        let stub = root.path().join("workload-examples").join("linked-thing");
        fs::create_dir_all(stub.join(".wash")).expect("mkdir");
        fs::write(
            stub.join(".wash").join("catalog.yaml"),
            "schema: 1\nid: linked-thing\nsource: workload-examples/linked-thing\n\
             repo: https://github.com/someone/thing\nshape: single\nnodes: []\n",
        )
        .expect("write");

        assert!(
            discover(root.path()).is_empty(),
            "a stub has no source to derive from"
        );
    }

    #[test]
    fn a_config_only_overlay_is_not_discovered_as_a_project() {
        let root = tempfile::tempdir().expect("tempdir");

        let project = root.path().join("real");
        fs::create_dir_all(project.join(".wash")).expect("mkdir");
        fs::write(
            project.join(".wash").join("config.yaml"),
            "build:\n  component_path: target/wasm32-wasip2/release/real.wasm\n",
        )
        .expect("write");

        // Same shape as examples/otel-config/chain/*.
        let overlay = root.path().join("overlay");
        fs::create_dir_all(overlay.join(".wash")).expect("mkdir");
        fs::write(
            overlay.join(".wash").join("config.yaml"),
            "build:\n  component_path: ../../target/wasm32-wasip2/release/real.wasm\n",
        )
        .expect("write");

        let found = discover(root.path());
        assert!(found.contains(&project), "{found:?}");
        assert!(
            !found.contains(&overlay),
            "an overlay would scaffold to a directory with no source: {found:?}"
        );
    }

    fn world(name: &str, imports: &[&str], exports: &[&str], owner: Option<&str>) -> WorldFacts {
        WorldFacts {
            name: name.into(),
            imports: imports.iter().map(|s| (*s).to_string()).collect(),
            exports: exports.iter().map(|s| (*s).to_string()).collect(),
            owner: owner.map(str::to_string),
        }
    }

    #[test]
    fn parse_worlds_reads_imports_and_exports() {
        let src = "package a:b@0.1.0;\nworld task {\n  import wasmcloud:messaging/consumer@0.2.0;\n  export wasmcloud:messaging/handler@0.2.0;\n}\n";
        let worlds = parse_worlds(&strip_comments(src));
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0].name, "task");
        assert_eq!(worlds[0].imports, ["wasmcloud:messaging/consumer@0.2.0"]);
        assert_eq!(worlds[0].exports, ["wasmcloud:messaging/handler@0.2.0"]);
    }

    #[test]
    fn comments_mentioning_import_are_not_parsed_as_items() {
        // Every shipped template has prose like this above its world.
        let src = "// wasi:http/incoming-handler is exported via a macro,\n// so import x; here is prose.\nworld hello {\n}\n";
        let worlds = parse_worlds(&strip_comments(src));
        assert_eq!(worlds.len(), 1);
        assert!(worlds[0].imports.is_empty());
        assert!(worlds[0].exports.is_empty());
    }

    #[test]
    fn a_single_exporter_links_directly() {
        let nodes = vec![
            Node {
                imports: vec!["middleware".into()],
                ..node("caller")
            },
            Node {
                exports: vec!["middleware".into()],
                ..node("mid")
            },
        ];
        let mut unresolved = Vec::new();
        let edges = derive_edges(&nodes, &mut unresolved);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "caller");
        assert_eq!(edges[0].to, "mid");
    }

    #[test]
    fn duplicate_exporters_are_ambiguous_not_edges() {
        // The host drops a twice-exported interface from the export map.
        let nodes = vec![
            Node {
                imports: vec!["acme:x/iface".into()],
                ..node("caller")
            },
            Node {
                exports: vec!["acme:x/iface".into()],
                ..node("a")
            },
            Node {
                exports: vec!["acme:x/iface".into()],
                ..node("b")
            },
        ];
        let mut unresolved = Vec::new();
        let edges = derive_edges(&nodes, &mut unresolved);
        assert!(edges.is_empty(), "ambiguous import must not become an edge");
        assert_eq!(unresolved[0].reason, "ambiguous-import");
    }

    #[test]
    fn imports_no_component_exports_are_capabilities() {
        let nodes = vec![
            Node {
                imports: vec!["wasi:keyvalue/store@0.2.0-draft".into(), "mid".into()],
                ..node("a")
            },
            Node {
                exports: vec!["mid".into()],
                ..node("b")
            },
        ];
        // `mid` is satisfied in-workload, so only the wasi import is a capability.
        assert_eq!(
            derive_capabilities(&nodes),
            vec!["wasi:keyvalue/store@0.2.0-draft"]
        );
    }

    #[test]
    fn a_world_in_a_components_own_directory_binds_to_that_component() {
        // A world inside a component's `wit/` is that component's.
        let worlds = vec![
            world("transcriber", &[], &["producer"], Some("transcriber")),
            world("web", &["producer"], &[], Some("web")),
        ];
        let mut nodes = vec![node("web"), node("transcriber")];
        let mut unresolved = Vec::new();
        bind_worlds(&mut nodes, &worlds, &mut unresolved);

        assert!(unresolved.is_empty(), "{unresolved:?}");
        assert_eq!(nodes[0].world_match.as_deref(), Some("own-directory"));
        assert_eq!(nodes[0].imports, ["producer"]);
        assert_eq!(nodes[1].exports, ["producer"]);
    }

    #[test]
    fn a_components_own_world_is_never_claimed_by_a_name_match() {
        // The sole-world fallback must not hand `step1` another component's world.
        let worlds = vec![world("step", &[], &["wrong"], Some("other"))];
        let mut nodes = vec![node("step1")];
        let mut unresolved = Vec::new();
        bind_worlds(&mut nodes, &worlds, &mut unresolved);
        assert_eq!(unresolved.len(), 1, "must not borrow another world");
    }

    #[test]
    fn world_binds_by_prefix_for_a_shared_worker_world() {
        // `task-leet` and `task-reverse` really do share `world task`.
        let worlds = vec![world(
            "task",
            &[],
            &["wasmcloud:messaging/handler@0.2.0"],
            None,
        )];
        let mut nodes = vec![node("task-leet"), node("task-reverse")];
        let mut unresolved = Vec::new();
        bind_worlds(&mut nodes, &worlds, &mut unresolved);
        assert!(unresolved.is_empty());
        assert_eq!(nodes[0].world_match.as_deref(), Some("prefix"));
    }

    #[test]
    fn direct_edges_alone_do_not_identify_a_shape() {
        // A chain and a fan-out both use direct edges; out-degree separates them.
        let nodes = vec![node("a"), node("b"), node("c")];
        let chain = vec![
            Edge {
                from: "a".into(),
                to: "b".into(),
                via: "x".into(),
                kind: EdgeKind::Direct,
                subject: None,
            },
            Edge {
                from: "b".into(),
                to: "c".into(),
                via: "y".into(),
                kind: EdgeKind::Direct,
                subject: None,
            },
        ];
        assert_eq!(classify(&nodes, &chain), Shape::Chain);

        let fan = vec![
            Edge {
                from: "a".into(),
                to: "b".into(),
                via: "x".into(),
                kind: EdgeKind::Direct,
                subject: None,
            },
            Edge {
                from: "a".into(),
                to: "c".into(),
                via: "y".into(),
                kind: EdgeKind::Direct,
                subject: None,
            },
        ];
        assert_eq!(classify(&nodes, &fan), Shape::FanOut);
    }

    #[test]
    fn discovery_finds_projects_at_any_depth() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        for rel in ["components/alpha", "workloads/beta/inner"] {
            let dir = tempdir.path().join(rel).join(".wash");
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join("config.toml"), "[build]\n").expect("write");
        }
        // Build output must never be mistaken for a project.
        let noise = tempdir.path().join("target/debug/thing/.wash");
        std::fs::create_dir_all(&noise).expect("mkdir");
        std::fs::write(noise.join("config.yaml"), "build:\n").expect("write");

        let found = discover(tempdir.path());
        assert_eq!(found.len(), 2, "found {found:?}");
        assert!(found.iter().any(|p| p.ends_with("alpha")));
        assert!(found.iter().any(|p| p.ends_with("inner")));
    }

    #[test]
    fn every_config_format_wash_accepts_is_discovered() {
        for name in CONFIG_FILES {
            let tempdir = tempfile::tempdir().expect("tempdir");
            let dir = tempdir.path().join("proj").join(".wash");
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join(name), "").expect("write");
            assert_eq!(discover(tempdir.path()).len(), 1, "{name} should be found");
        }
    }
}
