//! The workload topology schema: how a project's components are connected, in
//! the host's own linking terms. Derived, never hand-edited ([`crate::derive`]);
//! persisted only as the `dev.wasm.topology` OCI annotation and catalog stubs.

use serde::{Deserialize, Serialize};

/// Schema version; bump when an older reader would misinterpret a newer document.
pub const SCHEMA_VERSION: u32 = 1;

/// One project's derived wiring.
///
/// `repo` absent: clone the repository the catalog walked, at [`source`](Self::source).
/// `repo` present: clone [`repo`](Self::repo) at [`subfolder`](Self::subfolder), or the whole repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topology {
    pub schema: u32,
    /// Directory name of the project; unique within one index.
    pub id: String,
    /// Path to the project relative to the repository root; doubles as `--subfolder`.
    pub source: String,
    /// Origin repository for a linked entry; absent on every derived entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Path within [`repo`](Self::repo); absent clones the whole repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subfolder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub shape: Shape,
    /// Imports no component in this workload satisfies, i.e. host-provided.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    /// What derivation could not determine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<Unresolved>,
}

/// The architecture a picker offers this project under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// One component, one trigger.
    Single,
    /// Ingress calls another component directly, which may call a third.
    Chain,
    /// Ingress publishes; N workers consume by subject.
    FanOut,
    /// A long-lived `wasi:cli/run` service alongside a component.
    Service,
    /// Multiple components with no derivable edge between them.
    Unknown,
}

impl Shape {
    /// Heading used when grouping projects by architecture.
    pub fn label(self) -> &'static str {
        match self {
            Shape::Single => "SINGLE COMPONENT",
            Shape::Chain => "CHAIN",
            Shape::FanOut => "FAN-OUT",
            Shape::Service => "SERVICE",
            Shape::Unknown => "OTHER",
        }
    }

    /// Sort key for those headings; architectures with structure come first.
    pub fn rank(self) -> u8 {
        match self {
            Shape::Chain => 0,
            Shape::FanOut => 1,
            Shape::Service => 2,
            Shape::Unknown => 3,
            Shape::Single => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    /// How `world` was bound to this node; only meaningful when `facts_from` is `wit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_match: Option<String>,
    /// Where this node's interface names came from.
    pub facts_from: FactsSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<String>,
}

/// Provenance of a node's interface names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactsSource {
    /// Decoded from the built component; the authoritative surface the host links against.
    Wasm,
    /// Scanned from `wit/`. Misses anything a macro injects.
    Wit,
    /// Neither available.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Exports an HTTP handler; the host's server delivers to it.
    Ingress,
    /// Exports a messaging handler; the host's subscriber delivers to it.
    Worker,
    /// Exports `wasi:cli/run` and is started as a long-lived service.
    Service,
    /// Reached only by another component.
    Component,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub via: String,
    pub kind: EdgeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// In-process call: the importer's import resolved to the exporter.
    Direct,
    /// Host-mediated: publisher to subject-matched subscriber.
    Messaging,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unresolved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Short kebab-case classification, e.g. `socket-edge`.
    pub reason: String,
    pub hint: String,
}

// Interface classification, version-tolerant on purpose: `@0.2.2` and `@0.2.0`
// are the same interface for these questions.

/// Whether this interface is an HTTP entry point, in either preview.
pub fn is_http_handler(iface: &str) -> bool {
    iface.starts_with("wasi:http/incoming-handler") || iface.starts_with("wasi:http/handler")
}

/// Whether this interface receives broker messages.
pub fn is_messaging_handler(iface: &str) -> bool {
    iface.starts_with("wasmcloud:messaging/handler")
}

/// Whether this interface publishes to the broker.
pub fn is_messaging_consumer(iface: &str) -> bool {
    iface.starts_with("wasmcloud:messaging/consumer")
}

/// Whether this interface is a long-lived entry point.
pub fn is_cli_run(iface: &str) -> bool {
    iface.starts_with("wasi:cli/run")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosted() -> Topology {
        Topology {
            schema: SCHEMA_VERSION,
            id: "http-handler".into(),
            source: "templates/http-handler".into(),
            repo: None,
            subfolder: None,
            title: None,
            shape: Shape::Single,
            capabilities: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    #[test]
    fn an_entry_with_no_origin_does_not_mention_one() {
        // If these ever stop serialising away, every artifact digest changes.
        let yaml = serde_yaml_ng::to_string(&hosted()).expect("serialize");
        assert!(!yaml.contains("repo:"), "{yaml}");
        assert!(!yaml.contains("subfolder:"), "{yaml}");
    }

    #[test]
    fn an_origin_round_trips_when_there_is_one() {
        let linked = Topology {
            repo: Some("https://github.com/someone/thing".into()),
            subfolder: Some("components/thing".into()),
            ..hosted()
        };
        let yaml = serde_yaml_ng::to_string(&linked).expect("serialize");
        let back: Topology = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(
            back.repo.as_deref(),
            Some("https://github.com/someone/thing")
        );
        assert_eq!(back.subfolder.as_deref(), Some("components/thing"));
    }

    #[test]
    fn a_document_written_before_these_fields_existed_still_reads() {
        let yaml = "schema: 1\nid: old\nsource: templates/old\nshape: single\nnodes: []\n";
        let back: Topology = serde_yaml_ng::from_str(yaml).expect("deserialize");
        assert!(back.repo.is_none() && back.subfolder.is_none());
    }
}
