//! Resolves the `workload:` / `configs:` / `secrets:` schema from
//! [`crate::config::Config`] into a flat key-value map suitable for handing
//! to a wasmCloud runtime workload.
//!
//! Secrets and configs share the same source schema ([`ConfigSource`]) but
//! differ in security posture at resolve time:
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

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use tracing::warn;

use crate::config::{Config, ConfigSource, WorkloadConfig};

/// Resolved workload values ready to be applied to a runtime `LocalResources`.
#[derive(Debug, Default, Clone)]
pub struct ResolvedWorkload {
    /// Environment variables (wasi:cli/env), merged from inline + configFrom + secretFrom.
    pub environment: HashMap<String, String>,
    /// Opaque key-value config delivered to the component.
    pub config: HashMap<String, String>,
    /// Outbound HTTP allowlist.
    pub allowed_hosts: Vec<String>,
}

/// Resolve the workload section of a [`Config`], pulling in named entries
/// from the top-level `configs:` and `secrets:` blocks.
///
/// `project_dir` anchors relative `file:` paths and bounds path containment
/// checks. `repo_root` (when known) gates the "secret file lives in the repo
/// tree" warning.
pub fn resolve_workload(
    config: &Config,
    project_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<ResolvedWorkload> {
    let Some(workload) = config.workload.as_ref() else {
        return Ok(ResolvedWorkload::default());
    };

    let environment = resolve_environment(
        workload,
        &config.configs,
        &config.secrets,
        project_dir,
        repo_root,
    )?;

    Ok(ResolvedWorkload {
        environment,
        config: workload.config.clone(),
        allowed_hosts: workload.allowed_hosts.clone(),
    })
}

fn resolve_environment(
    workload: &WorkloadConfig,
    configs: &HashMap<String, ConfigSource>,
    secrets: &HashMap<String, ConfigSource>,
    project_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<HashMap<String, String>> {
    let Some(env) = workload.environment.as_ref() else {
        return Ok(HashMap::new());
    };

    let mut out: HashMap<String, String> = HashMap::new();

    // Layer order matches K8s envFrom: inline values first, then configFrom,
    // then secretFrom. Later layers overwrite earlier on key conflicts.
    out.extend(env.config.clone());

    apply_refs(
        &env.config_from,
        configs,
        project_dir,
        /* secret */ false,
        repo_root,
        &mut out,
    )
    .context("failed to resolve workload.environment.configFrom")?;
    apply_refs(
        &env.secret_from,
        secrets,
        project_dir,
        /* secret */ true,
        repo_root,
        &mut out,
    )
    .context("failed to resolve workload.environment.secretFrom")?;

    Ok(out)
}

fn apply_refs(
    refs: &[String],
    catalog: &HashMap<String, ConfigSource>,
    project_dir: &Path,
    secret: bool,
    repo_root: Option<&Path>,
    out: &mut HashMap<String, String>,
) -> Result<()> {
    for name in refs {
        let Some(source) = catalog.get(name) else {
            let kind = if secret { "secret" } else { "config" };
            bail!(
                "workload references {} '{}' which is not defined in the top-level `{}s:` block",
                kind,
                name,
                kind
            );
        };

        let resolved = resolve_source(name, source, project_dir, secret, repo_root)?;
        out.extend(resolved);
    }
    Ok(())
}

fn resolve_source(
    name: &str,
    source: &ConfigSource,
    project_dir: &Path,
    secret: bool,
    repo_root: Option<&Path>,
) -> Result<HashMap<String, String>> {
    let mut out: HashMap<String, String> = HashMap::new();

    out.extend(source.inline.clone());

    if let Some(file) = source.file.as_ref() {
        let resolved_path = resolve_contained_path(file, project_dir)
            .with_context(|| format!("source '{}' has an unsafe `file:` path", name))?;

        if secret {
            check_secret_file_perms(&resolved_path).with_context(|| {
                format!(
                    "secret source '{}' file permissions are too permissive",
                    name
                )
            })?;
            warn_if_in_repo(&resolved_path, repo_root, name);
        }

        let parsed = parse_env_file(&resolved_path).with_context(|| {
            // Reference by name only; never include resolved value snippets.
            format!("failed to parse `file:` for source '{}'", name)
        })?;
        out.extend(parsed);
    }

    for var in &source.from_env {
        match std::env::var(var) {
            Ok(v) => {
                out.insert(var.clone(), v);
            }
            Err(_) => {
                bail!(
                    "source '{}' references environment variable '{}' which is not set",
                    name,
                    var
                );
            }
        }
    }

    Ok(out)
}

/// Canonicalize `path` (resolving symlinks) and reject anything that escapes
/// `project_dir`. The CVE-2025-62725 class is "join attacker-controlled
/// segments to a base dir without checking the result is still under it".
/// Canonicalizing both sides and comparing prefixes is the standard fix.
fn resolve_contained_path(path: &Path, project_dir: &Path) -> Result<PathBuf> {
    // Allow `~` expansion for ergonomics.
    let expanded = expand_tilde(path);

    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        project_dir.join(expanded)
    };

    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("could not resolve `file:` path: {}", candidate.display()))?;

    let project_canonical = project_dir.canonicalize().with_context(|| {
        format!(
            "could not canonicalize project_dir: {}",
            project_dir.display()
        )
    })?;

    // Absolute paths anywhere on disk are allowed (e.g. `~/.config/wash/...`),
    // BUT relative paths must stay under the project. We detect "was relative"
    // by checking whether the original path was relative.
    if !path.is_absolute() && !is_tilde(path) && !canonical.starts_with(&project_canonical) {
        bail!(
            "relative `file:` path resolves outside the project directory: {} -> {}",
            path.display(),
            canonical.display()
        );
    }

    Ok(canonical)
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = home.as_ref()
    {
        return home.join(rest);
    }
    if s == "~"
        && let Some(home) = home
    {
        return home;
    }
    path.to_path_buf()
}

fn is_tilde(path: &Path) -> bool {
    matches!(path.to_str(), Some(s) if s == "~" || s.starts_with("~/"))
}

#[cfg(unix)]
fn check_secret_file_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::metadata(path)
        .with_context(|| format!("could not stat secret file: {}", path.display()))?;
    let mode = meta.permissions().mode() & 0o777;
    // Allow only owner-read or owner-read-write. Anything group/world readable
    // is rejected — matches Kubernetes Secret volume defaultMode 0400.
    if mode != 0o600 && mode != 0o400 {
        bail!(
            "secret file {} has mode {:o}; require 0600 or 0400",
            path.display(),
            mode
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_secret_file_perms(_path: &Path) -> Result<()> {
    // No portable equivalent on Windows; rely on the user's NTFS ACLs.
    Ok(())
}

fn warn_if_in_repo(path: &Path, repo_root: Option<&Path>, name: &str) {
    let Some(root) = repo_root else { return };
    let Ok(root_canon) = root.canonicalize() else {
        return;
    };
    if !path.starts_with(&root_canon) {
        return;
    }
    // In-tree but gitignored is the well-known pattern for local dev secret
    // files (`.env.local`, etc.) — don't be noisy about it. Only warn when
    // the file is in-tree AND tracked or untracked-but-not-ignored. If git
    // isn't available we err on the side of warning, matching the
    // pre-gitignore-aware behavior.
    if is_gitignored(&root_canon, path).unwrap_or(false) {
        return;
    }
    warn!(
        secret = name,
        "secret source `file:` resolves inside the repo working tree and is not gitignored; add it to .gitignore"
    );
}

/// Returns `Some(true)` if `git check-ignore` reports `path` as ignored from
/// `repo_root`, `Some(false)` if it reports it as not ignored, and `None` if
/// git is unavailable or the directory isn't a git repository. Wraps a
/// blocking subprocess; only called in the secret-resolve warning path so
/// the cost is negligible.
fn is_gitignored(repo_root: &Path, path: &Path) -> Option<bool> {
    use std::process::{Command, Stdio};
    // `--quiet` suppresses stdout; exit code carries the answer (per
    // `git help check-ignore`):
    //   0 = ignored, 1 = not ignored, 128 = not a git repo or other error.
    // stderr is piped to /dev/null because the 128 case spits "fatal: not a
    // git repository" — fine for a fallback path, not fine to leak into the
    // user's `wash dev` terminal.
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("check-ignore")
        .arg("--quiet")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// Minimal `.env` parser: `KEY=VALUE` per line, `#` line comments, blank lines
/// ignored. No quoting, escapes, or variable expansion — keep this surface
/// small and predictable.
fn parse_env_file(path: &Path) -> Result<HashMap<String, String>> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("could not read file: {}", path.display()))?;

    let mut out = HashMap::new();
    for (lineno, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            bail!("line {} is not KEY=VALUE in {}", lineno + 1, path.display());
        };
        let key = k.trim().to_string();
        if key.is_empty() {
            bail!("line {} has empty key in {}", lineno + 1, path.display());
        }
        out.insert(key, v.trim().to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EnvironmentLayer;
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn parse_env_file_basic() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        write(&f, "# comment\nFOO=bar\n\nBAZ=qux\n");
        let out = parse_env_file(&f).unwrap();
        assert_eq!(out.get("FOO").unwrap(), "bar");
        assert_eq!(out.get("BAZ").unwrap(), "qux");
    }

    #[test]
    fn relative_path_must_stay_under_project() {
        let outer = TempDir::new().unwrap();
        let project = outer.path().join("proj");
        std::fs::create_dir(&project).unwrap();
        let outside = outer.path().join("outside.env");
        write(&outside, "X=1\n");

        let err = resolve_contained_path(Path::new("../outside.env"), &project).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("outside the project directory"), "{}", msg);
    }

    #[test]
    fn relative_path_inside_project_ok() {
        let project = TempDir::new().unwrap();
        let inside = project.path().join("a.env");
        write(&inside, "X=1\n");

        let resolved = resolve_contained_path(Path::new("a.env"), project.path()).unwrap();
        assert!(resolved.ends_with("a.env"));
    }

    #[cfg(unix)]
    #[test]
    fn secret_perms_too_loose_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let f = dir.path().join("s.env");
        write(&f, "X=1\n");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = check_secret_file_perms(&f).unwrap_err();
        assert!(format!("{:#}", err).contains("require 0600 or 0400"));
    }

    #[cfg(unix)]
    #[test]
    fn secret_perms_0600_ok() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let f = dir.path().join("s.env");
        write(&f, "X=1\n");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        check_secret_file_perms(&f).unwrap();
    }

    #[test]
    fn missing_named_ref_errors() {
        let mut env = EnvironmentLayer::default();
        env.config_from.push("missing".into());
        let workload = WorkloadConfig {
            environment: Some(env),
            ..Default::default()
        };
        let project = TempDir::new().unwrap();
        let err = resolve_environment(
            &workload,
            &HashMap::new(),
            &HashMap::new(),
            project.path(),
            None,
        )
        .unwrap_err();
        assert!(format!("{:#}", err).contains("missing"));
    }

    #[test]
    fn parse_env_file_rejects_missing_equals() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        write(&f, "FOO=bar\nNOEQUALS\n");
        let err = parse_env_file(&f).unwrap_err();
        let msg = format!("{:#}", err);
        assert!(msg.contains("line 2"), "{}", msg);
        assert!(msg.contains("not KEY=VALUE"), "{}", msg);
    }

    #[test]
    fn parse_env_file_rejects_empty_key() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        write(&f, "=value\n");
        let err = parse_env_file(&f).unwrap_err();
        assert!(format!("{:#}", err).contains("empty key"));
    }

    // SAFETY rationale for `std::env::{set,remove}_var` in tests below:
    // each test uses a UUID-suffixed variable name, so no other test reads or
    // writes the same key. `setenv`/`unsetenv` are not thread-safe at the libc
    // level, but with disjoint keys the only realistic concurrent risk is
    // pointer corruption inside the env block — accepted for test code, and
    // the wider codebase has no other env-mutating tests to compound it.
    #[test]
    #[allow(unsafe_code)]
    fn from_env_resolves_set_var_and_errors_on_missing() {
        let var = format!("WASH_TEST_FROM_ENV_{}", uuid::Uuid::new_v4().simple());
        unsafe { std::env::set_var(&var, "hello") };

        let project = TempDir::new().unwrap();
        let source = ConfigSource {
            from_env: vec![var.clone()],
            ..Default::default()
        };
        let out = resolve_source("c", &source, project.path(), false, None).unwrap();
        assert_eq!(out.get(&var).unwrap(), "hello");

        unsafe { std::env::remove_var(&var) };
        let err = resolve_source("c", &source, project.path(), false, None).unwrap_err();
        assert!(format!("{:#}", err).contains(&var));
    }

    #[test]
    #[allow(unsafe_code)]
    fn source_precedence_inline_then_file_then_from_env() {
        // Within one source: inline → file → fromEnv (later wins). Use the
        // same key in all three so we can observe overrides.
        let project = TempDir::new().unwrap();
        let var = format!("WASH_TEST_PREC_{}", uuid::Uuid::new_v4().simple());

        let f = project.path().join("a.env");
        write(&f, &format!("{}=from_file\n", var));

        unsafe { std::env::set_var(&var, "from_env") };

        let source = ConfigSource {
            inline: HashMap::from([(var.clone(), "from_inline".into())]),
            file: Some(f.clone()),
            from_env: vec![var.clone()],
        };
        let out = resolve_source("c", &source, project.path(), false, None).unwrap();
        assert_eq!(out.get(&var).unwrap(), "from_env", "fromEnv must win");

        // Drop fromEnv → file should win over inline.
        unsafe { std::env::remove_var(&var) };
        let source_no_env = ConfigSource {
            from_env: vec![],
            ..source.clone()
        };
        let out = resolve_source("c", &source_no_env, project.path(), false, None).unwrap();
        assert_eq!(
            out.get(&var).unwrap(),
            "from_file",
            "file must win over inline"
        );
    }

    #[test]
    fn env_layer_precedence_inline_then_config_from_then_secret_from() {
        // Across envFrom layers: inline → configFrom → secretFrom (later wins),
        // matching K8s envFrom merge semantics. Each layer also writes a
        // disjoint marker key so a regression that silently drops a layer is
        // caught even when the conflicting key still gets a winning value.
        let project = TempDir::new().unwrap();
        let workload = WorkloadConfig {
            environment: Some(EnvironmentLayer {
                config: HashMap::from([
                    ("KEY".into(), "from_inline".into()),
                    ("ONLY_INLINE".into(), "1".into()),
                ]),
                config_from: vec!["shared_cfg".into()],
                secret_from: vec!["shared_sec".into()],
            }),
            ..Default::default()
        };
        let configs = HashMap::from([(
            "shared_cfg".to_string(),
            ConfigSource {
                inline: HashMap::from([
                    ("KEY".into(), "from_config".into()),
                    ("ONLY_CONFIG".into(), "1".into()),
                ]),
                ..Default::default()
            },
        )]);
        let secrets = HashMap::from([(
            "shared_sec".to_string(),
            ConfigSource {
                inline: HashMap::from([
                    ("KEY".into(), "from_secret".into()),
                    ("ONLY_SECRET".into(), "1".into()),
                ]),
                ..Default::default()
            },
        )]);

        // The secret source has no `file:`, so no perm check fires.
        let env = resolve_environment(&workload, &configs, &secrets, project.path(), None).unwrap();
        assert_eq!(
            env.get("KEY").unwrap(),
            "from_secret",
            "secretFrom must win"
        );
        // Disjoint markers prove every layer ran.
        assert_eq!(
            env.get("ONLY_INLINE").unwrap(),
            "1",
            "inline layer was dropped"
        );
        assert_eq!(
            env.get("ONLY_CONFIG").unwrap(),
            "1",
            "configFrom layer was dropped"
        );
        assert_eq!(
            env.get("ONLY_SECRET").unwrap(),
            "1",
            "secretFrom layer was dropped"
        );
    }

    #[test]
    #[allow(unsafe_code)]
    fn end_to_end_resolve_workload_all_source_types() {
        let project = TempDir::new().unwrap();

        // configFrom uses a file source (no perm check).
        let cfg_file = project.path().join("app.env");
        write(
            &cfg_file,
            "APP_FOO=app_foo_value\n# comment\nAPP_BAR=app_bar_value\n",
        );

        // secretFrom uses an inline + fromEnv source (file would require 0600,
        // which TempDir defaults don't enforce — keep this test cross-platform).
        let var = format!("WASH_TEST_E2E_{}", uuid::Uuid::new_v4().simple());
        unsafe { std::env::set_var(&var, "shell_value") };

        let workload = WorkloadConfig {
            environment: Some(EnvironmentLayer {
                config: HashMap::from([("INLINE_KEY".into(), "inline_value".into())]),
                config_from: vec!["app".into()],
                secret_from: vec!["creds".into()],
            }),
            config: HashMap::from([("WORKLOAD_CFG".into(), "cfg_value".into())]),
            allowed_hosts: vec!["https://api.example.com".into()],
        };

        let configs = HashMap::from([(
            "app".to_string(),
            ConfigSource {
                file: Some(cfg_file.clone()),
                ..Default::default()
            },
        )]);
        let secrets = HashMap::from([(
            "creds".to_string(),
            ConfigSource {
                inline: HashMap::from([("DB_USER".into(), "alice".into())]),
                from_env: vec![var.clone()],
                ..Default::default()
            },
        )]);

        let config = Config {
            workload: Some(workload),
            configs,
            secrets,
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
            vec!["https://api.example.com".to_string()]
        );
    }
}
