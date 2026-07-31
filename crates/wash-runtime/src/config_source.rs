//! The `configs:` / `secrets:` source model shared by every embedder.
//!
//! A host is handed flat key-value config; this module is where the layered
//! declaration of *where those values come from* is resolved into that flat
//! map. It lives in the runtime rather than in the `wash` CLI because the CLI
//! is not the only embedder: anything building a host (a Kubernetes control
//! plane, a custom supervisor) resolves the same `configFrom`/`secretFrom`
//! references against the same `dir:`/`file:`/`inline:`/`fromEnv:` sources,
//! and a second hand-written copy of these semantics is a copy that drifts.
//!
//! Configs and secrets carry distinct types ([`ConfigSource`],
//! [`SecretSource`]) so the stricter secret-handling posture can only be
//! applied to secrets:
//!
//! * **Path containment.** Every `file:`/`dir:` path is canonicalized and
//!   rejected if it escapes the project directory (defense against the
//!   `CVE-2025-62725` class, joining attacker-controlled path strings with a
//!   working directory without canonicalization).
//! * **Permission check (secrets only).** A secret `file:` source must be
//!   mode `0600` or `0400` on Unix. Looser modes are refused, matching the
//!   Kubernetes Secret volume `defaultMode: 0400` posture.
//! * **Repo-tree warning (secrets only).** When a resolved secret `file:`
//!   path sits inside the repo working tree we warn. This is the class of
//!   mistake behind the `.env` mass-exfiltration campaigns. Embedders that
//!   have no repo to speak of pass `repo_root: None` and skip it.
//! * **Logging hygiene.** Errors and traces reference sources and keys by
//!   name only; resolved values never appear in log output.
//! * **Memory only.** Resolved values are returned by value and never
//!   written back to disk.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

/// One layer of environment variables.
///
/// Inline values are written directly; `configFrom` / `secretFrom` reference
/// named entries in the top-level `configs:` / `secrets:` blocks by name. On
/// key conflicts later entries win, in order: inline → configFrom → secretFrom
/// (matches K8s `envFrom` semantics).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EnvironmentLayer {
    /// Inline plain values. Suitable for non-sensitive defaults.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[builder(default)]
    pub config: HashMap<String, String>,
    /// Names of entries in the top-level `configs:` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub config_from: Vec<String>,
    /// Names of entries in the top-level `secrets:` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub secret_from: Vec<String>,
}

/// A source of non-sensitive key-value pairs for a `configs:` entry.
///
/// Multiple fields can be set on a single entry. They merge last-wins in the
/// order `inline` → `file` → `fromEnv` (matches K8s ConfigMap merge
/// semantics). Resolution is [`ConfigSource::resolve`].
///
/// See [`SecretSource`] for the sibling type that carries the stricter
/// posture (file-mode check, in-repo-tree warning, etc.). The two share
/// today's wire schema but are deliberately distinct types so secret
/// handling can never be applied to a config and vice versa.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ConfigSource {
    /// Literal key-value entries supplied inline.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[builder(default)]
    pub inline: HashMap<String, String>,
    /// Path to a `.env`-format file. Relative paths resolve against the
    /// project directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Path to a directory of one-file-per-key entries — the shape a
    /// Kubernetes ConfigMap/Secret volume mount projects (filename = key,
    /// file content = value), unlike `file:`'s single `.env`-format blob.
    /// Entries whose name starts with `.` are skipped (kubelet's own
    /// `..data`/`..<timestamp>` bookkeeping). Relative paths resolve
    /// against the project directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// Names of environment variables to pull from the developer's shell.
    /// Each name is read at resolve time via [`std::env::var`]; a missing
    /// variable is a hard error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub from_env: Vec<String>,
}

/// A source of sensitive key-value pairs for a `secrets:` entry.
///
/// Same wire shape as [`ConfigSource`] today, but a distinct Rust type so
/// the stricter resolve-time posture (Unix file mode `0600`/`0400`,
/// `O_NOFOLLOW` open + `fstat` perm check, in-repo-tree warning, no value
/// snippets in error / log output) can only be applied here. Resolution
/// is [`SecretSource::resolve`].
///
/// The two types may diverge in the future (e.g. a future `rotation`
/// field that only makes sense for secrets) — keeping them separate now
/// avoids retrofitting the type split later.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SecretSource {
    /// Literal key-value entries supplied inline. Convenient for dev /
    /// test; do not commit production secrets this way.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[builder(default)]
    pub inline: HashMap<String, String>,
    /// Path to a `.env`-format file. Relative paths resolve against the
    /// project directory. The file must be Unix mode `0600` or `0400`
    /// and must not escape the project directory via `..` or symlink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Path to a directory of one-file-per-key entries — the shape a
    /// Kubernetes Secret volume mount projects (filename = key, file
    /// content = value). Unlike `file:`, entries here are read by
    /// following symlinks: a kubelet-projected volume is itself a
    /// symlink farm (`..data/<key>`, atomically swapped on rotation), a
    /// different trust boundary than a project-repo `file:` — the
    /// anti-symlink-race posture that path enforces doesn't apply to a
    /// kubelet-managed mount. Entries whose name starts with `.` are
    /// skipped. Relative paths resolve against the project directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// Names of environment variables to pull from the developer's shell.
    /// Each name is read at resolve time via [`std::env::var`]; a missing
    /// variable is a hard error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[builder(default)]
    pub from_env: Vec<String>,
}

/// Builds the flat env-var map for one [`EnvironmentLayer`] by walking
/// `environment.inline`, `environment.configFrom`, and
/// `environment.secretFrom` in K8s `envFrom` order (inline → configFrom →
/// secretFrom, later wins on key conflicts).
///
/// `owner` names the declaring scope (e.g. `"workload"` or
/// `"dev component 'hello'"`) in error messages.
pub fn resolve_environment_layer(
    env: Option<&EnvironmentLayer>,
    owner: &str,
    configs: &BTreeMap<String, ConfigSource>,
    secrets: &BTreeMap<String, SecretSource>,
    project_dir: &Path,
    repo_root: Option<&Path>,
) -> Result<HashMap<String, String>> {
    let Some(env) = env else {
        return Ok(HashMap::new());
    };

    let mut out: HashMap<String, String> = HashMap::new();

    // Layer order matches K8s envFrom: inline values first, then configFrom,
    // then secretFrom. Later layers overwrite earlier on key conflicts.
    out.extend(env.config.clone());

    apply_config_refs(&env.config_from, owner, configs, project_dir, &mut out)
        .with_context(|| format!("failed to resolve {owner} environment.configFrom"))?;
    apply_secret_refs(
        &env.secret_from,
        owner,
        secrets,
        project_dir,
        repo_root,
        &mut out,
    )
    .with_context(|| format!("failed to resolve {owner} environment.secretFrom"))?;

    Ok(out)
}

/// Looks up each named reference in `catalog`, resolves its
/// [`ConfigSource`], and merges the result into `out`. Missing names are
/// a hard error so typos in `configFrom` fail loudly.
fn apply_config_refs(
    refs: &[String],
    owner: &str,
    catalog: &BTreeMap<String, ConfigSource>,
    project_dir: &Path,
    out: &mut HashMap<String, String>,
) -> Result<()> {
    for name in refs {
        let Some(source) = catalog.get(name) else {
            bail!(
                "{owner} references config '{name}' which is not defined in the top-level `configs:` block",
            );
        };
        let resolved = source.resolve(name, project_dir)?;
        out.extend(resolved);
    }
    Ok(())
}

/// Looks up each named reference in `catalog`, resolves its
/// [`SecretSource`], and merges the result into `out`. Missing names are
/// a hard error so typos in `secretFrom` fail loudly.
fn apply_secret_refs(
    refs: &[String],
    owner: &str,
    catalog: &BTreeMap<String, SecretSource>,
    project_dir: &Path,
    repo_root: Option<&Path>,
    out: &mut HashMap<String, String>,
) -> Result<()> {
    for name in refs {
        let Some(source) = catalog.get(name) else {
            bail!(
                "{owner} references secret '{name}' which is not defined in the top-level `secrets:` block",
            );
        };
        let resolved = source.resolve(name, project_dir, repo_root)?;
        out.extend(resolved);
    }
    Ok(())
}

impl ConfigSource {
    /// Resolves this config source into a flat key-value map.
    ///
    /// Layers merge last-wins in the order `inline` → `file` → `fromEnv`
    /// (matches K8s ConfigMap merge semantics). `name` is used only in
    /// error / log messages — value snippets never appear in either.
    ///
    /// # Errors
    ///
    /// Returns an error if the `file:` path is unsafe (escapes the
    /// project directory) or unreadable, the file fails to parse as
    /// `.env`, or a `fromEnv` entry is missing from the process
    /// environment.
    pub fn resolve(&self, name: &str, project_dir: &Path) -> Result<HashMap<String, String>> {
        let mut out: HashMap<String, String> = HashMap::new();
        out.extend(self.inline.clone());

        if let Some(file) = self.file.as_ref() {
            let resolved_path = resolve_contained_path(file, project_dir)
                .with_context(|| format!("config source '{name}' has an unsafe `file:` path"))?;
            // `O_NOFOLLOW` on the open hardens against a symlink-swap race
            // between our `canonicalize` and the read — useful even for
            // non-secret config.
            let handle = open_file_secure(&resolved_path)
                .with_context(|| format!("could not open `file:` for config source '{name}'"))?;
            let parsed = parse_env_file(handle, &resolved_path)
                .with_context(|| format!("failed to parse `file:` for config source '{name}'"))?;
            out.extend(parsed);
        }

        if let Some(dir) = self.dir.as_ref() {
            let parsed = resolve_dir(dir, project_dir)
                .with_context(|| format!("failed to read `dir:` for config source '{name}'"))?;
            out.extend(parsed);
        }

        extend_from_env(&self.from_env, "config source", name, &mut out)?;
        Ok(out)
    }
}

impl SecretSource {
    /// Resolves this secret source into a flat key-value map under the
    /// stricter secret-handling posture.
    ///
    /// Layer order matches [`ConfigSource::resolve`] (inline → file →
    /// fromEnv, last-wins). In addition, when a `file:` is set the
    /// resolved path is:
    ///
    /// - opened with `O_NOFOLLOW` so a symlink swap between
    ///   canonicalize and open is rejected,
    /// - mode-checked via `fstat` on the open fd (must be `0600` or
    ///   `0400` on Unix), and
    /// - warned-about if it sits inside the repo working tree and is not
    ///   gitignored.
    ///
    /// `name` is used only in error / log messages — value snippets
    /// never appear in either.
    ///
    /// # Errors
    ///
    /// Returns an error if any check above fails, if the path is unsafe,
    /// if the file fails to parse, or if a `fromEnv` entry is missing
    /// from the process environment.
    pub fn resolve(
        &self,
        name: &str,
        project_dir: &Path,
        repo_root: Option<&Path>,
    ) -> Result<HashMap<String, String>> {
        let mut out: HashMap<String, String> = HashMap::new();
        out.extend(self.inline.clone());

        if let Some(file) = self.file.as_ref() {
            let resolved_path = resolve_contained_path(file, project_dir)
                .with_context(|| format!("secret source '{name}' has an unsafe `file:` path"))?;
            // TOCTOU-safe sequence: open once (`O_NOFOLLOW`), `fstat` the
            // open fd for perms, then read from that same fd. Closes both
            // the canonicalize↔open race (symlink swap) and the
            // perm-check↔read race (mode swap).
            let handle = open_file_secure(&resolved_path)
                .with_context(|| format!("could not open `file:` for secret source '{name}'"))?;
            check_secret_file_perms(&handle, &resolved_path).with_context(|| {
                format!("secret source '{name}' file permissions are too permissive")
            })?;
            warn_if_in_repo(&resolved_path, repo_root, name);
            let parsed = parse_env_file(handle, &resolved_path)
                .with_context(|| format!("failed to parse `file:` for secret source '{name}'"))?;
            out.extend(parsed);
        }

        if let Some(dir) = self.dir.as_ref() {
            let parsed = resolve_dir(dir, project_dir)
                .with_context(|| format!("failed to read `dir:` for secret source '{name}'"))?;
            out.extend(parsed);
        }

        extend_from_env(&self.from_env, "secret source", name, &mut out)?;
        Ok(out)
    }
}

/// Reads every regular file directly inside `dir` as one key-value entry
/// (filename = key, UTF-8 file content = value) — the shape a Kubernetes
/// ConfigMap/Secret volume mount projects. Entries whose name starts with
/// `.` are skipped (kubelet's own `..data`/`..<timestamp>` bookkeeping, and
/// the conventional way to "hide" an entry from a directory listing).
/// Symlinks are followed deliberately: a kubelet-projected volume is itself
/// a symlink farm, atomically swapped on rotation.
fn resolve_dir(dir: &Path, project_dir: &Path) -> Result<HashMap<String, String>> {
    let resolved_dir = resolve_contained_path(dir, project_dir).context("unsafe `dir:` path")?;
    let mut out = HashMap::new();
    let entries = std::fs::read_dir(&resolved_dir)
        .with_context(|| format!("could not read directory {}", resolved_dir.display()))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("could not read directory {}", resolved_dir.display()))?;
        let file_name = entry.file_name();
        let Some(key) = file_name.to_str() else {
            bail!(
                "entry {} in {} is not valid UTF-8",
                file_name.to_string_lossy(),
                resolved_dir.display()
            );
        };
        if key.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Resolve the symlink and require the real target to stay under
        // `resolved_dir` itself. A kubelet-projected volume's symlink farm
        // (`key -> ..data/<timestamp>/key`) always resolves within its own
        // mount root, so this passes; a `dir:` entry symlinked to point
        // somewhere else on disk (e.g. `~/.ssh/id_rsa`). The same
        // CVE-2025-62725-class risk `file:`'s containment check exists to
        // stop is rejected instead of silently read.
        let canonical = path
            .canonicalize()
            .with_context(|| format!("could not resolve {}", path.display()))?;
        if !canonical.starts_with(&resolved_dir) {
            bail!(
                "entry {} in `dir:` {} resolves outside the mounted directory ({})",
                key,
                resolved_dir.display(),
                canonical.display()
            );
        }
        // `fs::metadata` (not `DirEntry::metadata`, which behaves like
        // `lstat` and would see every kubelet-projected entry as a
        // symlink) follows the symlink to the real backing file.
        let is_file = std::fs::metadata(&canonical)
            .with_context(|| format!("could not stat {}", canonical.display()))?
            .is_file();
        if !is_file {
            continue;
        }
        let value = std::fs::read_to_string(&canonical)
            .with_context(|| format!("could not read {}", canonical.display()))?;
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

/// Reads each named env var and inserts it into `out`. `kind` is the
/// label used in error messages (e.g. `"config source"` /
/// `"secret source"`).
fn extend_from_env(
    vars: &[String],
    kind: &str,
    name: &str,
    out: &mut HashMap<String, String>,
) -> Result<()> {
    for var in vars {
        match std::env::var(var) {
            Ok(v) => {
                out.insert(var.clone(), v);
            }
            Err(_) => {
                bail!("{kind} '{name}' references environment variable '{var}' which is not set");
            }
        }
    }
    Ok(())
}

/// Canonicalizes `path` (resolving symlinks) and rejects anything that
/// escapes `project_dir`.
///
/// The CVE-2025-62725 class is "join attacker-controlled segments to a
/// base dir without checking the result is still under it".
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

/// Expands a leading `~` or `~/` in `path` to the user's home directory.
///
/// Returns `path` unchanged when it doesn't start with `~`, when the path
/// isn't valid UTF-8, or when the home directory can't be determined.
fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    // `etcetera::home_dir` wraps the same `home_dir` crate the rest of wash
    // uses for XDG resolution. Don't fall back to `$HOME`: an absent home
    // directory means "no expansion" rather than constructing a path that
    // can't possibly be right.
    let home = etcetera::home_dir().ok();
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

/// Returns `true` when `path` is exactly `~` or starts with `~/`.
fn is_tilde(path: &Path) -> bool {
    matches!(path.to_str(), Some(s) if s == "~" || s.starts_with("~/"))
}

/// Opens `path` read-only with symlink-swap protection.
///
/// On Unix, `O_NOFOLLOW` ensures the final path component is not a
/// symlink: if `path` was swapped to a symlink between our `canonicalize`
/// check and this open, the call fails (`ELOOP`) instead of following the
/// new target. The canonicalize step already resolved any earlier
/// symlinks, so `O_NOFOLLOW` here is the last hop's guarantee.
///
/// On non-Unix platforms there's no portable equivalent — falls back to a
/// plain open and relies on the host's NTFS ACLs (the same posture as
/// `check_secret_file_perms` for Windows).
#[cfg(unix)]
fn open_file_secure(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
}

#[cfg(not(unix))]
fn open_file_secure(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// Rejects a secret file unless its mode is `0600` or `0400`.
///
/// Performs the check via `fstat` on the open descriptor (Unix), which is
/// race-free against symlink / path swaps between this check and the
/// subsequent read. On Windows there's no portable equivalent and the
/// check is a no-op — callers fall back to NTFS ACLs.
#[cfg(unix)]
fn check_secret_file_perms(file: &File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // `File::metadata` is `fstat` on Unix — it operates on the open
    // descriptor, not the path, so it can't be raced by a symlink swap
    // between check and read.
    let meta = file
        .metadata()
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
fn check_secret_file_perms(_file: &File, path: &Path) -> Result<()> {
    // No portable equivalent on Windows — rely on the user's NTFS ACLs.
    // Emit a one-shot `warn!` so the developer knows their secrets aren't
    // mode-checked here; per-call warnings would spam workloads with
    // several secrets without adding signal.
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        warn!(
            ?path,
            "secret file permission checks are not enforced on this platform; \
             relying on the host filesystem ACLs"
        );
    }
    Ok(())
}

/// Emits a `warn!` log when `path` resolves inside the repo working tree
/// and is not gitignored.
///
/// This catches the common `.env`-in-repo footgun before a developer
/// accidentally commits secrets. Gitignored files are skipped (that's the
/// well-known `.env.local` dev pattern). If `repo_root` is unknown or git
/// is unavailable, errs on the side of warning.
fn warn_if_in_repo(path: &Path, repo_root: Option<&Path>, name: &str) {
    let Some(root) = repo_root else {
        // Caller didn't supply a repo root, so we can't decide. Trace-only so
        // it doesn't drown the dev terminal but is still findable.
        trace!(secret = name, "skipping in-repo check: repo_root unknown");
        return;
    };
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
    if is_path_gitignored(&root_canon, path) {
        return;
    }
    warn!(
        secret = name,
        "secret source `file:` resolves inside the repo working tree and is not gitignored; add it to .gitignore"
    );
}

/// Returns `true` only when `git check-ignore` confidently reports `path` as
/// ignored from `repo_root`. Any other outcome — not ignored, git missing,
/// not a git repo, command failure — returns `false`. The caller treats
/// "we can't be sure" the same as "definitely not ignored" so a developer
/// who has e.g. uninstalled git still gets the warning.
fn is_path_gitignored(repo_root: &Path, path: &Path) -> bool {
    use std::process::{Command, Stdio};
    // `--quiet` suppresses stdout; exit code carries the answer (per
    // `git help check-ignore`):
    //   0 = ignored, 1 = not ignored, 128 = not a git repo or other error.
    // stderr is piped to /dev/null because the 128 case spits "fatal: not a
    // git repository" — fine for a fallback path, not fine to leak into the
    // user's `wash dev` terminal.
    let Ok(status) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("check-ignore")
        .arg("--quiet")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    else {
        return false;
    };
    status.code() == Some(0)
}

/// Minimal `.env` parser: `KEY=VALUE` per line, `#` line comments, blank lines
/// ignored. No quoting, escapes, or variable expansion — keep this surface
/// small and predictable.
///
/// Takes an already-open [`File`] so the caller can keep one descriptor for
/// the lifetime of the perm-check-then-read sequence (eliminates the TOCTOU
/// race between a path-based stat and a path-based open). Streams via
/// `BufReader` so a large `.env` file never lands in memory whole. `path`
/// is used only for error messages.
fn parse_env_file(file: File, path: &Path) -> Result<HashMap<String, String>> {
    let reader = BufReader::new(file);

    let mut out = HashMap::new();
    for (lineno, line) in reader.lines().enumerate() {
        let raw = line.with_context(|| format!("could not read file: {}", path.display()))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
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
    use tempfile::TempDir;

    #[test]
    fn parse_env_file_basic() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        std::fs::write(&f, "# comment\nFOO=bar\n\nBAZ=qux\n").unwrap();
        let out = parse_env_file(open_file_secure(&f).unwrap(), &f).unwrap();
        assert_eq!(out.get("FOO").unwrap(), "bar");
        assert_eq!(out.get("BAZ").unwrap(), "qux");
    }

    #[test]
    fn relative_path_must_stay_under_project() {
        let outer = TempDir::new().unwrap();
        let project = outer.path().join("proj");
        std::fs::create_dir(&project).unwrap();
        let outside = outer.path().join("outside.env");
        std::fs::write(&outside, "X=1\n").unwrap();

        let err = resolve_contained_path(Path::new("../outside.env"), &project).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("outside the project directory"), "{}", msg);
    }

    #[test]
    fn relative_path_inside_project_ok() {
        let project = TempDir::new().unwrap();
        let inside = project.path().join("a.env");
        std::fs::write(&inside, "X=1\n").unwrap();

        let resolved = resolve_contained_path(Path::new("a.env"), project.path()).unwrap();
        assert!(resolved.ends_with("a.env"));
    }

    #[cfg(unix)]
    #[test]
    fn secret_perms_too_loose_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let f = dir.path().join("s.env");
        std::fs::write(&f, "X=1\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let handle = open_file_secure(&f).unwrap();
        let err = check_secret_file_perms(&handle, &f).unwrap_err();
        assert!(format!("{err:#}").contains("require 0600 or 0400"));
    }

    #[cfg(unix)]
    #[test]
    fn secret_perms_0600_ok() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let f = dir.path().join("s.env");
        std::fs::write(&f, "X=1\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        let handle = open_file_secure(&f).unwrap();
        check_secret_file_perms(&handle, &f).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn open_file_secure_rejects_symlink_to_secret() {
        // `O_NOFOLLOW` is the last-hop guarantee for the canonicalize↔open
        // race: if the final path component is a symlink, the open fails.
        // We don't follow symlinks even if the target would have passed
        // every prior check (mode, in-repo).
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.env");
        std::fs::write(&real, "X=1\n").unwrap();
        let link = dir.path().join("link.env");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err = open_file_secure(&link).expect_err("symlink open should be rejected");
        assert_eq!(
            err.raw_os_error(),
            Some(rustix::io::Errno::LOOP.raw_os_error()),
            "expected ELOOP from O_NOFOLLOW, got {err:?}"
        );
    }

    #[test]
    fn missing_named_ref_errors() {
        let mut env = EnvironmentLayer::default();
        env.config_from.push("missing".into());
        let project = TempDir::new().unwrap();
        let err = resolve_environment_layer(
            Some(&env),
            "workload",
            &BTreeMap::new(),
            &BTreeMap::new(),
            project.path(),
            None,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("missing"));
    }

    #[test]
    fn parse_env_file_rejects_missing_equals() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        std::fs::write(&f, "FOO=bar\nNOEQUALS\n").unwrap();
        let err = parse_env_file(open_file_secure(&f).unwrap(), &f).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("line 2"), "{}", msg);
        assert!(msg.contains("not KEY=VALUE"), "{}", msg);
    }

    #[test]
    fn parse_env_file_rejects_empty_key() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.env");
        std::fs::write(&f, "=value\n").unwrap();
        let err = parse_env_file(open_file_secure(&f).unwrap(), &f).unwrap_err();
        assert!(format!("{err:#}").contains("empty key"));
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
        let out = source.resolve("c", project.path()).unwrap();
        assert_eq!(out.get(&var).unwrap(), "hello");

        unsafe { std::env::remove_var(&var) };
        let err = source.resolve("c", project.path()).unwrap_err();
        assert!(format!("{err:#}").contains(&var));
    }

    #[test]
    #[allow(unsafe_code)]
    fn source_precedence_inline_then_file_then_from_env() {
        // Within one source: inline → file → fromEnv (later wins). Use the
        // same key in all three so we can observe overrides.
        let project = TempDir::new().unwrap();
        let var = format!("WASH_TEST_PREC_{}", uuid::Uuid::new_v4().simple());

        let f = project.path().join("a.env");
        std::fs::write(&f, format!("{var}=from_file\n")).unwrap();

        unsafe { std::env::set_var(&var, "from_env") };

        let source = ConfigSource {
            inline: HashMap::from([(var.clone(), "from_inline".into())]),
            file: Some(f.clone()),
            dir: None,
            from_env: vec![var.clone()],
        };
        let out = source.resolve("c", project.path()).unwrap();
        assert_eq!(out.get(&var).unwrap(), "from_env", "fromEnv must win");

        // Drop fromEnv → file should win over inline.
        unsafe { std::env::remove_var(&var) };
        let source_no_env = ConfigSource {
            from_env: vec![],
            ..source.clone()
        };
        let out = source_no_env.resolve("c", project.path()).unwrap();
        assert_eq!(
            out.get(&var).unwrap(),
            "from_file",
            "file must win over inline"
        );
    }

    #[test]
    fn dir_source_reads_one_file_per_key_like_a_k8s_volume_mount() {
        // A Kubernetes ConfigMap/Secret volume mount projects one regular
        // file per key (in practice via a symlink farm); `dir:` reads that
        // shape directly — filename is the key, content is the value.
        let project = TempDir::new().unwrap();
        let mount = project.path().join("mounted");
        std::fs::create_dir(&mount).unwrap();
        std::fs::write(mount.join("etcd-prefix"), "/wasmcloud/secrets").unwrap();
        std::fs::write(mount.join("api-key"), "s3cr3t-value").unwrap();
        // Kubelet bookkeeping entries (`..data`, `..2024_01_01.../`) must be
        // ignored, not read as keys.
        std::fs::create_dir(mount.join("..2024_01_01_00_00_00.123456789")).unwrap();
        std::fs::write(
            mount.join("..2024_01_01_00_00_00.123456789/api-key"),
            "stale",
        )
        .unwrap();

        let source = ConfigSource {
            dir: Some(mount),
            ..Default::default()
        };
        let out = source.resolve("mounted", project.path()).unwrap();
        assert_eq!(out.len(), 2, "hidden bookkeeping entries must be skipped");
        assert_eq!(out.get("etcd-prefix").unwrap(), "/wasmcloud/secrets");
        assert_eq!(out.get("api-key").unwrap(), "s3cr3t-value");
    }

    #[cfg(unix)]
    #[test]
    fn dir_source_follows_symlinks_like_a_projected_volume() {
        // The real K8s shape: the top-level entry is a symlink into a
        // timestamped target directory, not a regular file directly.
        use std::os::unix::fs::symlink;

        let project = TempDir::new().unwrap();
        let mount = project.path().join("mounted");
        // The timestamped target lives *inside* the mount root, same as a
        // real kubelet-projected volume, not beside it.
        let target_dir = mount.join("..2024_01_01_00_00_00.123456789");
        std::fs::create_dir(&mount).unwrap();
        std::fs::create_dir(&target_dir).unwrap();
        std::fs::write(target_dir.join("api-key"), "s3cr3t-value").unwrap();
        symlink(target_dir.join("api-key"), mount.join("api-key")).unwrap();

        let source = SecretSource {
            dir: Some(mount),
            ..Default::default()
        };
        let out = source.resolve("mounted", project.path(), None).unwrap();
        assert_eq!(out.get("api-key").unwrap(), "s3cr3t-value");
    }

    #[test]
    fn dir_source_rejects_unsafe_path() {
        let project = TempDir::new().unwrap();
        let source = ConfigSource {
            dir: Some(PathBuf::from("../../etc")),
            ..Default::default()
        };
        assert!(source.resolve("escape", project.path()).is_err());
    }

    #[test]
    fn env_layer_precedence_inline_then_config_from_then_secret_from() {
        // Across envFrom layers: inline → configFrom → secretFrom (later wins),
        // matching K8s envFrom merge semantics. Each layer also writes a
        // disjoint marker key so a regression that silently drops a layer is
        // caught even when the conflicting key still gets a winning value.
        let project = TempDir::new().unwrap();
        let env = EnvironmentLayer {
            config: HashMap::from([
                ("KEY".into(), "from_inline".into()),
                ("ONLY_INLINE".into(), "1".into()),
            ]),
            config_from: vec!["shared_cfg".into()],
            secret_from: vec!["shared_sec".into()],
        };
        let configs = BTreeMap::from([(
            "shared_cfg".to_string(),
            ConfigSource {
                inline: HashMap::from([
                    ("KEY".into(), "from_config".into()),
                    ("ONLY_CONFIG".into(), "1".into()),
                ]),
                ..Default::default()
            },
        )]);
        let secrets = BTreeMap::from([(
            "shared_sec".to_string(),
            SecretSource {
                inline: HashMap::from([
                    ("KEY".into(), "from_secret".into()),
                    ("ONLY_SECRET".into(), "1".into()),
                ]),
                ..Default::default()
            },
        )]);

        // The secret source has no `file:`, so no perm check fires.
        let env = resolve_environment_layer(
            Some(&env),
            "workload",
            &configs,
            &secrets,
            project.path(),
            None,
        )
        .unwrap();
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
}
