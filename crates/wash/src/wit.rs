//! WIT dependency management for wash components
//!
//! This module provides functionality to fetch and manage WebAssembly Interface Type (WIT)
//! dependencies for wasmCloud components. It integrates with the wasm-pkg-client for
//! fetching dependencies from registries and manages lock files for reproducible builds.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use url::Url;
use wasm_pkg_client::{
    PackageRef, RegistryMapping,
    caching::{CachingClient, FileCache},
};
use wasm_pkg_core::{lock::LockFile, wit::OutputType};

/// The default name of the lock file for wasmCloud projects
pub const WKG_LOCK_FILE_NAME: &str = "wkg.lock";

/// Configuration for WIT dependency management
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WitConfig {
    /// Registries for WIT package fetching (default: wasm.pkg registry)
    #[serde(default = "default_wit_registries")]
    pub registries: Vec<WitRegistry>,
    /// Skip fetching WIT dependencies
    #[serde(default)]
    pub skip_fetch: bool,
    /// The directory where WIT files are stored, if not `./wit` in the project root
    #[serde(default)]
    pub wit_dir: Option<PathBuf>,
    /// Source overrides for WIT dependencies (target -> source mapping)
    #[serde(default)]
    pub sources: HashMap<String, String>,
}

impl WitConfig {
    pub fn validate(&self, project_dir: &Path) -> anyhow::Result<()> {
        let mut errors: Vec<String> = Vec::new();

        for reg in &self.registries {
            if let Err(err) = Url::parse(&reg.url) {
                errors.push(format!(
                    "wit.registries entry '{}' is not a valid URL: {}",
                    reg.url, err
                ));
            }
        }

        for (target, source) in &self.sources {
            if target.trim().is_empty() {
                errors.push("wit.sources contains an entry with empty target key".to_string());
            }

            if source.trim().is_empty() {
                errors.push(format!("wit.sources['{target}'] has an empty source value"));
                continue;
            }

            if let RegistryPullSource::LocalPath(_) = detect_source_type(source) {
                let resolved = project_dir.join(source);
                let exists = resolved.exists();

                if !exists {
                    errors.push(format!(
                        "wit.sources['{target}'] local path '{}' does not exist",
                        resolved.display()
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("{}", errors.join("\n"))
        }
    }
}

/// Default WIT registries (just the standard wasm.pkg registry)
fn default_wit_registries() -> Vec<WitRegistry> {
    // TODO(#1): bring BCA + wasmcloud here.
    vec![WitRegistry {
        url: "https://wasm.pkg".to_string(),
        token: None,
    }]
}

/// WIT registry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitRegistry {
    /// Registry URL
    pub url: String,

    /// Optional authentication token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Registry pull source types for WIT dependency overrides
#[derive(Debug, Clone)]
pub enum RegistryPullSource {
    /// Local filesystem path
    LocalPath(String),
    /// HTTP/HTTPS URL (tar.gz archives)
    RemoteHttp(String),
    /// Git repository URL
    RemoteGit(String),
    /// OCI registry reference
    RemoteOci(String),
}

impl TryFrom<RegistryPullSource> for RegistryMapping {
    type Error = anyhow::Error;

    fn try_from(source: RegistryPullSource) -> Result<Self, Self::Error> {
        match source {
            RegistryPullSource::RemoteOci(url) => Ok(RegistryMapping::Registry(url.parse()?)),
            _ => bail!("Cannot convert {:?} to RegistryMapping", source),
        }
    }
}

/// Wrapper around a `wasm_pkg_client::Client` including configuration for fetching WIT dependencies.
/// Primarily enables reuse of functionality to override dependencies and properly setup the client.
pub struct WkgFetcher {
    wkg_config: wasm_pkg_core::config::Config,
    wkg_client_config: wasm_pkg_client::Config,
    cache: FileCache,
}

/// Common arguments for Wasm package tooling.
#[derive(Debug, Clone, Default)]
pub struct CommonPackageArgs {
    /// The path to the [wasm_pkg_client::Config] configuration file.
    pub config: Option<PathBuf>,
    /// The path to the cache directory. Defaults to the wash cache directory.
    pub cache: Option<PathBuf>,
}

impl CommonPackageArgs {
    /// Helper to load the config from the given path or other default paths
    pub async fn load_config(&self) -> anyhow::Result<wasm_pkg_client::Config> {
        // Get the default config so we have the default fallbacks
        let mut conf = wasm_pkg_client::Config::default();

        // We attempt to load config in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to config file in wash dir
        // 5. Default path to config file from wkg
        match (self.config.as_ref(), std::env::var_os("WKG_CONFIG_FILE")) {
            // We have a config file provided by the user flag or WASH env var
            (Some(path), _) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // We have a config file provided by the user via `WKG` env var
            (None, Some(path)) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file from {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // Otherwise we got nothing and attempt to load the default config locations
            (None, None) => {
                // TODO(#1): support package_config.toml
            }
        };
        let wasmcloud_label = "wasmcloud"
            .parse()
            .context("failed to parse wasmcloud label")?;
        // If they don't have a config set for the wasmcloud namespace, set it to the default defined here
        if conf.namespace_registry(&wasmcloud_label).is_none() {
            conf.set_namespace_registry(
                wasmcloud_label,
                RegistryMapping::Registry(
                    "wasmcloud.com"
                        .parse()
                        .context("failed to parse wasmcloud registry")?,
                ),
            );
        }
        // Same for wrpc
        let wrpc_label = "wrpc".parse().context("failed to parse wrpc label")?;
        if conf.namespace_registry(&wrpc_label).is_none() {
            conf.set_namespace_registry(
                wrpc_label,
                RegistryMapping::Registry(
                    "bytecodealliance.org"
                        .parse()
                        .context("failed to parse wrpc registry")?,
                ),
            );
        }
        Ok(conf)
    }

    /// Helper for loading the [`FileCache`]
    pub async fn load_cache(&self) -> anyhow::Result<FileCache> {
        // We attempt to setup a cache dir in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to cache in wash dir
        let dir = match (self.cache.as_ref(), std::env::var_os("WKG_CACHE_DIR")) {
            // We have a cache dir provided by the user flag or WASH env var
            (Some(path), _) => path.to_owned(),
            // We have a cache dir provided by the user via `WKG` env var
            (None, Some(path)) => PathBuf::from(path),
            // Otherwise we got nothing and attempt to load the default cache dir
            // (None, None) => cfg_dir()?.join("package_cache"),
            _ => todo!("use common dir"),
        };
        FileCache::new(dir).await
    }
}

impl WkgFetcher {
    pub const fn new(
        wkg_config: wasm_pkg_core::config::Config,
        wkg_client_config: wasm_pkg_client::Config,
        cache: FileCache,
    ) -> Self {
        Self {
            wkg_config,
            wkg_client_config,
            cache,
        }
    }

    /// Load a `WkgFetcher` from a `CommonPackageArgs` and a `wasm_pkg_core::config::Config`
    pub async fn from_common(
        common: &CommonPackageArgs,
        wkg_config: wasm_pkg_core::config::Config,
    ) -> Result<Self> {
        let cache = common
            .load_cache()
            .await
            .context("failed to load wkg cache")?;
        let wkg_client_config = common
            .load_config()
            .await
            .context("failed to load wkg config")?;
        Ok(Self::new(wkg_config, wkg_client_config, cache))
    }

    /// Enable extended pull configurations for wkg config. Call before calling `fetch_wit_dependencies` to
    /// update configuration used.
    pub async fn resolve_extended_pull_configs(
        &mut self,
        sources: &HashMap<String, String>,
        project_dir: impl AsRef<Path>,
    ) -> Result<()> {
        let wkg_config_overrides = self.wkg_config.overrides.get_or_insert_default();

        for (target, source) in sources {
            let (ns, pkgs, maybe_version) = parse_wit_package_name(target)?;
            let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

            let registry_pull_source = detect_source_type(source);

            match registry_pull_source {
                RegistryPullSource::LocalPath(_) => {
                    let resolved_path = project_dir.as_ref().join(source);
                    if !tokio::fs::try_exists(&resolved_path)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to check for WIT source path [{}]",
                                resolved_path.display()
                            )
                        })?
                    {
                        bail!(
                            "WIT source path [{}] does not exist",
                            resolved_path.display()
                        );
                    }
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, resolved_path);
                }
                RegistryPullSource::RemoteHttp(_) => {
                    let wit_dir = download_and_extract_http(source)
                        .await
                        .with_context(|| format!("failed to download HTTP source [{}]", source))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteGit(_) => {
                    let wit_dir = clone_git_and_find_wit(source)
                        .await
                        .with_context(|| format!("failed to clone Git source [{}]", source))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteOci(_) => {
                    let registry = registry_pull_source.try_into()?;
                    match pkgs.as_slice() {
                        [] => {
                            // Namespace-level override
                            self.wkg_client_config.set_namespace_registry(
                                format!("{ns}{version_suffix}").try_into()?,
                                registry,
                            );
                        }
                        packages => {
                            // Package-level overrides
                            for pkg in packages {
                                self.wkg_client_config.set_package_registry_override(
                                    PackageRef::new(
                                        ns.clone().try_into()?,
                                        pkg.to_string().try_into()?,
                                    ),
                                    registry.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn fetch_wit_dependencies(
        self,
        wit_dir: impl AsRef<Path>,
        lock: &mut LockFile,
    ) -> Result<()> {
        let client = CachingClient::new(
            Some(wasm_pkg_client::Client::new(self.wkg_client_config)),
            self.cache,
        );

        wasm_pkg_core::wit::fetch_dependencies(
            &self.wkg_config,
            wit_dir.as_ref(),
            lock,
            client,
            OutputType::Wit,
        )
        .await?;

        Ok(())
    }

    /// Build a WIT package into a Wasm binary
    pub async fn build_wit_package(
        self,
        wit_dir: impl AsRef<Path>,
        lock: &mut LockFile,
    ) -> Result<(
        wasm_pkg_client::PackageRef,
        Option<semver::Version>,
        Vec<u8>,
    )> {
        let client = CachingClient::new(
            Some(wasm_pkg_client::Client::new(self.wkg_client_config)),
            self.cache,
        );

        wasm_pkg_core::wit::build_package(&self.wkg_config, wit_dir.as_ref(), lock, client).await
    }
}

/// Detect source type from string format
fn detect_source_type(source: &str) -> RegistryPullSource {
    if is_git_source(source) {
        RegistryPullSource::RemoteGit(source.to_string())
    } else if source.starts_with("http://") || source.starts_with("https://") {
        RegistryPullSource::RemoteHttp(source.to_string())
    } else if source.contains('/') && !source.starts_with('.') && !source.starts_with("file://") {
        // Likely OCI reference (contains slash but not relative path)
        RegistryPullSource::RemoteOci(source.to_string())
    } else {
        // Default to local path
        RegistryPullSource::LocalPath(source.to_string())
    }
}

/// Heuristic for detecting a Git source URL.
///
/// A bare `.contains(".git")` is too eager — it misclassifies OCI registries
/// whose hostname happens to include `.git`, such as `registry.gitlab.com`.
/// Treat a source as Git only when it uses a Git-specific scheme/form or its
/// resource path ends with `.git` (optionally followed by a `#<ref>` fragment).
fn is_git_source(source: &str) -> bool {
    if source.starts_with("git+") || source.starts_with("git@") {
        return true;
    }
    // Strip any `#<ref>` fragment and trailing slash, then look for a `.git` suffix.
    let without_fragment = source.split('#').next().unwrap_or(source);
    without_fragment.trim_end_matches('/').ends_with(".git")
}

/// Parse a WIT package name into namespace, packages, and version
/// Format: namespace:package@version or namespace@version
fn parse_wit_package_name(target: &str) -> Result<(String, Vec<String>, Option<String>)> {
    // Split on @ to separate version
    let (name_part, version) = if let Some((name, ver)) = target.rsplit_once('@') {
        (name, Some(ver.to_string()))
    } else {
        (target, None)
    };

    // Split on : to separate namespace and packages
    if let Some((namespace, packages_part)) = name_part.split_once(':') {
        let packages = if packages_part.is_empty() {
            vec![]
        } else {
            vec![packages_part.to_string()]
        };
        Ok((namespace.to_string(), packages, version))
    } else {
        // Just namespace, no packages
        Ok((name_part.to_string(), vec![], version))
    }
}

/// Set override for the target WIT package using the wkg config overrides map
fn set_override_for_target(
    overrides: &mut std::collections::HashMap<String, wasm_pkg_core::config::Override>,
    namespace: &str,
    packages: &[String],
    path: PathBuf,
) {
    use wasm_pkg_core::config::Override;

    if packages.is_empty() {
        // Namespace-level override: "namespace" = { path = "..." }
        overrides.insert(
            namespace.to_string(),
            Override {
                path: Some(path),
                version: None,
            },
        );
    } else {
        // Package-level override: "namespace:package" = { path = "..." }
        for package in packages {
            let key = format!("{}:{}", namespace, package);
            overrides.insert(
                key,
                Override {
                    path: Some(path.clone()),
                    version: None,
                },
            );
        }
    }
}

/// Download and extract HTTP tar.gz source
async fn download_and_extract_http(url: &str) -> Result<PathBuf> {
    let parsed_url = Url::parse(url).with_context(|| format!("invalid HTTP URL [{}]", url))?;

    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for downloading [{}]", url))?
        .keep();

    let output_path = tempdir.join("unpacked");

    // Use reqwest to download the archive with rustls-tls
    let client = reqwest::ClientBuilder::new()
        .use_rustls_tls()
        .build()
        .context("failed to build HTTP client")?;
    let response = client
        .get(parsed_url)
        .send()
        .await
        .with_context(|| format!("failed to download from URL [{}]", url))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read response from URL [{}]", url))?;

    // Extract tar.gz
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&output_path)
        .with_context(|| format!("failed to unpack archive from URL [{}]", url))?;

    find_wit_folder_in_path(&output_path).await
}

/// Clone git repository and find WIT directory
async fn clone_git_and_find_wit(url: &str) -> Result<PathBuf> {
    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for cloning [{}]", url))?
        .keep();

    // Parse git URL to handle git+ prefix
    let git_url = if let Some(stripped) = url.strip_prefix("git+") {
        stripped
    } else {
        url
    };

    debug!(
        "cloning git repository: {} to {}",
        git_url,
        tempdir.display()
    );

    // Use system git command to clone the repository
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["clone", git_url, tempdir.to_string_lossy().as_ref()]);

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute git clone command for [{}]", git_url))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Git clone failed for [{}]: {}", git_url, stderr);
    }

    debug!(url = git_url, "successfully cloned git repository");

    let wit_path = find_wit_folder_in_path(&tempdir).await?;
    debug!(path = %wit_path.display(), "found WIT path");
    Ok(wit_path)
}

/// Find the WIT directory in the given path, preferring root-level 'wit' directories
async fn find_wit_folder_in_path(search_path: &Path) -> Result<PathBuf> {
    use tracing::debug;

    // First check if there's a 'wit' directory at the root level
    let root_wit = search_path.join("wit");
    if root_wit.exists() && root_wit.is_dir() {
        debug!(path = %root_wit.display(), "found root-level WIT directory");
        return Ok(root_wit);
    }

    // If no root-level wit directory, search recursively
    find_wit_folder_in_path_internal(search_path, 0).await
}

/// Internal helper for find_wit_folder_in_path with depth limit to prevent infinite recursion
fn find_wit_folder_in_path_internal(
    search_path: &Path,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf>> + Send + '_>> {
    Box::pin(async move {
        if depth > 10 {
            // Prevent infinite recursion
            return Ok(search_path.to_path_buf());
        }

        let mut entries = tokio::fs::read_dir(search_path)
            .await
            .with_context(|| format!("failed to read directory [{}]", search_path.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("wit") {
                    return Ok(path);
                }

                // Recursively search subdirectories
                if let Ok(nested_wit) = find_wit_folder_in_path_internal(&path, depth + 1).await
                    && nested_wit != path
                {
                    return Ok(nested_wit);
                }
            }
        }

        // If no wit directory found, return the search path itself
        Ok(search_path.to_path_buf())
    })
}

/// Load a lock file from the project directory
///
/// The lock file is expected to be at `wkg.lock` in the project root, or `.wash/wasmcloud.lock` relative to the project directory.
#[instrument(skip_all)]
pub async fn load_lock_file(project_dir: impl AsRef<Path>) -> Result<LockFile> {
    let project_dir = project_dir.as_ref();

    let wkg_lock_file_path = project_dir.join(WKG_LOCK_FILE_NAME);

    let lock_file_path = if wkg_lock_file_path.exists() {
        // If the wkg.lock file exists, we prefer to use it
        debug!(path = %wkg_lock_file_path.display(), "found wkg.lock file, using it");
        Some(&wkg_lock_file_path)
    } else {
        None
    };

    if let Some(lock_file_path) = lock_file_path {
        debug!(lock_file_path = %lock_file_path.display(), "loading lock file ");
        // Check if the lock file is empty; if so, remove it and create a new lock file
        let metadata = tokio::fs::metadata(lock_file_path).await?;
        if metadata.len() == 0 {
            debug!("wkg.lock file is empty, removing and creating a new one");
            tokio::fs::remove_file(lock_file_path).await?;
            return LockFile::new_with_path([], lock_file_path)
                .await
                .context("failed to create new lock file after removing empty wkg.lock file");
        }
        LockFile::load_from_path(lock_file_path, false)
            .await
            .with_context(|| {
                format!(
                    "failed to load lock file: {lock_file_path}",
                    lock_file_path = lock_file_path.display()
                )
            })
    } else {
        debug!("lock file does not exist, will create wkg.lock at project root when saving");
        // Create a new empty lock file that will be written to wkg.lock at the project root
        LockFile::new_with_path([], &wkg_lock_file_path)
            .await
            .context("failed to create new lock file")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_detect_source_type() {
        assert!(matches!(
            detect_source_type("../shared-wit"),
            RegistryPullSource::LocalPath(_)
        ));
        assert!(matches!(
            detect_source_type("./local/path"),
            RegistryPullSource::LocalPath(_)
        ));
        assert!(matches!(
            detect_source_type("https://example.com/archive.tar.gz"),
            RegistryPullSource::RemoteHttp(_)
        ));
        assert!(matches!(
            detect_source_type("git+https://github.com/user/repo.git"),
            RegistryPullSource::RemoteGit(_)
        ));
        assert!(matches!(
            detect_source_type("git@github.com:user/repo.git"),
            RegistryPullSource::RemoteGit(_)
        ));
        assert!(matches!(
            detect_source_type("https://github.com/user/repo.git"),
            RegistryPullSource::RemoteGit(_)
        ));
        assert!(matches!(
            detect_source_type("https://github.com/user/repo.git#main"),
            RegistryPullSource::RemoteGit(_)
        ));
        assert!(matches!(
            detect_source_type("ghcr.io/user/package"),
            RegistryPullSource::RemoteOci(_)
        ));
        // Regression: OCI registries whose hostname contains `.git` must not be
        // misclassified as Git sources (see wasmCloud/wasmCloud#5194).
        assert!(matches!(
            detect_source_type("registry.gitlab.com/group/project/wasm-component:1.0.0"),
            RegistryPullSource::RemoteOci(_)
        ));
        assert!(matches!(
            detect_source_type("registry.gitlab.com/group/project/wasm-component"),
            RegistryPullSource::RemoteOci(_)
        ));
    }

    #[test]
    fn test_parse_wit_package_name() {
        let (ns, pkgs, ver) = parse_wit_package_name("wasmcloud:bus@1.0.0").unwrap();
        assert_eq!(ns, "wasmcloud");
        assert_eq!(pkgs, vec!["bus"]);
        assert_eq!(ver, Some("1.0.0".to_string()));

        let (ns, pkgs, ver) = parse_wit_package_name("wasi:config").unwrap();
        assert_eq!(ns, "wasi");
        assert_eq!(pkgs, vec!["config"]);
        assert_eq!(ver, None);

        let (ns, pkgs, ver) = parse_wit_package_name("wasmcloud@2.0.0").unwrap();
        assert_eq!(ns, "wasmcloud");
        assert_eq!(pkgs, Vec::<String>::new());
        assert_eq!(ver, Some("2.0.0".to_string()));
    }

    #[test]
    fn test_set_override_for_target() {
        let mut overrides = HashMap::new();
        let path = PathBuf::from("/tmp/test-wit");

        // Test namespace-level override
        set_override_for_target(&mut overrides, "wasmcloud", &[], path.clone());
        assert!(overrides.contains_key("wasmcloud"));
        assert_eq!(overrides["wasmcloud"].path, Some(path.clone()));

        // Test package-level override
        set_override_for_target(
            &mut overrides,
            "wasi",
            &["config".to_string()],
            path.clone(),
        );
        assert!(overrides.contains_key("wasi:config"));
        assert_eq!(overrides["wasi:config"].path, Some(path));
    }

    #[test]
    fn test_wit_config_deserialization() {
        let json = r#"
        {
            "sources": {
                "wasmcloud:bus": "../shared-wit",
                "wasi:config": "https://example.com/config.tar.gz"
            }
        }
        "#;

        let config: WitConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources["wasmcloud:bus"], "../shared-wit");
        assert_eq!(
            config.sources["wasi:config"],
            "https://example.com/config.tar.gz"
        );
    }

    fn empty_wit() -> WitConfig {
        WitConfig {
            registries: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn wit_default_with_no_sources_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(empty_wit().validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_invalid_registry_url_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = WitConfig {
            registries: vec![WitRegistry {
                url: "not a url".to_string(),
                token: None,
            }],
            ..Default::default()
        };
        let err = cfg.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("wit.registries"));
    }

    #[test]
    fn wit_valid_registry_url_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = WitConfig {
            registries: vec![WitRegistry {
                url: "https://wasm.pkg".to_string(),
                token: None,
            }],
            ..Default::default()
        };
        assert!(cfg.validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_empty_source_key_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources
            .insert("  ".to_string(), "./local/wit".to_string());
        let err = cfg.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("empty target key"));
    }

    #[test]
    fn wit_empty_source_value_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources
            .insert("wasi:http".to_string(), "  ".to_string());
        let err = cfg.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("empty source value"));
    }

    #[test]
    fn wit_local_path_missing_is_err() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources
            .insert("example:local".to_string(), "./does-not-exist".to_string());
        let err = cfg.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn wit_local_path_exists_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let wit_dir = tmp.path().join("my-wit");
        std::fs::create_dir_all(&wit_dir).unwrap();
        let mut cfg = empty_wit();
        cfg.sources
            .insert("example:local".to_string(), "my-wit".to_string());
        assert!(cfg.validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_http_source_not_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources.insert(
            "example:http".to_string(),
            "https://example.com/nonexistent.tar.gz".to_string(),
        );
        assert!(cfg.validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_git_source_not_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources.insert(
            "example:git".to_string(),
            "git+https://github.com/nonexistent/repo.git".to_string(),
        );
        assert!(cfg.validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_oci_source_not_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = empty_wit();
        cfg.sources.insert(
            "example:oci".to_string(),
            "ghcr.io/nonexistent/pkg".to_string(),
        );
        assert!(cfg.validate(tmp.path()).is_ok());
    }

    #[test]
    fn wit_multiple_errors_are_all_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = WitConfig {
            registries: vec![WitRegistry {
                url: "bad url".to_string(),
                token: None,
            }],
            ..Default::default()
        };
        cfg.sources
            .insert("wasi:http".to_string(), "./missing-path".to_string());
        let err = cfg.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("wit.registries"), "missing registry error");
        assert!(err.contains("does not exist"), "missing path error");
    }
}
