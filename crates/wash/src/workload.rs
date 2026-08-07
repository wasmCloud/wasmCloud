//! Resolves the `workload:` / `configs:` / `secrets:` schema from
//! [`crate::config::Config`] into a flat key-value map suitable for handing
//! to a wasmCloud runtime workload.
//!
//! Configs and secrets carry distinct types ([`ConfigSource`],
//! [`SecretSource`]) so the stricter secret-handling posture can only be
//! applied to secrets:
//!
//! * **Path containment.** Every `file:` path is canonicalized and rejected
//!   if it escapes the project directory (defense against the
//!   `CVE-2025-62725` class, a joining attacker-controlled path strings with
//!   a working directory without canonicalization).
//! * **Permission check (secrets only).** A secret `file:` source must be
//!   mode `0600` or `0400` on Unix. Looser modes are refused, matching the
//!   Kubernetes Secret volume `defaultMode: 0400` posture.
//! * **Repo-tree warning (secrets only).** When a resolved secret `file:`
//!   path sits inside the repo working tree we warn. This is the class of
//!   mistake behind the `.env` mass-exfiltration campaigns.
//! * **Logging hygiene.** Errors and traces reference sources and keys by
//!   name only; resolved values never appear in log output (relevant given
//!   `wash dev`'s OpenTelemetry hook).
//! * **Memory only.** Resolved values are returned by value and never
//!   written back to disk.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use wash_runtime::host::allowed_hosts::AllowedHost;
use wash_runtime::host::allowed_ip_name::AllowedIpName;
use wash_runtime::host::allowed_loopback::AllowedLoopbackPort;

use wash_runtime::config_source::resolve_environment_layer;

use crate::config::{Config, DevComponent};

/// Resolved workload values ready to be applied to a runtime `LocalResources`.
#[derive(Debug, Default, Clone)]
pub struct ResolvedWorkload {
    /// Environment variables (wasi:cli/env), merged from inline + configFrom + secretFrom.
    pub environment: HashMap<String, String>,
    /// Opaque key-value config delivered to the component.
    pub config: HashMap<String, String>,
    /// Outbound HTTP allowlist (typed). When the wash YAML omitted
    /// `allowedHosts`, this is `[AllowedHost::Any]` (allow-all) — the
    /// serde default fires at deserialize time. An explicit
    /// `allowedHosts: []` in YAML stays empty here, and the runtime
    /// (see [`wash_runtime::host::http::check_allowed_hosts`]) interprets
    /// empty as deny-all.
    pub allowed_hosts: Vec<AllowedHost>,
    /// Names the component may resolve through
    /// `wasi:sockets/ip-name-lookup`. Empty denies every lookup, which is
    /// what an omitted `allowedIpNameLookups` resolves to.
    pub allowed_ip_name_lookups: Vec<AllowedIpName>,
    /// Host-loopback ports this workload may reach through
    /// `host.wasmcloud.internal`. Empty denies every one.
    pub allowed_host_loopback: Vec<AllowedLoopbackPort>,
}

/// Resolves the workload section of a [`Config`], pulling in named entries
/// from the top-level `configs:` and `secrets:` blocks.
///
/// `project_dir` anchors relative `file:` paths and bounds path containment
/// checks. `repo_root` (when known) gates the "secret file lives in the repo
/// tree" warning.
///
/// # Errors
///
/// Returns an error if any `configFrom` / `secretFrom` reference cannot be
/// found in the catalogs, if a `file:` source has an unsafe path (escapes
/// the project directory or has the wrong mode for a secret), if a
/// `fromEnv` reference points at a missing environment variable, or if a
/// `.env` file fails to parse.
pub fn resolve_workload(
    config: &Config,
    project_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<ResolvedWorkload> {
    let Some(workload) = config.workload.as_ref() else {
        return Ok(ResolvedWorkload::default());
    };

    let environment = resolve_environment_layer(
        workload.environment.as_ref(),
        "workload",
        &config.config_sources,
        &config.secret_sources,
        project_dir,
        repo_root,
    )?;

    // `workload.allowed_hosts` is whatever serde produced: `[Any]` when the
    // YAML omitted `allowedHosts` (via `default_allow_all_hosts`), or the
    // user's explicit list (including an explicit empty list, which the
    // runtime treats as deny-all). Either way, pass through unchanged.
    // The substitution lives on the serde default, not here.
    Ok(ResolvedWorkload {
        environment,
        config: workload.config.clone(),
        allowed_hosts: workload.allowed_hosts.clone(),
        allowed_ip_name_lookups: workload.allowed_ip_name_lookups.clone(),
        allowed_host_loopback: workload.allowed_host_loopback.clone(),
    })
}

/// Resolves a `dev.components` entry's overrides on top of the resolved
/// workload-level `base`.
///
/// `environment` / `config` merge over the workload's, component wins on
/// key conflicts. `allowedHosts`, when set, replaces the workload list
/// (an explicit `[]` denies all egress); when omitted the workload list
/// applies. `allowedIpNameLookups`, when set, likewise replaces the workload
/// list (an explicit `[]` denies every lookup).
///
/// # Errors
///
/// Same failure modes as [`resolve_workload`], for the component's own
/// `environment.configFrom` / `environment.secretFrom` references.
pub fn resolve_component_workload(
    base: &ResolvedWorkload,
    component: &DevComponent,
    config: &Config,
    project_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<ResolvedWorkload> {
    let owner = format!("dev component '{}'", component.name);
    let overrides = resolve_environment_layer(
        component.environment.as_ref(),
        &owner,
        &config.config_sources,
        &config.secret_sources,
        project_dir,
        repo_root,
    )?;

    let mut environment = base.environment.clone();
    environment.extend(overrides);

    let mut merged_config = base.config.clone();
    merged_config.extend(component.config.clone());

    let allowed_hosts = component
        .allowed_hosts
        .clone()
        .unwrap_or_else(|| base.allowed_hosts.clone());

    let allowed_ip_name_lookups = component
        .allowed_ip_name_lookups
        .clone()
        .unwrap_or_else(|| base.allowed_ip_name_lookups.clone());
    let allowed_host_loopback = component
        .allowed_host_loopback
        .clone()
        .unwrap_or_else(|| base.allowed_host_loopback.clone());

    Ok(ResolvedWorkload {
        environment,
        config: merged_config,
        allowed_hosts,
        allowed_ip_name_lookups,
        allowed_host_loopback,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{ConfigSource, EnvironmentLayer, SecretSource, WorkloadConfig};

    #[test]
    #[allow(unsafe_code)]
    fn end_to_end_resolve_workload_all_source_types() {
        let project = TempDir::new().unwrap();

        // configFrom uses a file source (no perm check).
        let cfg_file = project.path().join("app.env");
        std::fs::write(
            &cfg_file,
            "APP_FOO=app_foo_value\n# comment\nAPP_BAR=app_bar_value\n",
        )
        .unwrap();

        // secretFrom uses an inline + fromEnv source (file would require 0600,
        // which TempDir defaults don't enforce — keep this test cross-platform).
        let var = format!("WASH_TEST_E2E_{}", uuid::Uuid::new_v4().simple());
        unsafe { std::env::set_var(&var, "shell_value") };

        let workload = WorkloadConfig {
            environment: Some(
                EnvironmentLayer::builder()
                    .config(HashMap::from([(
                        "INLINE_KEY".into(),
                        "inline_value".into(),
                    )]))
                    .config_from(vec!["app".into()])
                    .secret_from(vec!["creds".into()])
                    .build(),
            ),
            config: HashMap::from([("WORKLOAD_CFG".into(), "cfg_value".into())]),
            allowed_hosts: vec!["https://api.example.com".parse().unwrap()],
            allowed_ip_name_lookups: vec![],
            allowed_host_loopback: vec![],
        };

        let configs = BTreeMap::from([(
            "app".to_string(),
            ConfigSource::builder().file(cfg_file.clone()).build(),
        )]);
        let secrets = BTreeMap::from([(
            "creds".to_string(),
            SecretSource::builder()
                .inline(HashMap::from([("DB_USER".into(), "alice".into())]))
                .from_env(vec![var.clone()])
                .build(),
        )]);

        let config = Config {
            workload: Some(workload),
            config_sources: configs,
            secret_sources: secrets,
            ..Default::default()
        };

        let resolved = resolve_workload(&config, project.path(), Some(project.path())).unwrap();

        unsafe { std::env::remove_var(&var) };

        // environment combines: inline + configFrom file values + secretFrom inline + secretFrom from_env
        assert_eq!(
            resolved.environment.get("INLINE_KEY").unwrap(),
            "inline_value"
        );
        assert_eq!(
            resolved.environment.get("APP_FOO").unwrap(),
            "app_foo_value"
        );
        assert_eq!(
            resolved.environment.get("APP_BAR").unwrap(),
            "app_bar_value"
        );
        assert_eq!(resolved.environment.get("DB_USER").unwrap(), "alice");
        assert_eq!(resolved.environment.get(&var).unwrap(), "shell_value");

        // config and allowed_hosts pass through untouched.
        assert_eq!(resolved.config.get("WORKLOAD_CFG").unwrap(), "cfg_value");
        assert_eq!(
            resolved.allowed_hosts,
            vec!["https://api.example.com".parse().unwrap()]
        );
    }

    #[test]
    fn component_overrides_merge_over_workload_base() {
        // environment / config: component entries win on key conflict, but
        // workload-only keys survive the merge.
        let base = ResolvedWorkload {
            environment: HashMap::from([
                ("LOG".into(), "debug".into()),
                ("SHARED".into(), "from_workload".into()),
            ]),
            config: HashMap::from([
                ("flag".into(), "off".into()),
                ("base_only".into(), "1".into()),
            ]),
            allowed_hosts: vec![AllowedHost::Any],
            allowed_ip_name_lookups: vec![],
            allowed_host_loopback: vec![],
        };
        let component = DevComponent {
            environment: Some(
                EnvironmentLayer::builder()
                    .config(HashMap::from([("SHARED".into(), "from_component".into())]))
                    .build(),
            ),
            config: HashMap::from([("flag".into(), "on".into())]),
            ..DevComponent::new("sidecar", "sidecar.wasm")
        };
        let project = TempDir::new().unwrap();

        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();

        assert_eq!(resolved.environment.get("LOG").unwrap(), "debug");
        assert_eq!(
            resolved.environment.get("SHARED").unwrap(),
            "from_component"
        );
        assert_eq!(resolved.config.get("flag").unwrap(), "on");
        assert_eq!(resolved.config.get("base_only").unwrap(), "1");
        // allowedHosts omitted on the component → workload list applies.
        assert_eq!(resolved.allowed_hosts, vec![AllowedHost::Any]);
    }

    #[test]
    fn component_allowed_hosts_replaces_workload_list() {
        let base = ResolvedWorkload {
            allowed_hosts: vec![AllowedHost::Any],
            ..Default::default()
        };
        let project = TempDir::new().unwrap();

        // Explicit list replaces (does not append to) the workload list.
        let component = DevComponent {
            allowed_hosts: Some(vec!["https://api.example.com".parse().unwrap()]),
            ..DevComponent::new("locked", "locked.wasm")
        };
        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();
        assert_eq!(
            resolved.allowed_hosts,
            vec!["https://api.example.com".parse().unwrap()]
        );

        // Explicit empty list replaces too — deny-all for this component
        // even though the workload allows everything.
        let component = DevComponent {
            allowed_hosts: Some(vec![]),
            ..DevComponent::new("sealed", "sealed.wasm")
        };
        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();
        assert!(resolved.allowed_hosts.is_empty());
    }

    #[test]
    fn component_allowed_ip_name_lookups_replaces_workload_setting() {
        let base = ResolvedWorkload {
            allowed_ip_name_lookups: vec!["*".parse().unwrap()],
            ..Default::default()
        };
        let project = TempDir::new().unwrap();

        // Omitted on the component, so the workload list applies.
        let component = DevComponent::new("inherits", "inherits.wasm");
        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();
        assert_eq!(resolved.allowed_ip_name_lookups, vec![AllowedIpName::Any]);

        // A narrower list replaces the workload's, rather than adding to it.
        let component = DevComponent {
            allowed_ip_name_lookups: Some(vec!["*.internal".parse().unwrap()]),
            ..DevComponent::new("narrowed", "narrowed.wasm")
        };
        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();
        assert_eq!(
            resolved.allowed_ip_name_lookups,
            vec!["*.internal".parse().unwrap()]
        );

        // An explicit empty list denies every lookup for this component even
        // though the workload resolves anything.
        let component = DevComponent {
            allowed_ip_name_lookups: Some(vec![]),
            ..DevComponent::new("sealed", "sealed.wasm")
        };
        let resolved =
            resolve_component_workload(&base, &component, &Config::default(), project.path(), None)
                .unwrap();
        assert!(resolved.allowed_ip_name_lookups.is_empty());

        // A component grant applies even when the workload declared none.
        let component = DevComponent {
            allowed_ip_name_lookups: Some(vec!["example.com".parse().unwrap()]),
            ..DevComponent::new("granted", "granted.wasm")
        };
        let resolved = resolve_component_workload(
            &ResolvedWorkload::default(),
            &component,
            &Config::default(),
            project.path(),
            None,
        )
        .unwrap();
        assert_eq!(
            resolved.allowed_ip_name_lookups,
            vec!["example.com".parse().unwrap()]
        );
    }

    #[test]
    fn component_env_refs_resolve_and_missing_ref_names_component() {
        // A component's environment.configFrom resolves against the same
        // top-level `configs:` catalog as the workload's...
        let config = Config {
            config_sources: BTreeMap::from([(
                "sidecar_cfg".to_string(),
                ConfigSource::builder()
                    .inline(HashMap::from([("FROM_CATALOG".into(), "yes".into())]))
                    .build(),
            )]),
            ..Default::default()
        };
        let component = DevComponent {
            environment: Some(
                EnvironmentLayer::builder()
                    .config_from(vec!["sidecar_cfg".into()])
                    .build(),
            ),
            ..DevComponent::new("sidecar", "sidecar.wasm")
        };
        let project = TempDir::new().unwrap();
        let resolved = resolve_component_workload(
            &ResolvedWorkload::default(),
            &component,
            &config,
            project.path(),
            None,
        )
        .unwrap();
        assert_eq!(resolved.environment.get("FROM_CATALOG").unwrap(), "yes");

        // ...and a missing reference names the component in the error.
        let component = DevComponent {
            environment: Some(
                EnvironmentLayer::builder()
                    .config_from(vec!["missing".into()])
                    .build(),
            ),
            ..DevComponent::new("sidecar", "sidecar.wasm")
        };
        let err = resolve_component_workload(
            &ResolvedWorkload::default(),
            &component,
            &Config::default(),
            project.path(),
            None,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("dev component 'sidecar'"), "{}", msg);
        assert!(msg.contains("missing"), "{}", msg);
    }

    #[test]
    fn workload_config_default_is_empty_allowed_hosts_for_deny_close() {
        // Programmatic `WorkloadConfig::default()` is fail-closed: empty
        // `allowed_hosts`. Only the serde deserialize path substitutes
        // `[Any]` when the YAML omits the field.
        let cfg = WorkloadConfig::default();
        assert!(cfg.allowed_hosts.is_empty());
    }

    #[test]
    fn allowed_hosts_defaults_to_any_when_yaml_omits_field() {
        // Serde default fires only on missing field, resolving to [Any].
        let yaml = r#"
workload:
  environment:
    config:
      LOG: debug
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let project = TempDir::new().unwrap();
        let resolved = resolve_workload(&config, project.path(), Some(project.path())).unwrap();
        assert_eq!(resolved.allowed_hosts, vec![AllowedHost::Any]);
    }

    #[test]
    fn allowed_hosts_explicit_empty_in_yaml_stays_empty_for_deny_all() {
        // Explicit `allowedHosts: []` must NOT trigger the serde default;
        // it should round-trip through `resolve_workload` as an empty list
        // so the runtime denies all egress.
        let yaml = r#"
workload:
  allowedHosts: []
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let project = TempDir::new().unwrap();
        let resolved = resolve_workload(&config, project.path(), Some(project.path())).unwrap();
        assert!(resolved.allowed_hosts.is_empty());
    }

    #[test]
    fn resolve_workload_empty_allowed_hosts_denied_by_runtime_check() {
        use wash_runtime::host::http::check_allowed_hosts;
        let yaml = "workload:\n  allowedHosts: []\n";
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let project = TempDir::new().unwrap();
        let resolved = resolve_workload(&config, project.path(), Some(project.path())).unwrap();
        let req = hyper::Request::builder()
            .uri("http://example.com")
            .body(())
            .unwrap();
        assert!(check_allowed_hosts(&req, &resolved.allowed_hosts).is_err());
    }

    #[test]
    fn resolve_workload_default_allowed_hosts_allowed_by_runtime_check() {
        use wash_runtime::host::http::check_allowed_hosts;
        let yaml = "workload:\n  environment:\n    config:\n      X: \"1\"\n";
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let project = TempDir::new().unwrap();
        let resolved = resolve_workload(&config, project.path(), Some(project.path())).unwrap();
        let req = hyper::Request::builder()
            .uri("http://example.com")
            .body(())
            .unwrap();
        assert!(check_allowed_hosts(&req, &resolved.allowed_hosts).is_ok());
    }
}
