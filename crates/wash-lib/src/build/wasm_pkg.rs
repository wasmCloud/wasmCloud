//! Structs and functions for working with wasm packages as dependencies for components
use std::sync::Arc;
use std::{path::Path, str::FromStr as _};

use anyhow::{Context, Result};
use futures::stream::StreamExt as _;
use futures_util::TryStreamExt;
use semver::Version;
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info};
use wasm_pkg_client::{Client, ContentDigest, PackageRef, Release};

use crate::build::WIT_DIR;
use crate::parser::{CommonConfig, ComponentConfig};

/// Function to fetch all packages in a component's world (dependencies) and store them in the `wit/deps` directory
///
/// Returns a list of all releases that are either present or were fetched. This function logs errors and skips packages
/// that fail to fetch.
pub(crate) async fn fetch_packages(
    component_config: &ComponentConfig,
    common_config: &CommonConfig,
) -> Result<Vec<WkgRelease>> {
    let (world, _id) = super::convert_wit_dir_to_world(
        common_config.path.join(WIT_DIR),
        component_config.wit_world.as_deref(),
    )?;
    let client = std::sync::Arc::new(Client::with_global_defaults()?);

    // For all of the resolvable packages in the world with a version, fetch the release
    let release_futs = world
        .package_names
        .into_iter()
        .filter_map(|(package, _id)| {
            if let Ok(pkg) =
                PackageRef::from_str(format!("{}:{}", package.namespace, package.name).as_str())
            {
                package.version.map(|version| {
                    ensure_release(
                        client.clone(),
                        pkg,
                        version,
                        common_config.path.join(WIT_DIR),
                    )
                })
            } else {
                debug!("Failed to parse package: {:?}, skipping", package);
                None
            }
        });

    let releases = futures::stream::iter(release_futs)
        .filter_map(|fut| async {
            // TODO: Error logging is a nice way to continue on error, but it's probably a
            // reasonable thing to bubble up the error to the user.
            fut.await
                .map_err(|e| error!("Error fetching release: {:?}", e))
                .ok()
        })
        .collect::<Vec<_>>()
        .await;

    // Create a wkg.lock file with the fetched releases
    let mut table = toml::Table::new();
    releases
        .iter()
        .map(WkgRelease::to_toml)
        .for_each(|(key, values)| {
            table.insert(key, values);
        });
    let wkg_lock = common_config.path.join("wit").join("wkg.lock");
    if let Err(e) = tokio::fs::write(
        wkg_lock,
        toml::to_string(&toml::Value::Table(table))
            .context("failed to serialize wkg.lock as toml file")?,
    )
    .await
    {
        error!("Failed to write wkg.lock file: {:?}", e);
    }

    Ok(releases)
}

/// Wrapper around a [`Release`] that includes the [`PackageRef`] and [`Version`] of the release
pub(crate) struct WkgRelease {
    release: Release,
    pkg: PackageRef,
    version: Version,
}

impl WkgRelease {
    /// Returns the (key, value) pair for inclusion in a wkg.lock file
    ///
    /// For example:
    /// ```toml
    /// [namespace:name]
    /// sha256 = "<sha256>"
    /// url = "<url>"
    /// version = "<version>"
    /// ```
    fn to_toml(&self) -> (String, toml::Value) {
        (
            format!("{}:{}", self.pkg.namespace(), self.pkg.name()),
            toml::Value::Table(toml::Table::from_iter([
                (
                    "sha256".to_string(),
                    toml::Value::String(self.release.content_digest.to_string()),
                ),
                (
                    "url".to_string(),
                    toml::Value::String(
                        "TODO: Current API doesn't have method for retrieving URL".to_string(),
                    ),
                ),
                (
                    "version".to_string(),
                    toml::Value::String(self.version.to_string()),
                ),
            ])),
        )
    }
}

async fn ensure_release(
    client: Arc<Client>,
    pkg: PackageRef,
    version: Version,
    base_dir: impl AsRef<Path>,
) -> Result<WkgRelease> {
    let release = client.get_release(&pkg, &version).await?;
    // NOTE(brooksmtownsend): I was just going to place this in `deps`, but that caused issues with
    // the `wit-parser` crate.
    let deps_dir_path = base_dir.as_ref().join("wkg_deps");
    let component_path = deps_dir_path.join(format!(
        "{}_{}_{}.wasm",
        pkg.namespace(),
        pkg.name(),
        version
    ));

    // If we fetched the component and the digest matches upstream, we can skip the fetch
    let existing_digest = ContentDigest::sha256_from_file(&component_path).await;
    match existing_digest {
        Ok(digest) if digest == release.content_digest => {
            debug!(
                "Skipping fetch of release for: {}@{} as it is already present",
                pkg, version
            );
        }
        // If the digest doesn't match, or doesn't exist, we stream the contents of the release to a file
        Err(_) | Ok(_) => {
            // Stream release content to a file.
            info!("Fetching release for: {}@{}", pkg, version);
            let mut stream = client.stream_content(&pkg, &release).await?;
            let _ = tokio::fs::create_dir_all(deps_dir_path).await;
            let mut file = tokio::fs::File::create(component_path)
                .await
                .context("failed to create path for wasm dependency package")?;
            while let Some(chunk) = stream
                .try_next()
                .await
                .context("failed to stream chunk from wasm release")?
            {
                file.write_all(&chunk)
                    .await
                    .context("failed to write chunk to file for wasm release")?;
            }
        }
    }

    Ok(WkgRelease {
        release,
        pkg,
        version,
    })
}
