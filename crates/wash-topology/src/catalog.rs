//! The published catalog: a repository's entries, derived once in its CI and
//! served as a single JSON document the wizard fetches instead of cloning.
//! Derivation stays the only producer; this is a CI-built cache of it.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::schema::Topology;

/// Filename a published catalog is stored under, at a repository's root.
pub const CATALOG_NAME: &str = "catalog.json";

/// Catalog document version; bump when an older reader would misinterpret it.
pub const CATALOG_SCHEMA: u32 = 1;

/// Reminder stamped into the document, since JSON has no comments.
const GENERATED_BY: &str = "cargo xtask example-index --build --write-catalog \
(wasmCloud repo) or wash wizard index --write-catalog; do not edit by hand";

#[derive(Serialize, Deserialize)]
struct Catalog {
    #[serde(rename = "_generated", default)]
    generated: String,
    schema: u32,
    entries: Vec<Topology>,
}

/// Serialize entries as a catalog document. Callers sort by id first so the
/// bytes are deterministic and a staleness check can compare strings.
pub fn to_catalog_json(entries: &[Topology]) -> Result<String> {
    let doc = Catalog {
        generated: GENERATED_BY.to_string(),
        schema: CATALOG_SCHEMA,
        entries: entries.to_vec(),
    };
    let json = serde_json::to_string_pretty(&doc).context("failed to serialize the catalog")?;
    Ok(format!("{json}\n"))
}

/// Parse a catalog document, refusing a schema this reader would misread.
pub fn from_catalog_json(raw: &str) -> Result<Vec<Topology>> {
    let doc: Catalog = serde_json::from_str(raw).context("failed to parse the catalog")?;
    if doc.schema != CATALOG_SCHEMA {
        bail!(
            "catalog schema {} is newer than this wash understands ({CATALOG_SCHEMA})",
            doc.schema
        );
    }
    Ok(doc.entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{SCHEMA_VERSION, Shape};

    fn entry(id: &str) -> Topology {
        Topology {
            schema: SCHEMA_VERSION,
            id: id.into(),
            source: format!("templates/{id}"),
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
    fn a_catalog_round_trips() {
        let json = to_catalog_json(&[entry("a"), entry("b")]).expect("serialize");
        let back = from_catalog_json(&json).expect("parse");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, "a");
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_misread() {
        let json = to_catalog_json(&[entry("a")])
            .expect("serialize")
            .replace("\"schema\": 1", "\"schema\": 99");
        assert!(from_catalog_json(&json).is_err());
    }
}
