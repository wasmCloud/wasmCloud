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

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail, ensure};

use crate::component_source::ComponentSource;
use crate::host::allowed_hosts::AllowedHost;
use crate::host::allowed_ip_name::AllowedIpName;

/// A host component plugin to load: a host-unique id, a source for its wasm, and
/// optional supervision/integrity settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPluginSpec {
    /// Host-unique plugin id. Collides loudly with an existing plugin's id at
    /// registration time (`HostBuilder::with_plugin` dedupes).
    pub id: String,
    pub source: ComponentSource,
    /// Supervised driver restarts before the plugin is declared dead. `None`
    /// uses the loader default.
    pub max_restarts: Option<u32>,
    /// Optional OCI digest to pin for supply-chain integrity. Only meaningful
    /// for [`ComponentSource::Oci`]; the loader rejects it on a file source.
    pub expected_digest: Option<String>,
    /// This plugin's own resolved bind-time config — delivered to every native
    /// capability it imports (e.g. `wasmcloud:secrets`) the same way a
    /// workload's `secretFrom`-sourced config is, via `on-workload-bind`, never
    /// written to a file the plugin itself reads. Already flattened/merged
    /// (literal `config` < `configFrom` < `secretFrom`, last source wins) by
    /// whoever built this spec — empty unless populated by a config source.
    pub config: HashMap<String, String>,
    /// Hosts this plugin's `wasi:http/outgoing-handler` calls may reach.
    /// Empty (the default) denies every outbound HTTP host, matching
    /// [`crate::types::LocalResources`]'s deny-all default for a workload.
    pub allowed_hosts: Arc<[AllowedHost]>,
    /// Names this plugin's `wasi:sockets/ip-name-lookup` calls may resolve.
    /// Empty (the default) denies every DNS lookup.
    pub allowed_ip_name_lookups: Arc<[AllowedIpName]>,
    /// Ports this plugin listens on. Empty (the default) means it binds nothing:
    /// the deny that applied to every plugin before ports existed.
    ///
    /// A plugin may always bind its own private virtual loopback whether or not
    /// it declares a port here — that reaches nothing until something publishes
    /// it. What this list controls is exposure: which of those the host binds a
    /// real port for, and which concrete addresses the plugin may bind itself.
    pub ports: Arc<[crate::host::declared_port::DeclaredPort]>,
}

impl ComponentPluginSpec {
    /// Build a spec from an id and source with default supervision/integrity
    /// settings (no restart-cap override, no digest pin, no bind-time config).
    pub fn from_plugin_source(id: impl Into<String>, source: ComponentSource) -> Self {
        Self {
            id: id.into(),
            source,
            max_restarts: None,
            expected_digest: None,
            config: HashMap::new(),
            allowed_hosts: Arc::from([]),
            allowed_ip_name_lookups: Arc::from([]),
            ports: Arc::from([]),
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
                // A port declaration is a list of records; this syntax is a
                // comma-separated `key=value` list and the comma is already the
                // field separator, so there is nowhere to put one. Say where it
                // does go rather than reporting it as an unknown field.
                "port" | "ports" => bail!(
                    "host plugin ports cannot be declared on --host-plugin; put them under \
                     `host.hostPlugins[].ports` in the `wash host` config file"
                ),
                other => bail!(
                    "unknown host plugin field {other:?}; expected id|image|file|pull|max-restarts|digest"
                ),
            }
        }

        let id = id.context("host plugin spec is missing required `id=`")?;
        let source =
            ComponentSource::from_image_or_file(image, file, pull, &format!("host plugin '{id}'"))?;

        Ok(Self {
            id,
            source,
            max_restarts,
            expected_digest: digest,
            config: HashMap::new(),
            allowed_hosts: Arc::from([]),
            allowed_ip_name_lookups: Arc::from([]),
            ports: Arc::from([]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oci::OciPullPolicy;

    #[test]
    fn parses_oci_spec_with_all_fields() {
        let spec: ComponentPluginSpec =
            "id=acme-kv,image=ghcr.io/acme/kv:1.0.0,pull=always,max-restarts=5,digest=sha256:abc"
                .parse()
                .unwrap();
        assert_eq!(spec.id, "acme-kv");
        assert_eq!(
            spec.source,
            ComponentSource::Oci {
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
        assert_eq!(file.source, ComponentSource::File("./kv.wasm".into()));

        let oci: ComponentPluginSpec = "id=kv,image=ghcr.io/acme/kv:1".parse().unwrap();
        assert_eq!(oci.source, ComponentSource::image("ghcr.io/acme/kv:1"));
    }

    #[test]
    fn rejects_ports_on_the_flag_and_says_where_they_go() {
        let err = "id=kv,file=./kv.wasm,port=8080"
            .parse::<ComponentPluginSpec>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("host.hostPlugins[].ports"), "got: {err}");
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
