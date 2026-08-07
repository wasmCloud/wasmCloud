//! Where a WebAssembly component's bytes come from, and the one way to fetch
//! them.
//!
//! Every front-end that runs or reads a component names its wasm the same two
//! ways: an OCI image reference or a local file path. Workload components and
//! services, `wash dev` components, host component plugins, `wash oci pull` and
//! `wash inspect` all make that choice. [`ComponentSource`] is the choice and
//! [`ComponentSource::load`] is the fetch, so a caller that gains one gains the
//! other.
//!
//! Three shapes of input converge here:
//!
//! - Separate `image` / `file` fields (a YAML config block, `key=value` CLI
//!   fields). [`ComponentSource::from_image_or_file`] enforces exactly one and
//!   rejects a pull policy on a file.
//! - A single positional reference with nothing to disambiguate it.
//!   [`ComponentSource::from_reference`] classifies it.
//! - An already-decided source uses the enum variants directly.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail, ensure};
use bytes::Bytes;

use crate::oci::{OciConfig, OciPullPolicy, pull_component};

/// Where a component's wasm bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentSource {
    /// Pulled from an OCI registry by image reference.
    Oci {
        image: String,
        pull_policy: OciPullPolicy,
    },
    /// Read from a local file path.
    File(PathBuf),
}

/// The bytes a [`ComponentSource`] resolved to, plus the identity they can be
/// cached under.
#[derive(Debug, Clone)]
pub struct LoadedComponent {
    pub bytes: Bytes,
    /// Registry digest of the pulled image, `None` for a file source.
    ///
    /// The engine keys its compilation cache on this. A local path carries no
    /// such identity. The same path can hold different bytes between two
    /// reads so a file source declines to be cached rather than offering a
    /// name that would collide with every other file.
    pub digest: Option<String>,
}

impl LoadedComponent {
    /// Check the fetched bytes against a pinned registry digest.
    ///
    /// Callers add their own context (which plugin, which component); this
    /// supplies the reason. A file source has no digest to check, so pinning
    /// one is a configuration error rather than a mismatch.
    pub fn verify_digest(&self, expected: &str) -> Result<()> {
        match &self.digest {
            Some(digest) if digest == expected => Ok(()),
            Some(digest) => bail!("digest mismatch: fetched {digest}, expected {expected}"),
            None => bail!("digest pinning applies only to `image` sources"),
        }
    }
}

impl ComponentSource {
    /// An OCI source that prefers the cache, the default nearly every caller
    /// wants.
    pub fn image(image: impl Into<String>) -> Self {
        Self::Oci {
            image: image.into(),
            pull_policy: OciPullPolicy::IfNotPresent,
        }
    }

    /// Resolve a source declared as separate `image` / `file` fields, requiring
    /// exactly one of them.
    ///
    /// `name` names the thing being configured and leads every error, so pass
    /// something the user can find in their config, e.g. `"host plugin 'acme-kv'"`,
    /// `"dev.components['sidecar']"`.
    ///
    /// `pull_policy` belongs to an image; supplying one alongside a file is
    /// rejected rather than ignored, so a config that reads as if it controls
    /// caching actually does.
    pub fn from_image_or_file(
        image: Option<String>,
        file: Option<PathBuf>,
        pull_policy: Option<OciPullPolicy>,
        name: &str,
    ) -> Result<Self> {
        match (image, file) {
            (Some(image), None) => Ok(Self::Oci {
                image,
                pull_policy: pull_policy.unwrap_or(OciPullPolicy::IfNotPresent),
            }),
            (None, Some(file)) => {
                ensure!(
                    pull_policy.is_none(),
                    "{name}: pull policy applies only to `image` sources"
                );
                ensure!(
                    !file.as_os_str().is_empty(),
                    "{name}: `file` source is empty"
                );
                Ok(Self::File(file))
            }
            (Some(_), Some(_)) => {
                bail!("{name} sets both `image` and `file`; use exactly one")
            }
            (None, None) => bail!("{name} needs an `image` or `file` source"),
        }
    }

    /// Classify a single user-supplied reference, for a positional CLI argument
    /// where no separate field says which kind it is.
    ///
    /// An existing path is a file. Otherwise anything spelled like a path — an
    /// absolute, home-relative or explicitly relative prefix, or a `.wasm`
    /// extension — stays a file so that a mistyped path reports a missing file
    /// instead of an obscure registry error. Everything else is an image
    /// reference.
    pub fn from_reference(reference: &str) -> Self {
        let path = Path::new(reference);
        if path.exists() || spelled_like_a_path(reference) {
            Self::File(path.to_path_buf())
        } else {
            Self::image(reference)
        }
    }

    /// Fetch the bytes and check them against a pinned registry digest.
    ///
    /// A source that carries no digest is rejected before any bytes move, so a
    /// pin on a file is a configuration error rather than wasted I/O.
    pub async fn load_pinned(
        &self,
        oci_config: OciConfig,
        expected_digest: Option<&str>,
    ) -> Result<LoadedComponent> {
        let Some(expected) = expected_digest else {
            return self.load(oci_config).await;
        };
        ensure!(
            matches!(self, Self::Oci { .. }),
            "digest pinning applies only to `image` sources"
        );
        let loaded = self.load(oci_config).await?;
        loaded.verify_digest(expected)?;
        Ok(loaded)
    }

    /// Fetch the bytes: pull the image or read the file.
    ///
    /// `oci_config` is ignored by a file source, so callers that accept either
    /// kind can build one unconditionally.
    pub async fn load(&self, oci_config: OciConfig) -> Result<LoadedComponent> {
        match self {
            Self::Oci { image, pull_policy } => {
                // `pull_component` already names the reference it failed on.
                let (bytes, digest) = pull_component(image, oci_config, *pull_policy).await?;
                Ok(LoadedComponent {
                    bytes: bytes.into(),
                    digest: Some(digest),
                })
            }
            Self::File(path) => {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("failed to read component file {}", path.display()))?;
                Ok(LoadedComponent {
                    bytes: bytes.into(),
                    digest: None,
                })
            }
        }
    }
}

/// Whether a reference is written the way a filesystem path is written, for
/// [`ComponentSource::from_reference`] to fall back on when nothing exists at
/// that path yet.
fn spelled_like_a_path(reference: &str) -> bool {
    reference.starts_with('/')
        || reference.starts_with('~')
        || reference.starts_with("./")
        || reference.starts_with(".\\")
        || reference.starts_with("../")
        || reference.starts_with("..\\")
        || Path::new(reference)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wasm"))
}

impl std::fmt::Display for ComponentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oci { image, .. } => write!(f, "{image}"),
            Self::File(path) => write!(f, "{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_or_file_takes_exactly_one_source() {
        let image = ComponentSource::from_image_or_file(
            Some("ghcr.io/acme/kv:1".into()),
            None,
            Some(OciPullPolicy::Always),
            "host plugin 'kv'",
        )
        .unwrap();
        assert_eq!(
            image,
            ComponentSource::Oci {
                image: "ghcr.io/acme/kv:1".into(),
                pull_policy: OciPullPolicy::Always,
            }
        );

        let file = ComponentSource::from_image_or_file(
            None,
            Some("./kv.wasm".into()),
            None,
            "host plugin 'kv'",
        )
        .unwrap();
        assert_eq!(file, ComponentSource::File("./kv.wasm".into()));

        for (image, file) in [
            (
                Some("ghcr.io/acme/kv:1".to_string()),
                Some(PathBuf::from("./kv.wasm")),
            ),
            (None, None),
        ] {
            assert!(
                ComponentSource::from_image_or_file(image, file, None, "host plugin 'kv'").is_err()
            );
        }
    }

    #[test]
    fn image_defaults_to_cached_pull_and_file_refuses_a_pull_policy() {
        let image =
            ComponentSource::from_image_or_file(Some("ghcr.io/acme/kv:1".into()), None, None, "kv")
                .unwrap();
        assert_eq!(
            image,
            ComponentSource::Oci {
                image: "ghcr.io/acme/kv:1".into(),
                pull_policy: OciPullPolicy::IfNotPresent,
            }
        );

        let err = ComponentSource::from_image_or_file(
            None,
            Some("./kv.wasm".into()),
            Some(OciPullPolicy::Always),
            "host plugin 'kv'",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("host plugin 'kv'"), "{err}");
        assert!(err.contains("pull policy"), "{err}");
    }

    #[test]
    fn empty_file_source_is_rejected() {
        assert!(
            ComponentSource::from_image_or_file(None, Some(PathBuf::new()), None, "sidecar")
                .is_err()
        );
    }

    #[test]
    fn reference_classifies_paths_and_images() {
        for path in [
            "./kv.wasm",
            "../build/kv.wasm",
            "/opt/kv.wasm",
            "~/kv.wasm",
            "build/kv.wasm",
            "kv.WASM",
        ] {
            assert_eq!(
                ComponentSource::from_reference(path),
                ComponentSource::File(path.into()),
                "{path} should be a file"
            );
        }

        for image in [
            "ghcr.io/acme/kv:1.0.0",
            "ghcr.io/acme/kv@sha256:abc",
            "localhost:5000/kv:latest",
            "kv",
        ] {
            assert_eq!(
                ComponentSource::from_reference(image),
                ComponentSource::image(image),
                "{image} should be an image"
            );
        }
    }

    #[test]
    fn an_existing_path_wins_over_image_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kv");
        std::fs::write(&path, b"\0asm").unwrap();
        let reference = path.to_str().unwrap();

        assert_eq!(
            ComponentSource::from_reference(reference),
            ComponentSource::File(path.clone())
        );
    }

    #[tokio::test]
    async fn a_file_source_loads_bytes_without_a_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kv.wasm");
        std::fs::write(&path, b"\0asm\x0d\0\x01\0").unwrap();

        let loaded = ComponentSource::File(path)
            .load(OciConfig::default())
            .await
            .unwrap();
        assert_eq!(loaded.bytes.as_ref(), b"\0asm\x0d\0\x01\0");
        assert!(loaded.digest.is_none());
    }

    #[tokio::test]
    async fn a_missing_file_names_the_path() {
        let err = ComponentSource::File("/nonexistent/kv.wasm".into())
            .load(OciConfig::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("/nonexistent/kv.wasm"), "{err}");
    }

    #[tokio::test]
    async fn a_digest_pin_on_a_file_is_rejected_before_reading_it() {
        // The path does not exist: reaching the read at all would report a
        // missing file instead of the pin being misplaced.
        let err = ComponentSource::File("/nonexistent/kv.wasm".into())
            .load_pinned(OciConfig::default(), Some("sha256:abc"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("digest pinning"), "{err}");
    }

    #[test]
    fn digest_pinning_needs_a_digest_to_check() {
        let file = LoadedComponent {
            bytes: Bytes::new(),
            digest: None,
        };
        assert!(file.verify_digest("sha256:abc").is_err());

        let pulled = LoadedComponent {
            bytes: Bytes::new(),
            digest: Some("sha256:abc".into()),
        };
        assert!(pulled.verify_digest("sha256:abc").is_ok());
        assert!(pulled.verify_digest("sha256:def").is_err());
    }
}
