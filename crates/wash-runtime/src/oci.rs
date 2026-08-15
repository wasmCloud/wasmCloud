//! OCI registry operations for pulling WebAssembly components
//!
//! This module provides functionality to interact with OCI registries for
//! WebAssembly components, including docker credential integration and
//! file-based caching. This module is only available when the `oci` feature is enabled.
//!
//! # Examples
//!
//! ```no_run
//! use wash_runtime::oci::{pull_component, OciConfig, OciPullPolicy};
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Basic pull
//!     let config = OciConfig::default();
//!     let (component_bytes, _digest) = pull_component("ghcr.io/wasmcloud/components/http-hello-world:latest", config, OciPullPolicy::IfNotPresent).await?;
//!     println!("Pulled component of {} bytes", component_bytes.len());
//!
//!     // Pull with credentials and timeout
//!     let config = OciConfig::new_with_credentials("username", "password")
//!         .with_timeout(Duration::from_secs(30));
//!     let (bytes, digest) = pull_component("ghcr.io/my-org/private:latest", config, OciPullPolicy::IfNotPresent).await?;
//!     println!("Pulled {} bytes, digest: {}", bytes.len(), digest);
//!
//!     Ok(())
//! }
//! ```

use anyhow::{Context, Result, anyhow, bail};
use docker_credential::{CredentialRetrievalError, DockerCredential, get_credential};
use oci_client::{
    Reference,
    client::{Certificate, CertificateEncoding, Client, ClientConfig, ClientProtocol},
    manifest::{OciDescriptor, OciImageManifest},
    secrets::RegistryAuth,
};
use oci_wasm::{ToConfig, WASM_LAYER_MEDIA_TYPE, WasmConfig};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};
use tracing::{debug, instrument, warn};

/// Extra CA certificates every OCI client in this process trusts, on top of
/// the compiled-in webpki roots.
///
/// Trust roots are a property of the host, not of any one pull, so they live
/// here rather than on [`OciConfig`]. That is built per workload (from an
/// image pull secret) and per plugin, and would otherwise have to carry the
/// same value to every construction site.
///
/// Empty unless [`set_extra_ca_certificates`] is called, which keeps the
/// default behavior exactly as it was: the roots `oci-client` compiles in.
static EXTRA_CA_CERTIFICATES: OnceLock<Vec<Certificate>> = OnceLock::new();

/// Trust the PEM CA bundles at `paths` for every subsequent OCI pull or push.
///
/// Call once, before serving. `oci-client` builds its TLS from the webpki roots
/// and honors no environment override, so a registry behind a private CA (an
/// in-cluster one, or a corporate mirror) is unreachable without this short of
/// disabling verification altogether.
///
/// Fails when a bundle cannot be read or does not parse, rather than starting
/// a host that will reject every pull from the registry it was pointed at.
pub fn set_extra_ca_certificates(paths: &[PathBuf]) -> Result<()> {
    let certs = load_ca_certificates(paths)?;
    debug!(count = certs.len(), "trusting extra OCI CA certificates");
    if EXTRA_CA_CERTIFICATES.set(certs).is_err() {
        warn!("extra OCI CA certificates were already set; keeping the first set");
    }
    Ok(())
}

/// Read and parse PEM CA bundles from disk. Split from
/// [`set_extra_ca_certificates`] so the loading is testable on its own: the
/// store it writes to can only be set once per process.
///
/// The certificates are parsed here and the bytes then handed on as read.
/// Parsing is what makes a bad bundle a startup failure: `oci-client` builds
/// its client through `Client::new`, which logs and falls back to a wholly
/// default configuration when a certificate fails to parse. That discards the
/// registry protocol, the timeouts and the proxy along with the trust roots,
/// and leaves only a warning to say so.
fn load_ca_certificates(paths: &[PathBuf]) -> Result<Vec<Certificate>> {
    paths
        .iter()
        .map(|path| {
            let data = std::fs::read(path)
                .with_context(|| format!("failed to read OCI CA bundle {}", path.display()))?;
            validate_ca_bundle(&data)
                .with_context(|| format!("invalid OCI CA bundle {}", path.display()))?;
            Ok(Certificate {
                encoding: CertificateEncoding::Pem,
                data,
            })
        })
        .collect()
}

/// Check that `data` is a PEM bundle holding at least one usable certificate.
///
/// Adding to a [`rustls::RootCertStore`] is the same work the TLS stack does
/// when the client is built, so a bundle that passes here cannot fail there:
/// PEM framing, and an X.509 body webpki accepts. A file that parses but holds
/// no certificate is rejected too. It would otherwise be accepted and trust
/// nothing, which reads identically to a CA that does not cover the registry.
fn validate_ca_bundle(data: &[u8]) -> Result<()> {
    use rustls::pki_types::pem::PemObject as _;

    let certs = rustls::pki_types::CertificateDer::pem_slice_iter(data)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("not PEM-encoded certificates: {err}"))?;
    if certs.is_empty() {
        bail!("no certificates found");
    }
    let mut store = rustls::RootCertStore::empty();
    for cert in certs {
        store
            .add(cert)
            .map_err(|err| anyhow!("certificate is not usable as a trust root: {err}"))?;
    }
    Ok(())
}

/// The extra CA certificates configured for this process, for a `ClientConfig`.
fn extra_ca_certificates() -> Vec<Certificate> {
    EXTRA_CA_CERTIFICATES.get().cloned().unwrap_or_default()
}

#[allow(deprecated)]
#[deprecated = "old media type used before Wasm WG standardization"]
const WASMCLOUD_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";

/// Configuration for OCI operations
/// ️ **Credential Precedence**:
/// 1. Explicit credentials (if provided in this config)
/// 2. Docker credential helper (system default)
/// 3. Anonymous (if no credentials found)
///
/// # Rate Limiting
///
/// Note: This implementation does not include retry logic or exponential backoff for
/// authentication failures. Repeated failures may trigger registry rate limits or
/// account lockouts. Consider implementing retry logic at a higher level if needed.
#[derive(Debug, Default, Clone)]
pub struct OciConfig {
    /// Optional explicit credentials (username, password)
    pub credentials: Option<(String, String)>,
    /// Whether to allow insecure registries (HTTP instead of HTTPS)
    pub insecure: bool,
    /// Cache directory override
    pub cache_dir: Option<PathBuf>,
    /// Timeout for OCI operations (pull, push, etc.)
    /// If None, uses default timeout from oci-client
    pub timeout: Option<Duration>,
}

impl OciConfig {
    /// Create a new OciConfig with a specific cache directory
    pub fn new_with_cache(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir: Some(cache_dir),
            ..Default::default()
        }
    }

    /// Create a new OciConfig with explicit credentials
    pub fn new_with_credentials(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            credentials: Some((username.into(), password.into())),
            ..Default::default()
        }
    }

    /// Create a new OciConfig for insecure registries (HTTP)
    pub fn new_insecure() -> Self {
        Self {
            insecure: true,
            ..Default::default()
        }
    }

    /// Create a new OciConfig with a timeout
    pub fn new_with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
            ..Default::default()
        }
    }

    /// Set the timeout for this config
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Cache manager for OCI artifacts
struct CacheManager {
    cache_dir: PathBuf,
}

impl CacheManager {
    /// Create a new cache manager with the specified cache directory
    fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Expire old artifacts from the cache
    async fn expire_artifacts(&self, age: Duration) -> Result<()> {
        // walk the cache directory, looking for artifact dirs
        // and remove those older than the specified age
        let mut dir_entries = match tokio::fs::read_dir(&self.cache_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context("failed to read cache directory"),
        };
        while let Some(entry) = dir_entries
            .next_entry()
            .await
            .context("failed to read cache entry")?
        {
            let metadata = entry
                .metadata()
                .await
                .context("failed to read cache entry metadata")?;
            if let Ok(modified) = metadata.modified() {
                let modified_duration = modified
                    .elapsed()
                    .context("failed to compute modified duration")?;
                if modified_duration > age {
                    debug!(path = %entry.path().display(), "expiring cached artifact");
                    tokio::fs::remove_dir_all(entry.path())
                        .await
                        .context("failed to remove expired cache entry")?;
                }
            }
        }

        Ok(())
    }

    /// Get the cache directory for a given OCI reference
    fn get_cache_dir(&self, reference: &str) -> PathBuf {
        // Hash for uniqueness, but keep the reference in the path for readability
        let mut hasher = Sha256::new();
        hasher.update(reference.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let short_hash = &hash[..8];

        // Sanitize the reference for filesystem use
        let sanitized = reference.replace(['/', ':', '@'], "_");

        // Directory: <cache_dir>/<sanitized_reference>_<short_hash>/
        self.cache_dir.join(format!("{sanitized}_{short_hash}"))
    }

    /// Get the cache path for the component .wasm data
    fn get_component_path(&self, reference: &str) -> PathBuf {
        let cache_dir = self.get_cache_dir(reference);

        // Use the last segment as the artifact name (after last '/')
        let artifact_name = reference
            .rsplit('/')
            .next()
            .unwrap_or("artifact")
            .replace([':', '@'], "_");

        cache_dir.join(format!("{artifact_name}.wasm"))
    }

    /// Get the cache path for the digest file
    fn get_digest_path(&self, reference: &str) -> PathBuf {
        let cache_dir = self.get_cache_dir(reference);
        cache_dir.join("digest")
    }

    /// Check if an artifact is cached (both component and digest must exist)
    async fn is_cached(&self, reference: &str) -> bool {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);
        tokio::fs::metadata(&component_path).await.is_ok()
            && tokio::fs::metadata(&digest_path).await.is_ok()
    }

    /// Read cached artifact, returning (component_data, digest)
    async fn read_cached(&self, reference: &str) -> Result<(Vec<u8>, String)> {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);

        debug!(component_path = %component_path.display(), digest_path = %digest_path.display(), "reading cached artifact");

        let component_data = tokio::fs::read(&component_path).await.with_context(|| {
            format!(
                "failed to read cached component at {}",
                component_path.display()
            )
        })?;

        let digest = tokio::fs::read_to_string(&digest_path)
            .await
            .with_context(|| {
                format!("failed to read cached digest at {}", digest_path.display())
            })?;

        Ok((component_data, digest.trim().to_string()))
    }

    /// Write artifact and digest to cache
    async fn write_to_cache(&self, reference: &str, data: &[u8], digest: &str) -> Result<()> {
        let component_path = self.get_component_path(reference);
        let digest_path = self.get_digest_path(reference);

        debug!(component_path = %component_path.display(), digest_path = %digest_path.display(), "writing to cache");

        // Create cache directory
        let cache_dir = self.get_cache_dir(reference);
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

        // Write component data
        tokio::fs::write(&component_path, data)
            .await
            .with_context(|| {
                format!(
                    "failed to write component to cache at {}",
                    component_path.display()
                )
            })?;

        // Write digest
        tokio::fs::write(&digest_path, digest)
            .await
            .with_context(|| {
                format!(
                    "failed to write digest to cache at {}",
                    digest_path.display()
                )
            })?;

        Ok(())
    }
}

/// Credential resolver that implements the precedence: explicit → docker creds → anonymous
struct CredentialResolver {
    explicit_credentials: Option<(String, String)>,
}

impl CredentialResolver {
    fn new(explicit_credentials: Option<(String, String)>) -> Self {
        Self {
            explicit_credentials,
        }
    }

    /// Resolve credentials for a given registry
    #[instrument(skip(self), fields(registry = %registry))]
    async fn resolve_credentials(&self, registry: &str) -> RegistryAuth {
        // First, try explicit credentials
        if let Some((username, password)) = &self.explicit_credentials {
            debug!("using explicit credentials");
            return RegistryAuth::Basic(username.clone(), password.clone());
        }

        // Next, try docker credential helper
        match self.get_docker_credentials(registry).await {
            Ok(Some(auth)) => {
                debug!("using docker credential helper");
                return auth;
            }
            Ok(None) => debug!("no docker credentials found"),
            Err(e) => warn!(error = %e, "failed to retrieve docker credentials"),
        }

        // Fall back to anonymous
        debug!("Using anonymous access");
        RegistryAuth::Anonymous
    }

    /// Attempt to retrieve credentials from docker credential helper
    async fn get_docker_credentials(&self, registry: &str) -> Result<Option<RegistryAuth>> {
        match get_credential(registry) {
            Ok(DockerCredential::UsernamePassword(user, pass)) => {
                Ok(Some(RegistryAuth::Basic(user, pass)))
            }
            Ok(DockerCredential::IdentityToken(_)) => {
                bail!("docker credential helper returned identity token, which is not supported")
            }
            Err(
                CredentialRetrievalError::ConfigNotFound
                | CredentialRetrievalError::NoCredentialConfigured,
            ) => Ok(None),
            // Edge case for macOS, shows as an error when really it's just not found
            Err(CredentialRetrievalError::HelperFailure { stdout, .. })
                if stdout.contains("credentials not found in native keychain") =>
            {
                Ok(None)
            }
            Err(e) => Err(anyhow!("docker credential retrieval error: {e}")),
        }
    }
}

/// OCI pull policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OciPullPolicy {
    /// ️ Always pull the component from the registry
    Always,
    /// ️ Pull the component only if not present in cache
    IfNotPresent,
    /// ️ Never pull the component; use only cached version
    Never,
}

impl std::str::FromStr for OciPullPolicy {
    type Err = anyhow::Error;

    /// Parse a pull policy name: `always`, `ifNotPresent`, or `never`
    /// (case- and separator-insensitive).
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "always" => Ok(Self::Always),
            "ifnotpresent" => Ok(Self::IfNotPresent),
            "never" => Ok(Self::Never),
            other => bail!("invalid pull policy {other:?}; expected always|ifNotPresent|never"),
        }
    }
}

/// Pull a WebAssembly component from an OCI registry
///
/// This function pulls a WebAssembly component from an OCI-compliant registry,
/// validates it, and optionally caches it for future use.
///
/// # Arguments
/// * `reference` - OCI reference (e.g., "registry.io/my/component:v1.0.0")
/// * `config` - Configuration for the pull operation
///
/// # Returns
/// Raw bytes of the WebAssembly component
///
/// # Errors
/// Returns an error if:
/// - The reference is invalid
/// - The registry is unreachable
/// - Authentication fails
/// - The pulled artifact is not a valid WebAssembly component
/// - Caching operations fail
///
/// # Examples
/// ```no_run
/// use wash_runtime::oci::{pull_component, OciConfig, OciPullPolicy};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = OciConfig::default();
///     let (component_bytes, _digest) = pull_component("ghcr.io/wasmcloud/components/http-hello-world:latest", config, OciPullPolicy::IfNotPresent).await?;
///     println!("Successfully pulled {} bytes", component_bytes.len());
///     Ok(())
/// }
/// ```
/// Fetch a reference's OCI manifest — annotations included — without pulling
/// any layer.
///
/// The read side of the topology annotations `push_component` writes: an
/// inspector wants the workload shape and its digest, both of which live in
/// the manifest, and pulling megabytes of wasm to read a kilobyte of metadata
/// would make inspection cost what it is meant to avoid.
/// Parse a reference and build the client + resolved credentials for it.
///
/// The one home for the protocol/trust-roots/credential dance — three call
/// sites used to carry verbatim copies, and they had already drifted (the
/// manifest fetch forgot the timeout its siblings honour).
async fn client_and_auth(
    reference: &str,
    config: &OciConfig,
) -> Result<(Reference, Client, RegistryAuth)> {
    let reference_parsed = Reference::try_from(reference)
        .with_context(|| format!("invalid OCI reference: {reference}"))?;

    let client_config = ClientConfig {
        protocol: if config.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        extra_root_certificates: extra_ca_certificates(),
        ..Default::default()
    };
    let client = Client::new(client_config);

    let credential_resolver = CredentialResolver::new(config.credentials.clone());
    let auth = credential_resolver
        .resolve_credentials(reference_parsed.registry())
        .await;
    Ok((reference_parsed, client, auth))
}

pub async fn fetch_manifest(
    reference: &str,
    config: OciConfig,
) -> Result<oci_client::manifest::OciImageManifest> {
    let (reference_parsed, client, auth) = client_and_auth(reference, &config).await?;

    let fetch = client.pull_image_manifest(&reference_parsed, &auth);
    let (manifest, _digest) = if let Some(timeout) = config.timeout {
        tokio::time::timeout(timeout, fetch)
            .await
            .with_context(|| format!("timed out fetching the manifest for {reference}"))?
    } else {
        fetch.await
    }
    .with_context(|| format!("failed to fetch the manifest for {reference}"))?;
    Ok(manifest)
}

#[instrument(skip(config), fields(reference = %reference, pull_policy = ?pull_policy))]
pub async fn pull_component(
    reference: &str,
    config: OciConfig,
    pull_policy: OciPullPolicy,
) -> Result<(Vec<u8>, String)> {
    // Parse OCI reference
    let (reference_parsed, client, auth) = client_and_auth(reference, &config).await?;

    // Initialize cache manager
    let cache_manager = config
        .cache_dir
        .as_ref()
        .map(|dir| CacheManager::new(dir.clone()));
    if let Some(cache_manager) = &cache_manager {
        // Check cache first
        if pull_policy != OciPullPolicy::Always && cache_manager.is_cached(reference).await {
            debug!("Found cached artifact");
            let (component_data, digest) = cache_manager.read_cached(reference).await?;

            let fetched_digest = client
                .fetch_manifest_digest(&reference_parsed, &auth)
                .await?;

            if digest == fetched_digest {
                return Ok((component_data, digest));
            }

            debug!("Cached artifact expired; pulling new component version");
        }
    }

    if pull_policy == OciPullPolicy::Never {
        bail!("component not found in cache and pull policy is 'Never'");
    }

    // Pull the component using oci-client
    let pull_future = client.pull(
        &reference_parsed,
        &auth,
        vec![
            WASM_LAYER_MEDIA_TYPE,
            #[allow(deprecated)]
            WASMCLOUD_MEDIA_TYPE,
        ],
    );

    // Apply timeout if configured, otherwise just await the pull
    let image_data = if let Some(timeout) = config.timeout {
        tokio::time::timeout(timeout, pull_future)
            .await
            .with_context(|| {
                format!("timeout pulling component from {reference} after {timeout:?}")
            })?
            .with_context(|| format!("failed to pull component from {reference}"))?
    } else {
        pull_future
            .await
            .with_context(|| format!("failed to pull component from {reference}"))?
    };

    // Extract the component bytes from the first layer
    let component_data = image_data
        .layers
        .first()
        .ok_or_else(|| anyhow!("no layers found in pulled artifact"))?
        .data
        .clone();
    let digest = image_data
        .digest
        .ok_or_else(|| anyhow!("no digest found in pulled artifact"))?;

    // Validate that it's a valid WebAssembly component
    validate_component(&component_data)
        .await
        .with_context(|| "pulled artifact is not a valid WebAssembly component")?;

    // Cache the component with its digest
    if let Some(cache_manager) = &cache_manager {
        cache_manager
            .write_to_cache(reference, &component_data, &digest)
            .await
            .with_context(|| "failed to cache component")?;
    }

    // oci-client 0.17 hands back layer data as `Bytes`; callers expect `Vec<u8>`.
    Ok((component_data.to_vec(), digest))
}

/// Push a WebAssembly component to an OCI registry
///
/// This function validates a WebAssembly component and pushes it to an OCI-compliant registry.
///
/// # Arguments
/// * `reference` - OCI reference (e.g., "registry.io/my/component:v1.0.0")
/// * `component_data` - Raw bytes of the WebAssembly component
/// * `config` - Configuration for the push operation
/// * `annotations` - Optional OCI annotations to add to the manifest
///
/// # Returns
/// The digest of the pushed component
///
/// # Errors
/// Returns an error if:
/// - The reference is invalid
/// - The component data is not valid WebAssembly
/// - Authentication fails
/// - The registry is unreachable
/// - Push operation fails
///
/// # Examples
/// ```no_run
/// use wash_runtime::oci::{push_component, OciConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let component_bytes = std::fs::read("my-component.wasm")?;
///     let config = OciConfig::default();
///     let digest = push_component("registry.example.com/my-component:latest", &component_bytes, config, None).await?;
///     println!("Pushed component with digest: {}", digest);
///     Ok(())
/// }
/// ```
#[instrument(
    skip(component_data, config, annotations),
    fields(
        reference = %reference,
        size = component_data.len(),
        annotation_count = annotations.as_ref().map_or(0, |a| a.len())
    )
)]
pub async fn push_component(
    reference: &str,
    component_data: &[u8],
    config: OciConfig,
    annotations: Option<HashMap<String, String>>,
) -> Result<String> {
    // Validate the component before pushing
    validate_component(component_data)
        .await
        .with_context(|| "component data is not a valid WebAssembly component")?;

    let (reference_parsed, client, auth) = client_and_auth(reference, &config).await?;

    // Create the WebAssembly configuration and layer using oci-wasm
    let (wasm_config, image_layer) = WasmConfig::from_raw_component(component_data.to_vec(), None)
        .with_context(|| "failed to create WebAssembly configuration from component")?;

    let layers = vec![image_layer];
    let config_obj = wasm_config
        .to_config()
        .with_context(|| "failed to convert WebAssembly config")?;

    // Create custom manifest with annotations if provided
    let manifest = annotations.filter(|a| !a.is_empty()).map(|annotations| {
        // Convert HashMap to BTreeMap for annotations
        let btree_annotations: BTreeMap<String, String> = annotations.into_iter().collect();

        // Create manifest descriptors for the config and layers
        let config_descriptor = OciDescriptor {
            media_type: config_obj.media_type.clone(),
            digest: config_obj.sha256_digest(),
            size: config_obj.data.len() as i64,
            urls: None,
            annotations: None,
            artifact_type: None,
        };

        let layer_descriptors: Vec<OciDescriptor> = layers
            .iter()
            .map(|layer| OciDescriptor {
                media_type: layer.media_type.clone(),
                digest: layer.sha256_digest(),
                size: layer.data.len() as i64,
                urls: None,
                annotations: None,
                artifact_type: None,
            })
            .collect();

        OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: config_descriptor,
            layers: layer_descriptors,
            subject: None,
            artifact_type: None,
            annotations: Some(btree_annotations),
        }
    });

    // Push the component
    let push_future = client.push(&reference_parsed, &layers, config_obj, &auth, manifest);

    // Apply timeout if configured, otherwise just await the push
    let push_result = if let Some(timeout) = config.timeout {
        tokio::time::timeout(timeout, push_future)
            .await
            .with_context(|| format!("timeout pushing component to {reference} after {timeout:?}"))?
            .with_context(|| format!("failed to push component to {reference}"))?
    } else {
        push_future
            .await
            .with_context(|| format!("failed to push component to {reference}"))?
    };

    // Extract the digest from the manifest URL
    // The manifest URL typically contains the digest in the format: registry/repo@sha256:digest
    let digest = if let Some(digest_part) = push_result.manifest_url.split('@').nth(1) {
        digest_part.to_string()
    } else {
        // Fetch the manifest digest from the registry
        client
            .fetch_manifest_digest(&reference_parsed, &auth)
            .await
            .with_context(|| format!("failed to fetch manifest digest for {reference}"))?
    };
    // Cache the pushed component with its digest
    if let Some(cache_dir) = config.cache_dir {
        let cache_manager = CacheManager::new(cache_dir);
        cache_manager
            .write_to_cache(reference, component_data, &digest)
            .await
            .with_context(|| "failed to cache pushed component")?;
    }

    Ok(digest)
}

/// Validate that the provided bytes represent a valid WebAssembly component
///
/// This function parses the WebAssembly bytes and validates that they form
/// a valid WebAssembly component, not just a raw module.
///
/// # Arguments
/// * `data` - The raw bytes to validate
///
/// # Returns
/// Returns `Ok(())` if the data represents a valid WebAssembly component,
/// otherwise returns an error describing why validation failed.
///
/// # Examples
/// ```no_run
/// use wash_runtime::oci::validate_component;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let component_bytes = std::fs::read("my-component.wasm")?;
///     validate_component(&component_bytes).await?;
///     println!("Component is valid!");
///     Ok(())
/// }
/// ```
pub async fn validate_component(data: &[u8]) -> Result<()> {
    wit_component::decode_reader(data)
        .context("failed to decode component bytes")
        .map(|_| ())
}

/// Cleanup cached OCI artifacts
#[instrument(skip(cache_dir))]
pub async fn cleanup_cache(cache_dir: impl AsRef<Path>, age: Duration) -> Result<()> {
    let cache_manager = CacheManager::new(cache_dir.as_ref().to_path_buf());
    cache_manager.expire_artifacts(age).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A bundle that cannot be read has to fail loudly at startup. Trust is
    /// configured once and used much later, so a silently skipped bundle would
    /// surface as every pull from that registry failing to verify, far from
    /// the typo that caused it.
    #[test]
    fn missing_ca_bundle_is_an_error() {
        let err = load_ca_certificates(&[PathBuf::from("/definitely/not/a/ca.pem")])
            .expect_err("a missing CA bundle must not be skipped");
        assert!(
            err.to_string().contains("CA bundle"),
            "the error should name what it failed to read, got: {err}"
        );
    }

    /// A self-signed certificate, PEM encoded, for the bundles below.
    fn test_certificate_pem(name: &str) -> String {
        rcgen::generate_simple_self_signed(vec![name.to_string()])
            .expect("generating a test certificate")
            .cert
            .pem()
    }

    #[test]
    fn ca_bundles_are_read_as_pem() {
        let dir = TempDir::new().unwrap();
        let (first, second) = (dir.path().join("a.pem"), dir.path().join("b.pem"));
        let (first_pem, second_pem) = (
            test_certificate_pem("a.test"),
            test_certificate_pem("b.test"),
        );
        std::fs::write(&first, &first_pem).unwrap();
        std::fs::write(&second, &second_pem).unwrap();

        let certs = load_ca_certificates(&[first, second]).expect("both bundles should load");
        assert_eq!(certs.len(), 2, "every bundle is kept, not just the last");
        // PEM, not DER: the bytes are handed to oci-client as read, so the
        // encoding has to match what is on disk or verification fails at use.
        assert!(matches!(certs[0].encoding, CertificateEncoding::Pem));
        assert_eq!(certs[0].data, first_pem.as_bytes());
        assert_eq!(certs[1].data, second_pem.as_bytes());
    }

    /// A bundle holding several certificates is kept whole: `oci-client` reads
    /// every certificate out of one PEM blob, so splitting or truncating it
    /// would drop trust roots the operator asked for.
    #[test]
    fn a_bundle_may_hold_several_certificates() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bundle.pem");
        let bundle = test_certificate_pem("one.test") + &test_certificate_pem("two.test");
        std::fs::write(&path, &bundle).unwrap();

        let certs = load_ca_certificates(&[path]).expect("a multi-certificate bundle should load");
        assert_eq!(certs.len(), 1, "one file is one entry, however many certs");
        assert_eq!(certs[0].data, bundle.as_bytes());
    }

    /// Content is parsed at load, not at first pull. `oci-client`'s
    /// `Client::new` reacts to an unparseable certificate by logging and
    /// building a client from defaults, losing the registry protocol and
    /// timeouts along with the trust roots. A bundle that would fail there has
    /// to fail here instead.
    #[test]
    fn unparseable_ca_bundles_are_rejected() {
        let dir = TempDir::new().unwrap();
        for (name, contents) in [
            ("garbage.pem", "not a certificate at all\n".as_bytes()),
            ("empty.pem", b""),
            // Correct framing, contents that are not a certificate: the shape
            // a truncated or wrongly-typed file takes.
            (
                "framed.pem",
                b"-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n",
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, contents).unwrap();
            let err = load_ca_certificates(&[path])
                .expect_err("{name} must not be accepted as a CA bundle");
            assert!(
                err.to_string().contains("invalid OCI CA bundle"),
                "the error should name the bundle it rejected, got: {err}"
            );
        }
    }

    #[test]
    fn no_ca_bundles_means_the_compiled_in_roots() {
        assert!(
            load_ca_certificates(&[]).unwrap().is_empty(),
            "an empty list must not invent a root; the default trust is oci-client's own"
        );
    }

    #[test]
    fn test_oci_config_default() {
        let config = OciConfig::default();
        assert!(config.credentials.is_none());
        assert!(!config.insecure);
        assert!(config.cache_dir.is_none());
    }

    #[test]
    fn test_oci_config_with_credentials() {
        let config = OciConfig::new_with_credentials("user".to_string(), "pass".to_string());
        assert_eq!(
            config.credentials,
            Some(("user".to_string(), "pass".to_string()))
        );
    }

    #[test]
    fn test_oci_config_insecure() {
        let config = OciConfig::new_insecure();
        assert!(config.insecure);
    }

    #[test]
    fn test_cache_manager_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf());

        let reference = "localhost:5000/test:latest";
        let component_path = cache_manager.get_component_path(reference);
        let digest_path = cache_manager.get_digest_path(reference);

        assert!(component_path.starts_with(temp_dir.path()));
        assert!(component_path.extension().unwrap() == "wasm");
        assert!(digest_path.starts_with(temp_dir.path()));
        assert!(digest_path.file_name().unwrap() == "digest");
    }

    #[tokio::test]
    async fn test_cache_manager_operations() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf());

        let reference = "localhost:5000/test:v1.0.0";
        let test_data = b"test component data";
        let test_digest = "sha256:abcd1234";

        // Should not be cached initially
        assert!(!cache_manager.is_cached(reference).await);

        // Cache the data with digest
        cache_manager
            .write_to_cache(reference, test_data, test_digest)
            .await
            .unwrap();

        // Should now be cached
        assert!(cache_manager.is_cached(reference).await);

        // Should be able to read the cached data and digest
        let (cached_data, cached_digest) = cache_manager.read_cached(reference).await.unwrap();
        assert_eq!(cached_data, test_data);
        assert_eq!(cached_digest, test_digest);
    }

    #[tokio::test]
    async fn test_validate_component_invalid_data() {
        let invalid_data = b"not wasm data";
        let result = validate_component(invalid_data).await;
        assert!(result.is_err());
    }

    // Integration test with real registry - marked `#[ignore]`, run with `cargo test --include-ignored`
    #[tokio::test]
    #[ignore = "hits a real OCI registry (network); run with `cargo test --include-ignored`"]
    async fn test_pull_and_validate_ghcr_component() {
        // Use public OCI references for testing
        let references = vec![
            // wasmCloud hello world component
            "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0",
            // Bytecode Alliance sample component
            "ghcr.io/bytecodealliance/sample-wasi-http-rust/sample-wasi-http-rust:latest",
        ];

        let config = OciConfig::default();

        // Pull the component anonymously
        for reference in references {
            let (component_bytes, digest) =
                pull_component(reference, config.clone(), OciPullPolicy::IfNotPresent)
                    .await
                    .expect("Failed to pull component");

            let res = validate_component(&component_bytes).await;
            assert!(
                res.is_ok(),
                "Component validation failed for {reference}: {}",
                res.unwrap_err()
            );

            // Verify digest format
            assert!(
                digest.starts_with("sha256:"),
                "Digest should start with sha256:"
            );
        }
    }

    #[test]
    fn test_oci_config_with_cache() {
        let temp_dir = TempDir::new().unwrap();
        let config = OciConfig::new_with_cache(temp_dir.path().to_path_buf());

        assert!(config.cache_dir.is_some());
        assert_eq!(config.cache_dir.unwrap(), temp_dir.path());
        assert!(config.credentials.is_none());
        assert!(!config.insecure);
    }
}
