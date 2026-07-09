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
    CustomConfig, PackageRef, Registry, RegistryMapping, RegistryMetadata,
    caching::{CachingClient, FileCache},
    oci::{BasicCredentials, OciRegistryConfig},
};
use wasm_pkg_core::{lock::LockFile, wit::OutputType};

/// The default name of the lock file for wasmCloud projects
pub const WKG_LOCK_FILE_NAME: &str = "wkg.lock";

/// Configuration for WIT dependency management
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WitConfig {
    /// Authentication for OCI registries that host WIT packages. Namespace-to-registry mapping
    /// is configured via [`WitConfig::sources`] or the wkg config; these entries only supply
    /// credentials for a registry host.
    #[serde(default)]
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
            if let Err(err) = reg.registry() {
                errors.push(format!(
                    "wit.registries entry '{}' is not a valid registry host: {err}",
                    reg.url
                ));
            }
            if let Err(err) = reg.credentials() {
                errors.push(err.to_string());
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

/// Authentication for an OCI registry host that serves WIT packages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitRegistry {
    /// Registry host this applies to, e.g. `ghcr.io` (a full `https://ghcr.io` URL is also
    /// accepted; the host is extracted from it).
    pub url: String,

    /// Username for basic auth. Required alongside `token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Password or personal access token for basic auth. Required alongside `username`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl WitRegistry {
    /// The registry host this entry configures.
    fn registry(&self) -> Result<Registry> {
        registry_authority(&self.url)
    }

    /// Basic-auth credentials for this entry, or `None` when unset. Errors when exactly one of
    /// `username`/`token` is set, since basic auth requires both.
    fn credentials(&self) -> Result<Option<(&str, &str)>> {
        match (self.username.as_deref(), self.token.as_deref()) {
            (Some(username), Some(token)) => Ok(Some((username, token))),
            (None, None) => Ok(None),
            _ => bail!(
                "wit.registries entry '{}' must set both username and token, or neither",
                self.url
            ),
        }
    }
}

/// Extract a wkg [`Registry`] (host`[:port]`) from a `wit.registries` `url`, accepting both a
/// bare authority (`ghcr.io`) and a full URL (`https://ghcr.io`).
fn registry_authority(url: &str) -> Result<Registry> {
    if let Ok(parsed) = Url::parse(url)
        && let Some(host) = parsed.host_str()
    {
        let authority = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        return authority
            .parse()
            .with_context(|| format!("invalid registry host [{url}]"));
    }
    url.parse()
        .with_context(|| format!("invalid registry host [{url}]"))
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

/// Build a wkg registry mapping from an OCI source reference for the given WIT target.
///
/// wasm-pkg reconstructs an OCI repository as `{namespace_prefix}{namespace}/{name}`, so
/// only the reference's host is a valid [`Registry`]. Any repository path leading up to the
/// conventional `{namespace}/{package}` (or `{namespace}` for a namespace-level override)
/// suffix becomes the OCI namespace prefix, expressed as a [`RegistryMapping::Custom`].
fn oci_registry_mapping(
    reference: &str,
    namespace: &str,
    package: Option<&str>,
) -> Result<RegistryMapping> {
    let reference = reference.strip_prefix("oci://").unwrap_or(reference);

    let (authority, repository) = match reference.split_once('/') {
        Some((authority, repository)) => (authority, Some(repository)),
        None => (reference, None),
    };

    let registry: Registry = authority.parse().with_context(|| {
        format!("invalid OCI registry host [{authority}] in source [{reference}]")
    })?;

    let Some(repository) = repository else {
        return Ok(RegistryMapping::Registry(registry));
    };

    // Drop any `:tag`; the tag is derived from the package version at fetch time.
    let repository = repository
        .rsplit_once(':')
        .map_or(repository, |(repo, _tag)| repo)
        .trim_end_matches('/');

    let suffix = match package {
        Some(package) => format!("{namespace}/{package}"),
        None => namespace.to_string(),
    };

    let namespace_prefix = if repository == suffix {
        None
    } else if let Some(prefix) = repository.strip_suffix(&format!("/{suffix}")) {
        Some(format!("{prefix}/"))
    } else {
        bail!(
            "OCI source [{reference}] does not resolve to the expected repository suffix \
             [{suffix}]; the WIT namespace must appear in the OCI repository path"
        );
    };

    let Some(namespace_prefix) = namespace_prefix else {
        return Ok(RegistryMapping::Registry(registry));
    };

    let mut oci_config = serde_json::Map::new();
    oci_config.insert("registry".to_string(), authority.into());
    oci_config.insert("namespacePrefix".to_string(), namespace_prefix.into());

    let mut metadata = RegistryMetadata::default();
    metadata.preferred_protocol = Some("oci".to_string());
    metadata
        .protocol_configs
        .insert("oci".to_string(), oci_config);

    Ok(RegistryMapping::Custom(CustomConfig { registry, metadata }))
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
            // Otherwise fall back to the user's global wkg config (e.g.
            // `~/.config/wasm-pkg/config.toml`), matching `wkg`'s own resolution so that
            // namespace registries and registry auth configured there are honored without
            // requiring `WKG_CONFIG_FILE`.
            (None, None) => {
                if let Some(loaded) = wasm_pkg_client::Config::read_global_config()
                    .await
                    .context("failed to read global wkg config")?
                {
                    conf.merge(loaded);
                }
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

    /// Build a fetcher for a project: reads the project's `wkg.toml` overrides and the resolved
    /// wkg client config, caching packages under `cache_dir`.
    pub async fn for_project(cache_dir: PathBuf, project_dir: impl AsRef<Path>) -> Result<Self> {
        let args = CommonPackageArgs {
            config: None,
            cache: Some(cache_dir),
        };
        let wkg_config = load_wkg_config(project_dir).await?;
        Self::from_common(&args, wkg_config).await
    }

    /// Apply a project's `[wit]` configuration to this fetcher: registry authentication (which
    /// configures credentials per registry host) and source overrides (which map WIT packages to
    /// registries or local paths). The two are independent and touch different config.
    pub async fn apply_wit_config(
        &mut self,
        wit_config: &WitConfig,
        project_dir: impl AsRef<Path>,
    ) -> Result<()> {
        self.apply_registry_auth(&wit_config.registries)?;
        if !wit_config.sources.is_empty() {
            debug!("applying WIT source overrides: {:?}", wit_config.sources);
            self.resolve_extended_pull_configs(&wit_config.sources, project_dir)
                .await
                .context("failed to resolve WIT source overrides")?;
        }
        Ok(())
    }

    /// Apply `wit.registries` authentication to the wkg client config as OCI basic-auth
    /// credentials for each registry host. Entries without credentials are ignored.
    pub fn apply_registry_auth(&mut self, registries: &[WitRegistry]) -> Result<()> {
        for reg in registries {
            let Some((username, token)) = reg.credentials()? else {
                continue;
            };
            let oci_config = OciRegistryConfig {
                credentials: Some(BasicCredentials {
                    username: username.to_string(),
                    password: token.to_string().into(),
                }),
                ..Default::default()
            };
            self.wkg_client_config
                .get_or_insert_registry_config_mut(&reg.registry()?)
                .set_backend_config("oci", oci_config)
                .with_context(|| format!("failed to set registry auth for [{}]", reg.url))?;
        }
        Ok(())
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
            let (ns, pkgs, _version) = parse_wit_package_name(target)?;

            match detect_source_type(source) {
                RegistryPullSource::LocalPath(path) => {
                    let resolved_path = project_dir.as_ref().join(&path);
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
                RegistryPullSource::RemoteHttp(url) => {
                    let wit_dir = download_and_extract_http(&url)
                        .await
                        .with_context(|| format!("failed to download HTTP source [{url}]"))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteGit(url) => {
                    let wit_dir = clone_git_and_find_wit(&url)
                        .await
                        .with_context(|| format!("failed to clone Git source [{url}]"))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteOci(reference) => match pkgs.as_slice() {
                    [] => {
                        // Namespace-level override
                        let mapping = oci_registry_mapping(&reference, &ns, None)?;
                        self.wkg_client_config
                            .set_namespace_registry(ns.clone().try_into()?, mapping);
                    }
                    packages => {
                        // Package-level overrides
                        for pkg in packages {
                            let mapping = oci_registry_mapping(&reference, &ns, Some(pkg))?;
                            self.wkg_client_config.set_package_registry_override(
                                PackageRef::new(
                                    ns.clone().try_into()?,
                                    pkg.to_string().try_into()?,
                                ),
                                mapping,
                            );
                        }
                    }
                },
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
            let key = format!("{namespace}:{package}");
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
    let parsed_url = Url::parse(url).with_context(|| format!("invalid HTTP URL [{url}]"))?;

    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for downloading [{url}]"))?
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
        .with_context(|| format!("failed to download from URL [{url}]"))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read response from URL [{url}]"))?;

    // Extract tar.gz
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&output_path)
        .with_context(|| format!("failed to unpack archive from URL [{url}]"))?;

    find_wit_folder_in_path(&output_path).await
}

/// Clone git repository and find WIT directory
async fn clone_git_and_find_wit(url: &str) -> Result<PathBuf> {
    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for cloning [{url}]"))?
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
        .with_context(|| format!("failed to execute git clone command for [{git_url}]"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Git clone failed for [{git_url}]: {stderr}");
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

/// Load a project's `wkg.toml` package overrides, or the default (empty) config
/// when the project has none.
///
/// `wkg.toml` lets a project resolve WIT packages from a local path or an
/// alternate registry instead of the default one. Both `wash build` and
/// `wash wit fetch` call this so those overrides are honored the same way the
/// standalone `wkg` tool honors them.
pub async fn load_wkg_config(
    project_dir: impl AsRef<Path>,
) -> Result<wasm_pkg_core::config::Config> {
    let config_path = project_dir
        .as_ref()
        .join(wasm_pkg_core::config::CONFIG_FILE_NAME);
    if tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        debug!(path = %config_path.display(), "loading wkg.toml overrides");
        wasm_pkg_core::config::Config::load_from_path(&config_path)
            .await
            .with_context(|| format!("failed to load {}", config_path.display()))
    } else {
        Ok(wasm_pkg_core::config::Config::default())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A `WkgFetcher` backed by a cache under `dir` and empty configs, for exercising the
    /// config-application methods.
    async fn test_fetcher(dir: &Path) -> WkgFetcher {
        let cache = FileCache::new(dir.to_path_buf()).await.unwrap();
        WkgFetcher::new(
            wasm_pkg_core::config::Config::default(),
            wasm_pkg_client::Config::default(),
            cache,
        )
    }

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

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OciMeta {
        registry: Option<String>,
        namespace_prefix: Option<String>,
    }

    fn oci_meta(mapping: &RegistryMapping) -> (String, OciMeta) {
        match mapping {
            RegistryMapping::Custom(custom) => (
                custom.registry.to_string(),
                custom
                    .metadata
                    .protocol_config("oci")
                    .expect("oci protocol config should deserialize")
                    .expect("oci protocol config should be present"),
            ),
            other => panic!("expected a custom OCI mapping, got {other:?}"),
        }
    }

    #[test]
    fn oci_source_conventional_path_is_plain_registry() {
        // The default `{registry}/{namespace}/{package}` layout needs no prefix, and the
        // `:tag` is dropped since the tag comes from the resolved package version.
        let mapping = oci_registry_mapping(
            "ghcr.io/seamlezz/surrealdb:0.2.0",
            "seamlezz",
            Some("surrealdb"),
        )
        .unwrap();
        match mapping {
            RegistryMapping::Registry(reg) => assert_eq!(reg.to_string(), "ghcr.io"),
            other => panic!("expected a plain registry mapping, got {other:?}"),
        }
    }

    #[test]
    fn oci_source_strips_scheme() {
        let mapping = oci_registry_mapping(
            "oci://ghcr.io/seamlezz/surrealdb",
            "seamlezz",
            Some("surrealdb"),
        )
        .unwrap();
        assert!(matches!(mapping, RegistryMapping::Registry(_)));
    }

    #[test]
    fn oci_source_deep_path_sets_namespace_prefix() {
        let mapping = oci_registry_mapping(
            "ghcr.io/myorg/wit/seamlezz/surrealdb",
            "seamlezz",
            Some("surrealdb"),
        )
        .unwrap();
        let (registry, oci) = oci_meta(&mapping);
        assert_eq!(registry, "ghcr.io");
        assert_eq!(oci.registry.as_deref(), Some("ghcr.io"));
        assert_eq!(oci.namespace_prefix.as_deref(), Some("myorg/wit/"));
    }

    #[test]
    fn oci_namespace_level_source() {
        // A namespace-level override with the namespace as the trailing path segment.
        let mapping = oci_registry_mapping("ghcr.io/seamlezz", "seamlezz", None).unwrap();
        assert!(matches!(mapping, RegistryMapping::Registry(_)));

        // A leading path becomes the namespace prefix.
        let mapping = oci_registry_mapping("ghcr.io/myorg/seamlezz", "seamlezz", None).unwrap();
        let (_, oci) = oci_meta(&mapping);
        assert_eq!(oci.namespace_prefix.as_deref(), Some("myorg/"));
    }

    #[test]
    fn oci_source_namespace_mismatch_is_err() {
        // The WIT namespace `seamlezz` does not appear where wasm-pkg would reconstruct it.
        let err = oci_registry_mapping("ghcr.io/other/pkg", "seamlezz", Some("surrealdb"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("does not resolve to the expected repository suffix"),
            "unexpected error: {err}"
        );
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
                username: None,
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
                username: None,
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
                username: None,
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

    #[test]
    fn registry_authority_accepts_host_and_url() {
        assert_eq!(
            registry_authority("ghcr.io").unwrap().to_string(),
            "ghcr.io"
        );
        assert_eq!(
            registry_authority("https://ghcr.io").unwrap().to_string(),
            "ghcr.io"
        );
        assert_eq!(
            registry_authority("localhost:5000").unwrap().to_string(),
            "localhost:5000"
        );
        assert_eq!(
            registry_authority("http://localhost:5000")
                .unwrap()
                .to_string(),
            "localhost:5000"
        );
        // A repository path is not a registry host.
        assert!(registry_authority("ghcr.io/seamlezz").is_err());
    }

    #[test]
    fn wit_registry_requires_username_and_token_together() {
        let tmp = tempfile::tempdir().unwrap();
        let only_token = WitConfig {
            registries: vec![WitRegistry {
                url: "ghcr.io".to_string(),
                username: None,
                token: Some("ghp_xxx".to_string()),
            }],
            ..Default::default()
        };
        let err = only_token.validate(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("both username and token"), "unexpected: {err}");

        let both = WitConfig {
            registries: vec![WitRegistry {
                url: "ghcr.io".to_string(),
                username: Some("me".to_string()),
                token: Some("ghp_xxx".to_string()),
            }],
            ..Default::default()
        };
        assert!(both.validate(tmp.path()).is_ok());
    }

    #[tokio::test]
    async fn apply_registry_auth_configures_oci_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fetcher = test_fetcher(tmp.path()).await;
        fetcher
            .apply_registry_auth(&[WitRegistry {
                url: "https://ghcr.io".to_string(),
                username: Some("me".to_string()),
                token: Some("ghp_xxx".to_string()),
            }])
            .unwrap();

        let registry: Registry = "ghcr.io".parse().unwrap();
        let registry_config = fetcher
            .wkg_client_config
            .registry_config(&registry)
            .expect("registry config should be set for ghcr.io");
        assert!(
            registry_config
                .configured_backend_types()
                .any(|backend| backend == "oci"),
            "oci backend auth should be configured"
        );
    }

    #[tokio::test]
    async fn apply_registry_auth_rejects_half_configured_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fetcher = test_fetcher(tmp.path()).await;
        let err = fetcher
            .apply_registry_auth(&[WitRegistry {
                url: "ghcr.io".to_string(),
                username: None,
                token: Some("ghp_xxx".to_string()),
            }])
            .unwrap_err()
            .to_string();
        assert!(err.contains("both username and token"), "unexpected: {err}");
    }

    #[tokio::test]
    async fn resolve_extended_pull_configs_wires_oci_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let mut fetcher = test_fetcher(tmp.path()).await;

        let sources = HashMap::from([
            (
                "seamlezz:surrealdb".to_string(),
                "ghcr.io/seamlezz/surrealdb".to_string(),
            ),
            ("acme".to_string(), "ghcr.io/acme".to_string()),
        ]);
        fetcher
            .resolve_extended_pull_configs(&sources, tmp.path())
            .await
            .unwrap();

        // Package-level source becomes a package registry override.
        let pkg = PackageRef::new(
            "seamlezz".to_string().try_into().unwrap(),
            "surrealdb".to_string().try_into().unwrap(),
        );
        assert!(
            fetcher
                .wkg_client_config
                .package_registry_override(&pkg)
                .is_some(),
            "package-level OCI source should set a package registry override"
        );

        // Namespace-level source becomes a namespace registry mapping.
        let ns_label = "acme".parse().unwrap();
        assert!(
            fetcher
                .wkg_client_config
                .namespace_registry(&ns_label)
                .is_some(),
            "namespace-level OCI source should set a namespace registry"
        );
    }

    #[tokio::test]
    async fn resolve_extended_pull_configs_wires_local_path_source() {
        let tmp = tempfile::tempdir().unwrap();
        let wit_dir = tmp.path().join("shared-wit");
        std::fs::create_dir_all(&wit_dir).unwrap();
        let mut fetcher = test_fetcher(tmp.path()).await;

        let sources = HashMap::from([("local:shared".to_string(), "shared-wit".to_string())]);
        fetcher
            .resolve_extended_pull_configs(&sources, tmp.path())
            .await
            .unwrap();

        let overrides = fetcher
            .wkg_config
            .overrides
            .as_ref()
            .expect("local path source should populate wkg overrides");
        assert!(
            overrides.contains_key("local:shared"),
            "local path source should set an override for the target package"
        );
    }

    #[tokio::test]
    async fn load_wkg_config_reads_project_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(
            tmp.path().join(wasm_pkg_core::config::CONFIG_FILE_NAME),
            "[overrides]\n\"wasmcloud:app\" = { path = \"../wasmcloud-app\" }\n",
        )
        .await
        .unwrap();

        let config = load_wkg_config(tmp.path()).await.unwrap();
        let overrides = config.overrides.expect("overrides should be loaded");
        let app = overrides
            .get("wasmcloud:app")
            .expect("wasmcloud:app override should be present");
        assert_eq!(app.path.as_deref(), Some(Path::new("../wasmcloud-app")));
    }

    #[tokio::test]
    async fn load_wkg_config_defaults_without_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config = load_wkg_config(tmp.path()).await.unwrap();
        assert!(config.overrides.is_none());
    }
}
