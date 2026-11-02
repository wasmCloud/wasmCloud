//! The errors and messages a WIT fetch reports.

use std::path::PathBuf;

use anyhow::{Result, bail};
use wasm_pkg_client::{
    PackageRef,
    caching::{CachingClient, FileCache},
};

use super::requested_version;

/// A package in an overridden namespace that the namespace's local source does not hold.
#[derive(Debug)]
pub(super) struct SourceLacksPackage {
    pub(super) package: PackageRef,
    pub(super) source: PathBuf,
    pub(super) held: Vec<PackageRef>,
}

impl std::fmt::Display for SourceLacksPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self
            .held
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "[{}] is not in the source override for its namespace at [{}], which holds [{held}].\n\
             \n\
             Add the package to that source, or give it its own entry under `wit.sources` in \
             .wash/config.yaml (or in wkg.toml).",
            self.package,
            self.source.display(),
        )
    }
}

impl std::error::Error for SourceLacksPackage {}

/// The message for a package the WIT names at more than one version.
///
/// Versions in different major.minor lines are separate packages that WIT is happy to resolve
/// side by side, but a fetch only resolves one version per package name, so `wit/deps` ends up
/// with one of them and the imports of the other have nothing to resolve against. Two versions
/// within one major.minor line cannot coexist at all.
pub(super) fn multiple_versions_message(
    package: &PackageRef,
    requirements: &[semver::VersionReq],
) -> String {
    let versions: Vec<String> = requirements
        .iter()
        .map(|requirement| requested_version(requirement).unwrap_or_else(|| NO_VERSION.to_string()))
        .collect();
    let listed = versions.join(", ");

    let parsed: Vec<semver::Version> = requirements
        .iter()
        .filter_map(|requirement| semver::Version::parse(&requested_version(requirement)?).ok())
        .collect();
    let same_line = parsed.iter().enumerate().any(|(i, version)| {
        parsed
            .iter()
            .skip(i + 1)
            .any(|other| (version.major, version.minor) == (other.major, other.minor))
    });

    // An unversioned reference asks for `*`, which no versioned package satisfies
    if let Some(named) = versions.iter().find(|version| *version != NO_VERSION)
        && versions.iter().any(|version| version == NO_VERSION)
    {
        return format!(
            "The WIT names [{package}] both with and without a version: {listed}.\n\
             \n\
             An unversioned statement does not resolve against a versioned package. Give the \
             unversioned one a version — [{named}] is what the rest of the WIT uses."
        );
    }

    match same_line {
        true => format!(
            "The WIT names [{package}] at more than one version in the same major.minor line: \
             {listed}.\n\
             \n\
             Those cannot coexist; the imports have to agree on one of them."
        ),
        false => format!(
            "The WIT names [{package}] at more than one version: {listed}.\n\
             \n\
             A fetch resolves one version per package name, so only one of those reaches \
             wit/deps and the imports of the others have nothing to resolve against. The imports \
             have to agree on one version."
        ),
    }
}

/// What the report calls a statement that names no version
const NO_VERSION: &str = "no version";

/// Check the registry for a package before it is resolved, so that a package or version the
/// registry does not have is reported as such rather than as a resolution failure. Returns the
/// message to show, or `None` when the requirement can be satisfied.
pub(super) async fn missing_from_registry(
    client: &CachingClient<FileCache>,
    package: &PackageRef,
    version: Option<&str>,
    requirement: &semver::VersionReq,
) -> Result<Option<String>> {
    let registry = client
        .client()
        .ok()
        .and_then(|client| client.config().resolve_registry(package))
        .map(|registry| format!(" in registry [{registry}]"))
        .unwrap_or_default();

    let versions = match client.list_all_versions(package).await {
        Ok(versions) => versions,
        Err(wasm_pkg_client::Error::PackageNotFound) => {
            return Ok(Some(format!(
                "Package [{package}] was not found{registry}.\n\
                 \n\
                 Check the package name, and that the registry serving it is configured under \
                 `wit.sources` in .wash/config.yaml."
            )));
        }
        Err(wasm_pkg_client::Error::NoRegistryForNamespace(namespace)) => {
            return Ok(Some(format!(
                "No registry is configured for the [{namespace}] namespace, so [{package}] cannot be looked up.\n\
                 \n\
                 Map the namespace to a registry under `wit.sources` in .wash/config.yaml."
            )));
        }
        Err(e) => bail!("failed to list versions of [{package}]{registry}: {e}"),
    };

    let mut published: Vec<semver::Version> = versions
        .into_iter()
        .filter(|version| !version.yanked)
        .map(|version| version.version)
        .collect();
    published.sort();

    if published.iter().any(|v| requirement.matches(v)) {
        return Ok(None);
    }

    if published.is_empty() {
        return Ok(Some(format!(
            "Package [{package}] has no published versions{registry}."
        )));
    }

    let listed = published
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Some(match version {
        Some(version) => format!(
            "Package [{package}] has no version [{version}]{registry}.\n\
             \n\
             Published versions: {listed}"
        ),
        // Every version being a pre-release is the one case where a package with published
        // versions still selects none of them, and naming one is the way out
        None if published.iter().all(|version| !version.pre.is_empty()) => {
            let newest = published
                .last()
                .map(ToString::to_string)
                .unwrap_or_default();
            format!(
                "Every published version of [{package}]{registry} is a pre-release, and a \
                 pre-release is only selected when it is named.\n\
                 \n\
                 Published versions: {listed}\n\
                 \n\
                 Name one, for example [{package}@{newest}]."
            )
        }
        None => format!(
            "No version of [{package}] could be selected{registry}.\n\
             \n\
             Published versions: {listed}"
        ),
    }))
}
