//! Declarative spec for a host component plugin: a host-unique id plus where to
//! fetch its wasm from.
//!
//! A spec is produced from a `wash host --host-plugin` flag string (via
//! [`FromStr`]) or converted from a `wash dev` config entry, then resolved to a
//! running plugin by [`super::component_host::load_component_plugin`]. That
//! loader is gated on the `host-component-plugins` feature; this spec type is
//! always compiled so the CLI can accept a plugin declaration and fail with a
//! clear error on a build that lacks the feature, rather than silently dropping
//! it.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context as _, anyhow, bail, ensure};

use crate::oci::OciPullPolicy;

/// Where a host component plugin's wasm bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// Pulled from an OCI registry by image reference.
    Oci {
        image: String,
        pull_policy: OciPullPolicy,
    },
    /// Read from a local file path.
    File(PathBuf),
}

/// A host component plugin to load: a host-unique id, a source for its wasm, and
/// optional supervision/integrity settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPluginSpec {
    /// Host-unique plugin id. Collides loudly with an existing plugin's id at
    /// registration time (`HostBuilder::with_plugin` dedupes).
    pub id: String,
    pub source: PluginSource,
    /// Supervised driver restarts before the plugin is declared dead. `None`
    /// uses the loader default.
    pub max_restarts: Option<u32>,
    /// Optional OCI digest to pin for supply-chain integrity. Only meaningful
    /// for [`PluginSource::Oci`]; the loader rejects it on a file source.
    pub expected_digest: Option<String>,
}

impl ComponentPluginSpec {
    /// Build a spec from an id and source with default supervision/integrity
    /// settings (no restart-cap override, no digest pin).
    pub fn from_plugin_source(id: impl Into<String>, source: PluginSource) -> Self {
        Self {
            id: id.into(),
            source,
            max_restarts: None,
            expected_digest: None,
        }
    }
}

/// Parse a `wash host --host-plugin` value: a comma-separated list of
/// `key=value` fields. Required: `id`, and exactly one of `image` / `file`.
/// Optional: `pull` (image only), `max-restarts`, `digest` (image only).
///
/// ```text
/// id=acme-kv,image=ghcr.io/acme/kv-host:1.0.0,pull=ifNotPresent,max-restarts=3
/// id=acme-kv,file=./build/kv_plugin.wasm
/// ```
impl FromStr for ComponentPluginSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut id = None;
        let mut image = None;
        let mut file = None;
        let mut pull = None;
        let mut max_restarts = None;
        let mut digest = None;

        for field in s.split(',') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let (key, value) = field
                .split_once('=')
                .ok_or_else(|| anyhow!("host plugin field {field:?} is not `key=value`"))?;
            let value = value.trim().to_string();
            ensure!(
                !value.is_empty(),
                "host plugin field {:?} has an empty value",
                key.trim()
            );
            match key.trim() {
                "id" => id = Some(value),
                "image" => image = Some(value),
                "file" => file = Some(PathBuf::from(value)),
                "pull" | "pull-policy" => pull = Some(value.parse()?),
                "max-restarts" => {
                    max_restarts = Some(value.parse().with_context(|| {
                        format!("max-restarts must be a non-negative integer, got {value:?}")
                    })?)
                }
                "digest" => digest = Some(value),
                other => bail!(
                    "unknown host plugin field {other:?}; expected id|image|file|pull|max-restarts|digest"
                ),
            }
        }

        let id = id.context("host plugin spec is missing required `id=`")?;
        let source = match (image, file) {
            (Some(image), None) => PluginSource::Oci {
                image,
                pull_policy: pull.unwrap_or(OciPullPolicy::IfNotPresent),
            },
            (None, Some(file)) => {
                ensure!(
                    pull.is_none(),
                    "host plugin '{id}': `pull=` applies only to `image=` sources"
                );
                PluginSource::File(file)
            }
            (Some(_), Some(_)) => {
                bail!("host plugin '{id}' sets both `image=` and `file=`; use exactly one")
            }
            (None, None) => bail!("host plugin '{id}' needs an `image=` or `file=` source"),
        };

        Ok(Self {
            id,
            source,
            max_restarts,
            expected_digest: digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_oci_spec_with_all_fields() {
        let spec: ComponentPluginSpec =
            "id=acme-kv,image=ghcr.io/acme/kv:1.0.0,pull=always,max-restarts=5,digest=sha256:abc"
                .parse()
                .unwrap();
        assert_eq!(spec.id, "acme-kv");
        assert_eq!(
            spec.source,
            PluginSource::Oci {
                image: "ghcr.io/acme/kv:1.0.0".into(),
                pull_policy: OciPullPolicy::Always,
            }
        );
        assert_eq!(spec.max_restarts, Some(5));
        assert_eq!(spec.expected_digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn parses_file_spec_and_defaults_pull_policy_for_oci() {
        let file: ComponentPluginSpec = "id=kv,file=./kv.wasm".parse().unwrap();
        assert_eq!(file.source, PluginSource::File("./kv.wasm".into()));

        let oci: ComponentPluginSpec = "id=kv,image=ghcr.io/acme/kv:1".parse().unwrap();
        assert_eq!(
            oci.source,
            PluginSource::Oci {
                image: "ghcr.io/acme/kv:1".into(),
                pull_policy: OciPullPolicy::IfNotPresent,
            }
        );
    }

    #[test]
    fn rejects_missing_id_both_sources_and_neither_source() {
        assert!("image=ghcr.io/x:1".parse::<ComponentPluginSpec>().is_err());
        assert!(
            "id=x,image=ghcr.io/x:1,file=./x.wasm"
                .parse::<ComponentPluginSpec>()
                .is_err()
        );
        assert!("id=x".parse::<ComponentPluginSpec>().is_err());
        assert!(
            "id=x,file=./x.wasm,pull=always"
                .parse::<ComponentPluginSpec>()
                .is_err()
        );
        assert!(
            "id=x,image=ghcr.io/x:1,bogus=1"
                .parse::<ComponentPluginSpec>()
                .is_err()
        );
    }
}
