//! Utilities for working with and managing wit dependencies

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use async_compression::tokio::bufread::GzipDecoder;
use futures::TryStreamExt as _;
use semver::VersionReq;
use url::Url;
use walkdir::WalkDir;
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    PackageRef,
};
use wasm_pkg_core::{config::Override, lock::LockFile, wit::OutputType};
use wasmcloud_core::parse_wit_package_name;

use crate::lib::cli::CommonPackageArgs;
use crate::lib::common::{clone_git_repo, RepoRef};
use crate::lib::parser::{RegistryPullConfig, RegistryPullSource, RegistryPullSourceOverride};

/// Wrapper around a `wasm_pkg_client::Client` including configuration for fetching WIT dependencies.
/// Primarily enables reuse of functionality to override dependencies and properly setup the client.
pub struct WkgFetcher {
    wkg_config: wasm_pkg_core::config::Config,
    wkg_client_config: wasm_pkg_client::Config,
    cache: FileCache,
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

    pub const fn get_config(&self) -> &wasm_pkg_core::config::Config {
        &self.wkg_config
    }

    /// Use configuration to create a caching client for fetching wkg dependencies
    pub fn into_client(self) -> CachingClient<FileCache> {
        let client = wasm_pkg_client::Client::new(self.wkg_client_config);
        CachingClient::new(Some(client), self.cache)
    }

    /// Enable extended pull configurations for wkg config. Call before calling `Self::get_client` to
    /// update configuration used.
    pub async fn resolve_extended_pull_configs(
        &mut self,
        pull_cfg: &RegistryPullConfig,
        wasmcloud_toml_dir: impl AsRef<Path>,
    ) -> Result<()> {
        let wkg_config_overrides = self.wkg_config.overrides.get_or_insert_default();

        for RegistryPullSourceOverride { target, source } in &pull_cfg.sources {
            let (ns, pkgs, _, _, maybe_version) = parse_wit_package_name(target)?;
            let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

            match source {
                // Local files can be used by adding them to the config
                RegistryPullSource::LocalPath(_) => {
                    let path = source
                        .resolve_file_path(wasmcloud_toml_dir.as_ref())
                        .await?;
                    if !tokio::fs::try_exists(&path).await.with_context(|| {
                        format!(
                            "failed to check for registry pull source local path [{}]",
                            path.display()
                        )
                    })? {
                        bail!(
                            "registry pull source path [{}] does not exist",
                            path.display()
                        );
                    }

                    // Set the local override for the namespaces and/or packages
                    update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), path);
                }
                RegistryPullSource::Builtin => bail!("no builtins are supported"),
                RegistryPullSource::RemoteHttp(s) => {
                    let url = Url::parse(s)
                        .with_context(|| format!("invalid registry pull source url [{s}]"))?;
                    let tempdir = tempfile::tempdir()
                        .with_context(|| {
                            format!("failed to create temp dir for downloading [{url}]")
                        })?
                        .into_path();
                    let output_path = tempdir.join("unpacked");
                    let http_client = crate::lib::start::get_download_client()?;
                    let req = http_client.get(url.clone()).send().await.with_context(|| {
                        format!("failed to retrieve WIT output from URL [{}]", &url)
                    })?;
                    let mut archive = tokio_tar::Archive::new(GzipDecoder::new(
                        tokio_util::io::StreamReader::new(req.bytes_stream().map_err(|e| {
                            std::io::Error::other(format!(
                            "failed to receive byte stream while downloading from URL [{}]: {e}",
                            &url
                        ))
                        })),
                    ));
                    archive.unpack(&output_path).await.with_context(|| {
                        format!("failed to unpack archive downloaded from URL [{}]", &url)
                    })?;

                    // Find the first nested directory named 'wit', if present
                    let output_wit_dir = find_wit_folder_in_path(&output_path).await?;

                    // Set the local override for the namespaces and/or packages
                    // Set the local override for the namespaces and/or packages
                    update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), output_wit_dir);
                }
                RegistryPullSource::RemoteGit(s) => {
                    let url = Url::parse(s)
                        .with_context(|| format!("invalid registry pull source url [{s}]"))?;
                    let query_pairs = url.query_pairs().collect::<HashMap<_, _>>();
                    let tempdir = tempfile::tempdir()
                        .with_context(|| {
                            format!("failed to create temp dir for downloading [{url}]")
                        })?
                        .into_path();

                    // Determine the right git ref to use, based on the submitted query params
                    let git_ref = match (
                        query_pairs.get("branch"),
                        query_pairs.get("sha"),
                        query_pairs.get("ref"),
                    ) {
                        (Some(branch), _, _) => Some(RepoRef::Branch(String::from(branch.clone()))),
                        (_, Some(sha), _) => Some(RepoRef::from_str(sha)?),
                        (_, _, Some(r)) => Some(RepoRef::Unknown(String::from(r.clone()))),
                        _ => None,
                    };

                    clone_git_repo(
                        None,
                        &tempdir,
                        s.into(),
                        query_pairs
                            .get("subfolder")
                            .map(|s| String::from(s.clone())),
                        git_ref,
                    )
                    .await
                    .with_context(|| {
                        format!("failed to clone repo for pull source git repo [{s}]",)
                    })?;

                    // Find the first nested directory named 'wit', if present
                    let output_wit_dir = find_wit_folder_in_path(&tempdir).await?;

                    // Set the local override for the namespaces and/or packages
                    update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), output_wit_dir);
                }
                // All other registry pull sources should be directly convertible to `RegistryMapping`s
                rps @ (RegistryPullSource::RemoteOci(_)
                | RegistryPullSource::RemoteHttpWellKnown(_)) => {
                    let registry = rps.clone().try_into()?;
                    match (ns, pkgs.as_slice()) {
                        // namespace-level override
                        (ns, []) => {
                            self.wkg_client_config.set_namespace_registry(
                                format!("{ns}{version_suffix}").try_into()?,
                                registry,
                            );
                        }
                        // package-level override
                        (ns, packages) => {
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

    /// This is a hacky, monkey-patch helper for the fact that the wasi:logging package is not versioned
    /// in the host, which makes it hard to use with packaging tools. We have added a version, but
    /// pretty much everything uses the versionless wasi:logging package. This function wraps the normal
    /// dependency fetching steps, checking if the package has a wasi:logging dep that isn't versioned.
    /// If it does have the unversioned one, then the hackery commences to do some string replacements
    /// in the wit files in a temp dir, pulls down the dependencies, and then removes the versioned wit.
    /// This is ugliness in the highest degree, but it is the only way to get the logging package to
    /// work with the packaging tools. The current libraries don't really support printing unresolved
    /// packages or substituting things in (which makes sense), so this is what we have to live with
    ///
    /// DO NOT USE THIS unless you know what you are doing. This function is exempted from any semver
    /// guarantees and will be removed as soon as we move to the properly versioned wasi:logging
    /// package.
    #[doc(hidden)]
    pub async fn monkey_patch_fetch_logging(
        mut self,
        wit_dir: impl AsRef<Path>,
        lock: &mut LockFile,
    ) -> Result<()> {
        let wasi_logging_name: PackageRef = "wasi:logging"
            .parse()
            .expect("wasi:logging top parse as a PackageRef");
        // This is inefficient since we have to load this again when we fetch deps, but we need to do
        // this to get the list of packages from the package
        let (_, packages) = wasm_pkg_core::wit::get_packages(&wit_dir)
            .context("failed to get packages from wit dir")?;
        // If there is a dependency on unversioned wasi:logging, add an override (if not present)
        let patch_dir = if packages.contains(&(wasi_logging_name.clone(), VersionReq::STAR)) {
            // copy all top level wit files to a temp dir. All the stuff people should be doing at the top
            // level so this is fine
            let wit_dir_temp = tokio::task::spawn_blocking(tempfile::tempdir)
                .await
                .context("failed to create temporary wit patch directory")?
                .context("failed to create temporary wit patch directory")?;
            let mut readdir = tokio::fs::read_dir(&wit_dir)
                .await
                .context("failed to read temporary wit patch directory")?;
            while let Some(entry) = readdir
                .next_entry()
                .await
                .context("failed to read entry in temporary wit patch directory")?
            {
                let path = entry.path();
                let meta = entry
                    .metadata()
                    .await
                    .context("failed to get metadata for entry in temporary wit patch directory")?;

                if meta.is_file() && path.extension().unwrap_or_default() == "wit" {
                    // Read all data as a string and replace
                    let data = tokio::fs::read_to_string(&path).await.context(
                        "failed to read interface for entry in temporary wit patch directory",
                    )?;
                    let data =
                        data.replace("wasi:logging/logging", "wasi:logging/logging@0.1.0-draft");
                    tokio::fs::write(wit_dir_temp.path().join(path.file_name().unwrap()), data)
                        .await
                        .context(
                            "failed to write interface for entry in temporary wit patch directory",
                        )?;
                }
            }
            // set the overrides
            let overrides = self.wkg_config.overrides.get_or_insert_with(HashMap::new);
            if let std::collections::hash_map::Entry::Vacant(e) =
                overrides.entry(wasi_logging_name.to_string())
            {
                e.insert(Override {
                    version: Some("=0.1.0-draft".parse().unwrap()),
                    ..Default::default()
                });
            }
            Some(wit_dir_temp)
        } else {
            None
        };

        let client = CachingClient::new(
            Some(wasm_pkg_client::Client::new(self.wkg_client_config)),
            self.cache,
        );

        wasm_pkg_core::wit::fetch_dependencies(
            &self.wkg_config,
            patch_dir
                .as_ref()
                .map_or(wit_dir.as_ref(), tempfile::TempDir::path),
            lock,
            client,
            OutputType::Wit,
        )
        .await?;

        if let Some(patch_dir) = patch_dir {
            // Rewrite the logging dep to not have a version
            let dep_path = patch_dir
                .path()
                .join("deps")
                .join("wasi-logging-0.1.0-draft")
                .join("package.wit");
            let contents = tokio::fs::read_to_string(&dep_path).await?;
            let replaced =
                contents.replace("package wasi:logging@0.1.0-draft;", "package wasi:logging;");
            tokio::fs::write(&dep_path, replaced)
                .await
                .context("Unable to write patched logging dependency")?;
            // Remove the destination deps
            let dest_deps_dir = wit_dir.as_ref().join("deps");
            match tokio::fs::remove_dir_all(&dest_deps_dir).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            };
            // Copy the deps dir back
            copy_dir(patch_dir.path().join("deps"), dest_deps_dir).await?;
        }
        Ok(())
    }
}

async fn copy_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&destination).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let filetype = entry.file_type().await?;
        if filetype.is_dir() {
            Box::pin(copy_dir(
                entry.path(),
                destination.as_ref().join(entry.file_name()),
            ))
            .await?;
        } else {
            tokio::fs::copy(entry.path(), destination.as_ref().join(entry.file_name())).await?;
        }
    }
    Ok(())
}

/// Helper for updating wkg overrides
fn update_override_dir(
    overrides: &mut HashMap<String, Override>,
    ns: String,
    pkgs: &[String],
    dir: impl AsRef<Path>,
) {
    let dir = dir.as_ref();
    match (ns, pkgs) {
        // namespace-level override
        (ns, []) => {
            overrides.insert(
                ns,
                Override {
                    path: Some(dir.into()),
                    version: None,
                },
            );
        }
        // package-level override
        (ns, packages) => {
            for pkg in packages {
                overrides.insert(
                    format!("{ns}:{pkg}"),
                    Override {
                        path: Some(dir.into()),
                        version: None,
                    },
                );
            }
        }
    }
}

/// Find a folder named 'wit' inside a given dir, if present, otherwise return the path itself
async fn find_wit_folder_in_path(dir: impl AsRef<Path>) -> Result<PathBuf> {
    let dir = dir.as_ref().to_path_buf();
    // Find the first nested directory named 'wit', if present
    tokio::task::spawn_blocking(move || {
        if let Some(path) = WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .find(|e| e.file_name() == "wit")
            .map(walkdir::DirEntry::into_path)
        {
            path
        } else {
            dir
        }
    })
    .await
    .context("failed to resolve folder with WIT in downloaded archive")
}
