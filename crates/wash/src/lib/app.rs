//! Interact with and manage wadm applications over NATS, requires the `nats` feature
//!
//! This crate is essentially a wrapper around the `wadm_client` crate, and it's recommended to use
//! that crate directly instead.
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, Context};
use async_nats::Client;
use regex::Regex;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::warn;
use url::Url;
use wadm_client::Result;
use wadm_types::api::{ModelSummary, Status, VersionInfo};
use wadm_types::validation::{validate_manifest, ValidationFailure, ValidationFailureLevel};
use wadm_types::{CapabilityProperties, ComponentProperties, Manifest, Properties};
use wasmcloud_core::tls;
use wasmcloud_core::OciFetcher;

use crate::lib::config::DEFAULT_LATTICE;

#[derive(Debug)]
pub enum AppManifest {
    SerializedModel(serde_yaml::Value),
    ModelName(String),
}

impl AppManifest {
    /// Resolve relative file paths in the given app manifest to some base path
    pub fn resolve_image_relative_file_paths(&mut self, base: impl AsRef<Path>) -> Result<()> {
        if let Self::SerializedModel(ref mut content) = self {
            resolve_relative_file_paths_in_yaml(content, base)?;
        }
        Ok(())
    }

    /// Retrieve the name of a given [`AppManifest`]
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::ModelName(name) => Some(name),
            Self::SerializedModel(manifest) => manifest
                .get("metadata")?
                .get("name")
                .and_then(|v| v.as_str()),
        }
    }

    /// Retrieve the version of a given [`AppManifest`], returning None if the manifest
    /// does not contain a version (or is not the type to contain a version)
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::ModelName(_) => None,
            Self::SerializedModel(manifest) => manifest
                .get("metadata")?
                .get("annotations")?
                .get("version")
                .and_then(|v| v.as_str()),
        }
    }
}

/// Resolve the relative paths in a YAML value, given a base path (directory)
/// from which to resolve the relative paths that are found
fn resolve_relative_file_paths_in_yaml(
    content: &mut serde_yaml::Value,
    base_dir: impl AsRef<Path>,
) -> Result<()> {
    match content {
        // If we encounter a string anywhere that is a relative path, resolve it
        serde_yaml::Value::String(s)
            if s.starts_with("file://") && s.chars().nth(7).is_some_and(|v| v != '/') =>
        {
            // Convert the base dir + relative path into a file based URL
            let full_path = base_dir.as_ref().join(
                s.strip_prefix("file://")
                    .context("failed to strip prefix on relative file path")?,
            );

            // Ensure the resolved relative path exists
            if !full_path.exists() {
                return Err(wadm_client::error::ClientError::ManifestLoad(
                    anyhow::anyhow!(
                        "relative file path [{s}] (resolved to [{}]) does not exist",
                        full_path.display()
                    ),
                ));
            }

            // Build a file based URL and replace the existing one
            if let Ok(url) = Url::from_file_path(&full_path) {
                *s = url.into();
            } else {
                warn!(
                    "failed to build a file URL from path [{}], is the file missing?",
                    full_path.display()
                );
            }
        }
        // If the YAML value is a mapping, recur into it to process more values
        serde_yaml::Value::Mapping(m) => {
            for (_key, value) in m.iter_mut() {
                resolve_relative_file_paths_in_yaml(value, base_dir.as_ref())?;
            }
        }
        // If the YAML value is a sequence, recur into it to process more values
        serde_yaml::Value::Sequence(values) => {
            for value in values {
                resolve_relative_file_paths_in_yaml(value, base_dir.as_ref())?;
            }
        }
        // All other cases we can ignore replacements
        _ => {}
    }
    Ok(())
}

pub trait AsyncReadSource: AsyncRead + Unpin + Send + Sync {}
impl<T: AsyncRead + Unpin + Send + Sync> AsyncReadSource for T {}
pub enum AppManifestSource {
    AsyncReadSource(Box<dyn AsyncReadSource>),
    File(PathBuf),
    Url(url::Url),
    // the inner string is intended to be the model name
    Model(String),
}

impl FromStr for AppManifestSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self, Self::Err> {
        if s == "-" {
            return Ok(Self::AsyncReadSource(Box::new(tokio::io::stdin())));
        }

        // Is the source a file path?
        if PathBuf::from(s).is_file() {
            match PathBuf::from(s).extension() {
                    Some(ext) if ext == "yaml" || ext == "yml" || ext == "json" => {
                        return Ok(Self::File(PathBuf::from(s)));
                    }
                    _ => bail!("file {} has an unsupported extension. Only .yaml, .yml, and .json are supported at this time", s),

                }
        }

        // Is the source a url?
        if Url::parse(s).is_ok() {
            if !s.starts_with("http") {
                bail!("file url {} has an unsupported scheme. Only http(s):// is supported at this time", s)
            }

            return Ok(Self::Url(url::Url::parse(s)?));
        }

        // Is the source a valid model name?
        let model_name_regex =
            Regex::new(r"^[-\w]+$").context("failed to instantiate manifest name regex")?;

        if model_name_regex.is_match(s) {
            return Ok(Self::Model(s.to_owned()));
        }

        bail!("invalid manifest source: {}", s)
    }
}

/// Undeploy a model, instructing wadm to no longer manage the given application
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application is managed on, defaults to `default`
/// * `model_name` - Model name to undeploy
pub async fn undeploy_model(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
) -> Result<()> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client.undeploy_manifest(model_name).await.map(|_| ())
}

/// Deploy a model, instructing wadm to manage the application
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application will be managed on, defaults to `default`
/// * `model_name` - Model name to deploy
/// * `version` - Version to deploy, defaults to deploying the latest "put" version
pub async fn deploy_model(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
    version: Option<String>,
) -> Result<(String, Option<String>)> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client
        .deploy_manifest(model_name, version.as_deref())
        .await
}

/// Put a model definition, instructing wadm to store the application manifest for later deploys
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifest will be stored on, defaults to `default`
/// * `model` - The full YAML or JSON string containing the OAM wadm manifest
///
/// # Returns
/// The name and version of the model that was put
pub async fn put_model(
    client: &Client,
    lattice: Option<String>,
    model: &str,
) -> Result<(String, String)> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    let manifest = model.as_bytes();
    wadm_client.put_manifest(manifest).await
}

/// Deploy a model, instructing wadm to manage the application
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application will be managed on, defaults to `default`
/// * `model` - The full YAML or JSON string containing the OAM wadm manifest
///
/// # Returns
/// The name and version of the model that was put
pub async fn put_and_deploy_model(
    client: &Client,
    lattice: Option<String>,
    model: &str,
) -> Result<(String, String)> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    let manifest = model.as_bytes();
    wadm_client.put_and_deploy_manifest(manifest).await
}

/// Query wadm for the history of a given model name
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifest is stored on, defaults to `default`
/// * `model_name` - Name of the model to retrieve history for
pub async fn get_model_history(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
) -> Result<Vec<VersionInfo>> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client.list_versions(model_name).await
}

/// Query wadm for the status of a given model by name
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifest is stored on, defaults to `default`
/// * `model_name` - Name of the model to retrieve status for
pub async fn get_model_status(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
) -> Result<Status> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client.get_manifest_status(model_name).await
}

/// Query wadm for details on a given model
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifest is stored on, defaults to `default`
/// * `model_name` - Name of the model to retrieve history for
/// * `version` - Version to retrieve, defaults to retrieving the latest "put" version
pub async fn get_model_details(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
    version: Option<String>,
) -> Result<Manifest> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client
        .get_manifest(model_name, version.as_deref())
        .await
}

/// Delete a model version from wadm
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifest is stored on, defaults to `default`
/// * `model_name` - Name of the model
/// * `version` - Version to retrieve, defaults to deleting the latest "put" version (or all if `delete_all` is specified)
pub async fn delete_model_version(
    client: &Client,
    lattice: Option<String>,
    model_name: &str,
    version: Option<String>,
) -> Result<bool> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client
        .delete_manifest(model_name, version.as_deref())
        .await
}

/// Query wadm for all application manifests
///
/// # Arguments
/// * `client` - The [`Client`] to use in order to send the request message
/// * `lattice` - Optional lattice name that the application manifests are stored on, defaults to `default`
pub async fn get_models(client: &Client, lattice: Option<String>) -> Result<Vec<ModelSummary>> {
    let wadm_client = wadm_client::Client::from_nats_client(
        &lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string()),
        None,
        client.clone(),
    );

    wadm_client.list_manifests().await
}

//  NOTE(ahmedtadde): This should probably be refactored at some point to account for cases where the source's input is unusually (or erroneously) large.
//  For now, we'll just assume that the input is small enough to be a oneshot read into memory and that the default timeout of 1 sec is plenty sufficient (or even too generous?) for the desired/expected behavior.
pub async fn load_app_manifest(source: AppManifestSource) -> anyhow::Result<AppManifest> {
    let load_from_source = || async {
        match source {
            AppManifestSource::AsyncReadSource(mut stdin) => {
                let mut buffer = String::new();
                stdin
                    .read_to_string(&mut buffer)
                    .await
                    .context("failed to read model from stdin")?;
                if buffer.is_empty() {
                    bail!("unable to load app manifest from empty stdin input")
                }

                Ok(AppManifest::SerializedModel(
                    serde_yaml::from_str(&buffer).context("failed to parse yaml from STDIN")?,
                ))
            }
            AppManifestSource::File(path) => {
                let mut manifest = AppManifest::SerializedModel(
                    serde_yaml::from_str(
                        tokio::fs::read_to_string(&path)
                            .await
                            .context("failed to read model from file")?
                            .as_str(),
                    )
                    .with_context(|| {
                        format!("failed to parse yaml from file @ [{}]", path.display())
                    })?,
                );

                // For manifests loaded from a local file, canonicalize the path that held the YAML
                // and use that directory (immediate parent) to resolve relative file paths inside
                manifest.resolve_image_relative_file_paths(
                    path.canonicalize()
                        .context("failed to canonicalize path to app manifest")?
                        .parent()
                        .context("failed to get parent directory of app manifest")?,
                )?;

                Ok(manifest)
            }
            AppManifestSource::Url(url) => {
                let res = tls::DEFAULT_REQWEST_CLIENT
                    .get(url.clone())
                    .send()
                    .await
                    .context("request to remote model file failed")?;
                let text = res
                    .text()
                    .await
                    .context("failed to read model from remote file")?;
                serde_yaml::from_str(&text)
                    .with_context(|| format!("failed to parse YAML from URL [{url}]"))
                    .map(AppManifest::SerializedModel)
            }
            AppManifestSource::Model(name) => Ok(AppManifest::ModelName(name)),
        }
    };

    // Note(ahmedtadde): considered having a timeout: Option<Duration> parameter, but decided against it since, given the use case for this fn, the callers can fairly
    // assume that the manifest should be loaded within a reasonable time frame. Now, reasonable is debatable, but i think anything over 1 sec is out of the question as things stand.
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
    tokio::time::timeout(DEFAULT_TIMEOUT, load_from_source())
        .await
        .context("app manifest loader timed out")?
}

/// Validate the contents of a manifest file and optionally validate the OCI references
pub async fn validate_manifest_file(
    manifest_file_path: &Path,
    oci_check: bool,
) -> Result<(Manifest, Vec<ValidationFailure>)> {
    let content = tokio::fs::read_to_string(manifest_file_path)
        .await
        .with_context(|| {
            format!(
                "failed to read manifest file [{}]",
                manifest_file_path.display()
            )
        })?;

    let manifest = serde_yaml::from_slice(content.as_ref()).with_context(|| {
        format!(
            "failed to parse manifest content in file: {}",
            manifest_file_path.display()
        )
    })?;

    let mut failures = validate_manifest(&manifest).await.with_context(|| {
        format!(
            "failed to validate manifest in file: {}",
            manifest_file_path.display()
        )
    })?;

    if oci_check {
        let image_references = extract_image_references(&manifest);
        validate_oci_references(image_references, &mut failures).await;
    }
    Ok((manifest, failures))
}

pub async fn validate_oci_references(refs: Vec<String>, failures: &mut Vec<ValidationFailure>) {
    let fetcher = OciFetcher::default();

    for image in refs {
        if let Err(err) = fetcher.fetch_component(&image).await {
            let mut fetch_failure = ValidationFailure::default();
            fetch_failure.level = ValidationFailureLevel::Error;
            fetch_failure.msg = format!("Failed to fetch OCI component '{image}': {err}");
            failures.push(fetch_failure);
        }
    }
}

/// Extract image references from a given manifest
#[must_use]
pub fn extract_image_references(manifest: &Manifest) -> Vec<String> {
    let mut image_refs = Vec::new();
    for component in &manifest.spec.components {
        match &component.properties {
            Properties::Component {
                properties:
                    ComponentProperties {
                        image: Some(image), ..
                    },
            } => {
                image_refs.push(image.clone());
            }
            Properties::Capability {
                properties:
                    CapabilityProperties {
                        image: Some(image), ..
                    },
            } => {
                image_refs.push(image.clone());
            }
            _ => {}
        }
    }
    image_refs
}

#[cfg(test)]
mod test {
    use super::*;
    use tempfile::tempdir;

    use anyhow::Result;

    #[test]
    fn test_app_manifest_source_from_str() -> Result<(), Box<dyn std::error::Error>> {
        // test stdin
        let stdin = AppManifestSource::from_str("-")?;
        assert!(
            matches!(stdin, AppManifestSource::AsyncReadSource(_)),
            "expected AppManifestSource::AsyncReadSource"
        );

        // create temporary file for this test
        let tmp_dir = tempdir()?;
        std::fs::write(tmp_dir.path().join("foo.yaml"), "foo")?;
        std::fs::write(tmp_dir.path().join("foo.toml"), "foo")?;

        // test file
        let file = AppManifestSource::from_str(tmp_dir.path().join("foo.yaml").to_str().unwrap())?;
        assert!(
            matches!(file, AppManifestSource::File(_)),
            "expected AppManifestSource::File"
        );

        // test url
        let url = AppManifestSource::from_str(
            "https://raw.githubusercontent.com/wasmCloud/wasmCloud/main/examples/rust/components/http-hello-world/wadm.yaml",
        )?;

        assert!(
            matches!(url, AppManifestSource::Url(_)),
            "expected AppManifestSource::Url"
        );

        let url = AppManifestSource::from_str(
            "https://raw.githubusercontent.com/wasmCloud/wasmCloud/main/examples/rust/components/http-hello-world/wadm.yaml",
        )?;

        assert!(
            matches!(url, AppManifestSource::Url(_)),
            "expected AppManifestSource::Url"
        );

        // test model
        let model = AppManifestSource::from_str("foo")?;
        assert!(
            matches!(model, AppManifestSource::Model(_)),
            "expected AppManifestSource::Model"
        );

        // test invalid
        let invalid = AppManifestSource::from_str("foo.bar");
        assert!(
            invalid.is_err(),
            "expected error on invalid app manifest model name"
        );

        let invalid = AppManifestSource::from_str("sftp://foobar.com");
        assert!(
            invalid.is_err(),
            "expected error on invalid app manifest url source"
        );

        let invalid =
            AppManifestSource::from_str(tmp_dir.path().join("foo.json").to_str().unwrap());

        assert!(
            invalid.is_err(),
            "expected error on invalid app manifest file source"
        );

        let invalid =
            AppManifestSource::from_str(tmp_dir.path().join("foo.toml").to_str().unwrap());

        assert!(
            invalid.is_err(),
            "expected error on invalid app manifest file source"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_relative_manifest() -> Result<()> {
        let tmp_dir = tempdir()?;
        std::fs::write(tmp_dir.path().join("foo.yaml"), "exists")?;
        let mut yaml = serde_yaml::from_str(
            r"
mapping:
  path: 'file://foo.yaml'
",
        )
        .context("failed to build YAML")?;

        resolve_relative_file_paths_in_yaml(&mut yaml, tmp_dir)
            .context("failed to resolve relative file path")?;
        assert!(matches!(
                &yaml["mapping"]["path"],
                serde_yaml::Value::String(s) if s.contains("file:///") && s.contains("/foo.yaml")
        ));
        Ok(())
    }
}
